// What commands replaced, kept to undo them, and the undos, to redo them.

use rusqlite::ToSql;
use rusqlite::types::Value;

use super::{Error, Write};
use crate::database::schema::Table;

/// Commands kept to undo. Past this, the oldest is forgotten.
const HISTORY: usize = 100;
/// Tables whose changes aren't undone: preferences aren't edits, so Ctrl+Z
/// never changes the theme.
const NOT_UNDONE: [&str; 1] = ["settings"];

/// What undo needs of a table, without its type.
#[derive(Clone, Copy, Debug)]
pub(super) struct Shape {
    pub(super) name: &'static str,
    key: &'static [&'static str],
    pub(super) device: bool,
}

impl Shape {
    pub(super) fn of<T: Table>() -> Self {
        Self {
            name: T::NAME,
            key: T::KEY,
            device: T::DEVICE,
        }
    }

    /// device.sqlite holds where the window was and which page was open:
    /// never edits.
    pub(super) fn undoable(self) -> bool {
        !self.device && !NOT_UNDONE.contains(&self.name)
    }

    /// `"id" = ?first`, and so on for each key column, for the key's values
    /// as parameters from `first` on.
    pub(super) fn condition(self, first: usize) -> String {
        self.key
            .iter()
            .zip(first..)
            .map(|(column, index)| format!("\"{column}\" = ?{index}"))
            .collect::<Vec<_>>()
            .join(" AND ")
    }
}

/// One change a command made, with what it replaced.
#[derive(Debug)]
pub(super) enum Step {
    /// Fields of one row set in one statement: each one's name, value before
    /// and value after. Undone in one statement too, since CHECKs may tie
    /// them together, like an event's kind and its interview kind.
    Fields {
        shape: Shape,
        key: Vec<Value>,
        fields: Vec<(&'static str, Value, Value)>,
    },
    /// A row added, undone by marking it deleted.
    Inserted { shape: Shape, key: Vec<Value> },
}

/// The last `HISTORY` commands' steps, newest last. A ring of fixed size: it
/// never grows or moves, and pushing onto a full one drops the oldest.
pub(super) struct History {
    slots: [Option<Vec<Step>>; HISTORY],
    /// Where the next push goes.
    next: usize,
    len: usize,
}

impl History {
    pub(super) fn new() -> Self {
        Self {
            slots: [const { None }; HISTORY],
            next: 0,
            len: 0,
        }
    }

    pub(super) fn push(&mut self, steps: Vec<Step>) {
        self.slots[self.next] = Some(steps);
        self.next = (self.next + 1) % HISTORY;
        self.len = (self.len + 1).min(HISTORY);
    }

    pub(super) fn pop(&mut self) -> Option<Vec<Step>> {
        if self.len == 0 {
            return None;
        }
        self.next = (self.next + HISTORY - 1) % HISTORY;
        self.len -= 1;
        self.slots[self.next].take()
    }

    pub(super) fn clear(&mut self) {
        self.slots.fill_with(|| None);
        self.len = 0;
    }
}

impl Write<'_> {
    /// Writes back what `steps` replaced, newest first. A row changed since,
    /// by a later command or another device, keeps the newer values.
    pub(super) fn apply(&mut self, steps: &[Step]) -> Result<(), Error> {
        for step in steps.iter().rev() {
            match step {
                Step::Fields { shape, key, fields } => {
                    let key: Vec<&dyn ToSql> = key.iter().map(|value| value as &dyn ToSql).collect();
                    let current =
                        self.read_fields(*shape, &key, fields.iter().map(|(field, _, _)| *field))?;
                    let unchanged = current.is_some_and(|current| {
                        current.iter().eq(fields.iter().map(|(_, _, after)| after))
                    });
                    if !unchanged {
                        continue;
                    }
                    let before: Vec<(&'static str, &dyn ToSql)> = fields
                        .iter()
                        .map(|(field, before, _)| (*field, before as &dyn ToSql))
                        .collect();
                    self.update_row(*shape, &key, &before)?;
                }
                Step::Inserted { shape, key } => {
                    let key: Vec<&dyn ToSql> = key.iter().map(|value| value as &dyn ToSql).collect();
                    let deleted_at = self.read_fields(*shape, &key, ["deleted_at"].into_iter())?;
                    if deleted_at.as_deref() == Some(&[Value::Null]) {
                        let now = self.now;
                        self.update_row(*shape, &key, &[("deleted_at", &now)])?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::OptionalExtension;
    use uuid::Uuid;

    use super::*;
    use crate::database::Writer;
    use crate::database::schema::{Facts, device_settings, facts, settings};

    fn add_fact(writer: &mut Writer, text: &str) -> Result<Uuid, Error> {
        let id = Uuid::now_v7();
        writer.execute(|write| {
            write.insert(
                Facts,
                id,
                &[
                    facts::KIND.value(&"bullet"),
                    facts::TEXT.value(&text),
                    facts::EXTRACTED.value(&text),
                ],
            )
        })?;
        Ok(id)
    }

    /// The fact's text and when it was deleted; `None` if there's no row.
    fn fact(writer: &Writer, id: Uuid) -> Result<Option<(String, Option<i64>)>, Error> {
        Ok(writer
            .kron()
            .query_row("SELECT text, deleted_at FROM facts WHERE id = ?1", [id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?)
    }

    fn text(writer: &Writer, id: Uuid) -> Result<Option<String>, Error> {
        Ok(fact(writer, id)?.map(|(text, _)| text))
    }

    #[test]
    fn undo_puts_back_what_a_command_replaced_and_redo_does_it_again() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = add_fact(&mut writer, "Led the Rust rewrite")?;
        writer.execute(|write| write.set(facts::TEXT, id, &"Led the Go rewrite"))?;

        let changes = writer.undo()?;
        assert!(changes.is_some_and(|changes| !changes.is_empty()), "views hear of it");
        assert_eq!(text(&writer, id)?.as_deref(), Some("Led the Rust rewrite"));
        assert!(writer.redo()?.is_some());
        assert_eq!(text(&writer, id)?.as_deref(), Some("Led the Go rewrite"));
        assert!(writer.redo()?.is_none(), "one undo, one redo");
        Ok(())
    }

    #[test]
    fn undoing_an_insert_deletes_the_row_and_redoing_brings_it_back() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = add_fact(&mut writer, "Led the Rust rewrite")?;
        assert!(writer.undo()?.is_some());
        assert!(fact(&writer, id)?.is_some_and(|(_, deleted_at)| deleted_at.is_some()));
        assert!(writer.redo()?.is_some());
        assert_eq!(fact(&writer, id)?, Some(("Led the Rust rewrite".into(), None)));
        Ok(())
    }

    #[test]
    fn undoing_a_delete_restores_the_row() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = add_fact(&mut writer, "Led the Rust rewrite")?;
        writer.execute(|write| write.delete(Facts, id))?;
        assert!(writer.undo()?.is_some());
        assert_eq!(fact(&writer, id)?, Some(("Led the Rust rewrite".into(), None)));
        Ok(())
    }

    #[test]
    fn undo_goes_back_one_command_at_a_time() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = add_fact(&mut writer, "one")?;
        for text in ["two", "three"] {
            writer.execute(|write| write.set(facts::TEXT, id, &text))?;
        }
        writer.undo()?;
        assert_eq!(text(&writer, id)?.as_deref(), Some("two"));
        writer.undo()?;
        assert_eq!(text(&writer, id)?.as_deref(), Some("one"));
        writer.redo()?;
        assert_eq!(text(&writer, id)?.as_deref(), Some("two"));
        Ok(())
    }

    #[test]
    fn a_new_command_ends_redoing() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = add_fact(&mut writer, "one")?;
        writer.execute(|write| write.set(facts::TEXT, id, &"two"))?;
        writer.undo()?;
        writer.execute(|write| write.set(facts::TEXT, id, &"three"))?;
        assert!(writer.redo()?.is_none());
        assert_eq!(text(&writer, id)?.as_deref(), Some("three"));
        Ok(())
    }

    #[test]
    fn a_field_changed_since_keeps_the_newer_value() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = add_fact(&mut writer, "one")?;
        writer.execute(|write| write.set(facts::TEXT, id, &"two"))?;
        // As a sync from another device would write it.
        writer
            .kron()
            .execute("UPDATE facts SET text = 'from the laptop' WHERE id = ?1", [id])?;
        assert!(writer.undo()?.is_some_and(|changes| changes.is_empty()));
        assert_eq!(text(&writer, id)?.as_deref(), Some("from the laptop"));
        Ok(())
    }

    #[test]
    fn the_history_keeps_the_last_hundred_commands() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = add_fact(&mut writer, "0")?;
        for count in 1..=HISTORY + 5 {
            writer.execute(|write| write.set(facts::TEXT, id, &count.to_string()))?;
        }
        for _ in 0..HISTORY {
            assert!(writer.undo()?.is_some());
        }
        assert!(writer.undo()?.is_none());
        assert_eq!(text(&writer, id)?.as_deref(), Some("5"), "the first five are forgotten");
        Ok(())
    }

    #[test]
    fn preferences_and_this_devices_state_are_not_undone() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        writer.execute(|write| {
            write.set(settings::APPEARANCE, 1, &"dark")?;
            write.set(device_settings::SIDEBAR_COLLAPSED, 1, &true)
        })?;
        assert!(writer.undo()?.is_none());
        Ok(())
    }

    #[test]
    fn a_command_can_leave_itself_off_the_history() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = add_fact(&mut writer, "one")?;
        writer.execute(|write| {
            write.not_undoable();
            write.set(facts::TEXT, id, &"two")
        })?;
        writer.undo()?;
        assert!(fact(&writer, id)?.is_some_and(|(_, deleted_at)| deleted_at.is_some()), "undid the insert before it");
        Ok(())
    }

    #[test]
    fn a_failed_command_leaves_the_history_as_it_was() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = add_fact(&mut writer, "one")?;
        let failed = writer.execute(|write| {
            write.set(facts::TEXT, id, &"two")?;
            write.set(facts::TEXT, Uuid::now_v7(), &"no such fact")
        });
        assert!(failed.is_err());
        writer.undo()?;
        assert!(fact(&writer, id)?.is_some_and(|(_, deleted_at)| deleted_at.is_some()));
        Ok(())
    }

    #[test]
    fn erasing_everything_forgets_the_history() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        add_fact(&mut writer, "one")?;
        writer.execute(|write| write.erase_everything())?;
        assert!(writer.undo()?.is_none());
        Ok(())
    }
}

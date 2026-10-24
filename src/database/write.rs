use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::{Null, ToSqlOutput, Value, ValueRef};
use rusqlite::{CachedStatement, Connection, ToSql, Transaction, TransactionBehavior};

use super::schema::{Assignment, Column, Files, Fits, Key, Nullable, SoftDelete, Table, files};
use super::{Change, Error};

/// Distinct statements each connection keeps prepared. rusqlite's default of
/// 16 is fewer than the tables times the fields commands write, or the
/// queries the views run.
pub(super) const STATEMENT_CACHE: usize = 256;

/// Owns the write connections. Lives on the writer thread, or directly in a
/// test, and runs one command at a time.
pub struct Writer {
    kron: Connection,
    device: Connection,
    clock: Clock,
    /// Where `add_file` saves files; `None` in tests, which keep only the rows.
    files: Option<PathBuf>,
}

impl Writer {
    /// Takes both connections migrated, with foreign keys on.
    pub(super) fn new(
        kron: Connection,
        device: Connection,
        files: Option<PathBuf>,
    ) -> Result<Self, Error> {
        kron.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
        device.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
        let clock = Clock::recover(&kron, &device)?;
        Ok(Self {
            kron,
            device,
            clock,
            files,
        })
    }

    /// Both databases in memory, migrated: for tests of commands.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, Error> {
        let mut kron = Connection::open_in_memory()?;
        let mut device = Connection::open_in_memory()?;
        super::migrate(&mut kron, "kron.sqlite", super::KRON_MIGRATIONS)?;
        super::migrate(&mut device, "device.sqlite", super::DEVICE_MIGRATIONS)?;
        super::device_id(&device)?;
        Self::new(kron, device, None)
    }

    /// For tests to read back what a command wrote.
    #[cfg(test)]
    pub fn kron(&self) -> &Connection {
        &self.kron
    }

    #[cfg(test)]
    pub fn device(&self) -> &Connection {
        &self.device
    }

    /// Runs `command` in one transaction and returns what it returned with
    /// the rows it changed. If it fails, nothing it wrote is kept.
    pub fn execute<R>(
        &mut self,
        command: impl FnOnce(&mut Write) -> Result<R, Error>,
    ) -> Result<(R, Vec<Change>), Error> {
        let now = now();
        let sync_on: bool =
            self.device
                .query_row("SELECT pulled_until IS NOT NULL FROM device", [], |row| {
                    row.get(0)
                })?;
        // One clock value for the whole command: its changes happen together.
        let clock = sync_on.then(|| self.clock.next(now));
        let mut write = Write {
            transaction: self
                .kron
                .transaction_with_behavior(TransactionBehavior::Immediate)?,
            device: &self.device,
            device_transaction: None,
            clock,
            now,
            files: self.files.as_deref(),
            changes: Vec::new(),
        };
        let value = command(&mut write)?;
        let Write {
            transaction,
            device_transaction,
            changes,
            ..
        } = write;
        transaction.commit()?;
        // WAL makes no promise across two files, so a command writing both
        // could keep its kron.sqlite rows without these. None does yet.
        if let Some(device_transaction) = device_transaction {
            device_transaction.commit()?;
        }
        if let Some(clock) = clock {
            // The records are already committed, so a failure here isn't the
            // command's: the next start recovers the clock from the outbox.
            if let Err(error) = self
                .device
                .execute("UPDATE device SET clock = ?1 WHERE clock < ?1", [clock])
            {
                eprintln!("couldn't save the sync clock: {error}");
            }
        }
        Ok((value, changes))
    }
}

/// What a command writes through. Every write goes through `insert`, `set`,
/// `update`, `clear` or `delete`, which also note the change for the views,
/// and for sync when the table is in kron.sqlite. Reads use `prepare`, which refuses anything
/// that writes.
///
/// Tables and columns are the ones `build.rs` generates from the migrations,
/// so a wrong name, a value of the wrong type, a key of the wrong shape or
/// a change to a key fails to compile.
pub struct Write<'a> {
    transaction: Transaction<'a>,
    device: &'a Connection,
    /// Begun on the command's first write to a device.sqlite table.
    device_transaction: Option<Transaction<'a>>,
    /// The command's clock value while sync is on; `None`: sync is off, and
    /// the outbox stays empty.
    clock: Option<i64>,
    now: i64,
    files: Option<&'a Path>,
    changes: Vec<Change>,
}

impl Write<'_> {
    /// When the command started, in ms since the Unix epoch: one time for
    /// everything it writes.
    pub fn now(&self) -> i64 {
        self.now
    }

    /// Adds the row with this key, with `values` for its other columns.
    pub fn insert<T: Table>(
        &mut self,
        _table: T,
        key: impl Key<T::Key>,
        values: &[Assignment<T>],
    ) -> Result<(), Error> {
        let mut parameters = key.values();
        let row_key = row_key(&parameters)?;
        parameters.extend(values.iter().map(|assignment| assignment.value));
        let columns = T::KEY
            .iter()
            .copied()
            .chain(values.iter().map(|assignment| assignment.column))
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let placeholders = (1..=parameters.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        self.connection_of::<T>()?
            .prepare_cached(&format!(
                "INSERT INTO \"{}\" ({columns}) VALUES ({placeholders})",
                T::NAME
            ))?
            .execute(parameters.as_slice())?;

        if let Some(clock) = self.clock.filter(|_| !T::DEVICE) {
            self.transaction
                .prepare_cached(
                    "INSERT INTO outbox (table_name, row_key, field, clock) VALUES (?1, ?2, '', ?3)",
                )?
                .execute((T::NAME, &row_key, clock))?;
        }
        self.changes.push(Change {
            table: T::NAME,
            key: row_key.into(),
            field: None,
        });
        Ok(())
    }

    /// Sets one column of the row with this key.
    pub fn set<T: Table, V>(
        &mut self,
        column: Column<T, V>,
        key: impl Key<T::Key>,
        value: &impl Fits<V>,
    ) -> Result<(), Error> {
        self.update_fields::<T>(&key.values(), &[(column.name(), value)])
    }

    /// Sets several columns of the row with this key in one statement, so
    /// CHECKs that tie them together see them change at once.
    pub fn update<T: Table>(
        &mut self,
        _table: T,
        key: impl Key<T::Key>,
        values: &[Assignment<T>],
    ) -> Result<(), Error> {
        let fields: Vec<_> = values
            .iter()
            .map(|assignment| (assignment.column, assignment.value))
            .collect();
        self.update_fields::<T>(&key.values(), &fields)
    }

    /// Sets a column that may be NULL to NULL.
    pub fn clear<T: Table, V>(
        &mut self,
        column: Column<T, Nullable<V>>,
        key: impl Key<T::Key>,
    ) -> Result<(), Error> {
        self.update_fields::<T>(&key.values(), &[(column.name(), &Null)])
    }

    /// Marks the row deleted.
    pub fn delete<T: SoftDelete>(&mut self, _table: T, key: impl Key<T::Key>) -> Result<(), Error> {
        let now = self.now;
        self.update_fields::<T>(&key.values(), &[("deleted_at", &now)])
    }

    /// Saves `bytes` as `files/ab/abcd…`, named by their BLAKE3 hash, and adds
    /// their `files` row, unless both are there already. Returns the hash.
    pub fn add_file(&mut self, bytes: &[u8], media_type: &str) -> Result<[u8; 32], Error> {
        let hash = blake3::hash(bytes);
        if let Some(files) = self.files {
            let hex = hash.to_hex();
            let directory = files.join(&hex[..2]);
            let path = directory.join(hex.as_str());
            if !path.exists() {
                // Renamed into place, so a crash never leaves a partial file
                // under the hash. A file saved by a command that then fails
                // stays without a row, which costs space but nothing else.
                let partial = directory.join(format!("{hex}.partial"));
                fs::create_dir_all(&directory)
                    .and_then(|()| fs::write(&partial, bytes))
                    .and_then(|()| fs::rename(&partial, &path))
                    .map_err(|error| Error::File(path.clone(), error))?;
            }
        }
        let hash = *hash.as_bytes();
        let exists = self
            .prepare("SELECT 1 FROM files WHERE hash = ?1")?
            .exists([hash])?;
        if !exists {
            self.insert(
                Files,
                hash,
                &[
                    files::MEDIA_TYPE.value(&media_type),
                    files::SIZE.value(&(bytes.len() as i64)),
                ],
            )?;
        }
        Ok(hash)
    }

    /// Deletes every row, for resetting development data. Not a sync change:
    /// the outbox is emptied too.
    #[cfg(any(debug_assertions, test))]
    pub fn erase_everything(&mut self) -> Result<(), Error> {
        // Checked at commit, so the order tables empty in doesn't matter.
        self.transaction
            .pragma_update(None, "defer_foreign_keys", true)?;
        for table in super::schema::TABLES.iter().chain(&["outbox"]) {
            self.transaction
                .execute(&format!("DELETE FROM \"{table}\""), [])?;
        }
        self.transaction
            .execute("INSERT INTO settings (singleton) VALUES (1)", [])?;
        // An empty key matches no row: views that show any of a table hear
        // of it through `Changed::touches`.
        self.changes
            .extend(super::schema::TABLES.iter().map(|&table| Change {
                table,
                key: Box::default(),
                field: None,
            }));
        Ok(())
    }

    /// Prepares a read inside the command's transaction, so it sees what the
    /// command has written so far. Refuses a statement that writes: those go
    /// through `insert`, `set`, `update`, `clear` and `delete`.
    pub fn prepare(&self, sql: &str) -> Result<CachedStatement<'_>, Error> {
        let statement = self.transaction.prepare_cached(sql)?;
        if !statement.readonly() {
            return Err(Error::Misuse(format!(
                "writes go through insert, set, update, clear or delete: {sql}"
            )));
        }
        Ok(statement)
    }

    /// The connection `T` is in, inside the command's transaction there.
    fn connection_of<T: Table>(&mut self) -> Result<&Connection, Error> {
        if !T::DEVICE {
            return Ok(&self.transaction);
        }
        let transaction = match self.device_transaction.take() {
            Some(transaction) => transaction,
            None => Transaction::new_unchecked(self.device, TransactionBehavior::Immediate)?,
        };
        Ok(self.device_transaction.insert(transaction))
    }

    fn update_fields<T: Table>(
        &mut self,
        key: &[&dyn ToSql],
        fields: &[(&'static str, &dyn ToSql)],
    ) -> Result<(), Error> {
        if fields.is_empty() {
            return Ok(());
        }
        let row_key = row_key(key)?;
        let assignments = fields
            .iter()
            .zip(1..)
            .map(|((field, _), index)| format!("\"{field}\" = ?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let condition = T::KEY
            .iter()
            .zip(fields.len() + 1..)
            .map(|(column, index)| format!("\"{column}\" = ?{index}"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut parameters: Vec<&dyn ToSql> = fields.iter().map(|(_, value)| *value).collect();
        parameters.extend_from_slice(key);
        let updated = self
            .connection_of::<T>()?
            .prepare_cached(&format!(
                "UPDATE \"{}\" SET {assignments} WHERE {condition}",
                T::NAME
            ))?
            .execute(parameters.as_slice())?;
        if updated == 0 {
            return Err(Error::RowNotFound { table: T::NAME });
        }
        for (field, _) in fields {
            self.note_field_change::<T>(&row_key, field)?;
        }
        Ok(())
    }

    fn note_field_change<T: Table>(
        &mut self,
        row_key: &[u8],
        field: &'static str,
    ) -> Result<(), Error> {
        if let Some(clock) = self.clock.filter(|_| !T::DEVICE) {
            // Entries hold no values: the sender reads them from the row. So
            // a field needs one entry, with its latest clock, and none while
            // the row is still unsent: field '' sends it whole, edits included.
            // Either way the clock moves, so a sender that read the row before
            // this edit keeps the entry for the next sync.
            let unsent = self
                .transaction
                .prepare_cached(
                    "UPDATE outbox SET clock = ?3 WHERE table_name = ?1 AND row_key = ?2 AND field = ''",
                )?
                .execute((T::NAME, row_key, clock))?;
            if unsent == 0 {
                self.transaction
                    .prepare_cached(
                        "INSERT INTO outbox (table_name, row_key, field, clock) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (table_name, row_key, field) DO UPDATE SET clock = excluded.clock",
                    )?
                    .execute((T::NAME, row_key, field, clock))?;
            }
        }
        self.changes.push(Change {
            table: T::NAME,
            key: row_key.into(),
            field: Some(field),
        });
        Ok(())
    }
}

/// Hybrid logical clock: ms since the Unix epoch shifted up 16 bits, plus a
/// counter for changes within the same ms or while the wall clock is behind
/// the last value. Never goes back.
#[derive(Debug)]
struct Clock {
    last: i64,
}

impl Clock {
    /// Resumes from the larger of the saved clock and the newest change in
    /// the outbox: a crash can commit the outbox but not the saved clock.
    fn recover(kron: &Connection, device: &Connection) -> Result<Self, Error> {
        let saved: i64 = device.query_row("SELECT clock FROM device", [], |row| row.get(0))?;
        let outbox: Option<i64> =
            kron.query_row("SELECT max(clock) FROM outbox", [], |row| row.get(0))?;
        Ok(Self {
            last: saved.max(outbox.unwrap_or(0)),
        })
    }

    fn next(&mut self, now: i64) -> i64 {
        self.last = (now << 16).max(self.last + 1);
        self.last
    }
}

/// The key's values joined, as `outbox.row_key` holds them: blobs (IDs,
/// hashes) and text as their bytes, an integer (`settings`' 1) as 8 bytes
/// big-endian. Unambiguous as long as only a key's last column varies in
/// length, as the text in `template_files` and `template_kinds` keys does.
pub(super) fn row_key(values: &[&dyn ToSql]) -> Result<Vec<u8>, Error> {
    let mut key = Vec::with_capacity(16 * values.len());
    for value in values {
        match value.to_sql()? {
            ToSqlOutput::Borrowed(ValueRef::Blob(bytes) | ValueRef::Text(bytes)) => {
                key.extend_from_slice(bytes)
            }
            ToSqlOutput::Owned(Value::Blob(bytes)) => key.extend_from_slice(&bytes),
            ToSqlOutput::Owned(Value::Text(text)) => key.extend_from_slice(text.as_bytes()),
            ToSqlOutput::Borrowed(ValueRef::Integer(integer))
            | ToSqlOutput::Owned(Value::Integer(integer)) => {
                key.extend_from_slice(&integer.to_be_bytes())
            }
            other => {
                return Err(Error::Misuse(format!(
                    "a key is blobs, text or an integer, not {other:?}"
                )));
            }
        }
    }
    Ok(key)
}

fn now() -> i64 {
    // A clock before 1970 is a broken clock; 0 still keeps the hybrid clock
    // moving forward on its counter.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use rusqlite::OptionalExtension;
    use uuid::Uuid;

    use super::*;
    use crate::database::schema::{
        FactOverrides, Facts, Jobs, device_settings, fact_overrides, facts, jobs, settings,
    };

    fn turn_sync_on(writer: &Writer) -> Result<(), Error> {
        writer
            .device()
            .execute("UPDATE device SET pulled_until = 0", [])?;
        Ok(())
    }

    fn add_fact(write: &mut Write, id: Uuid, text: &str) -> Result<(), Error> {
        write.insert(
            Facts,
            id,
            &[
                facts::KIND.value(&"bullet"),
                facts::TEXT.value(&text),
                facts::EXTRACTED.value(&text),
            ],
        )
    }

    fn add_job(write: &mut Write, id: Uuid) -> Result<(), Error> {
        write.insert(
            Jobs,
            id,
            &[
                jobs::TITLE.value(&"Staff engineer"),
                jobs::COMPANY.value(&"Acme"),
                jobs::CREATED_AT.value(&0),
                jobs::BOARD_SORT_KEY.value(&"a"),
            ],
        )
    }

    /// The outbox as (table, field, clock), oldest first.
    fn outbox(writer: &Writer) -> Result<Vec<(String, String, i64)>, Error> {
        let rows = writer
            .kron()
            .prepare("SELECT table_name, field, clock FROM outbox ORDER BY clock, field")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn fact_text(writer: &Writer, id: Uuid) -> Result<Option<String>, Error> {
        Ok(writer
            .kron()
            .query_row("SELECT text FROM facts WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?)
    }

    #[test]
    fn without_sync_nothing_goes_in_the_outbox() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = Uuid::now_v7();
        let ((), changes) = writer.execute(|write| {
            add_fact(write, id, "Led the Rust rewrite")?;
            write.set(facts::TEXT, id, &"Led the Go rewrite")
        })?;
        assert_eq!(
            fact_text(&writer, id)?.as_deref(),
            Some("Led the Go rewrite")
        );
        assert!(outbox(&writer)?.is_empty());
        let fields: Vec<_> = changes.iter().map(|change| change.field).collect();
        assert_eq!(fields, [None, Some("text")]);
        assert!(changes.iter().all(|change| &*change.key == id.as_bytes()));
        Ok(())
    }

    #[test]
    fn a_new_row_goes_up_whole_and_its_edits_need_no_entry() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        turn_sync_on(&writer)?;
        let id = Uuid::now_v7();
        writer.execute(|write| add_fact(write, id, "Led the Rust rewrite"))?;
        let inserted = outbox(&writer)?;
        writer.execute(|write| write.set(facts::TEXT, id, &"Led the Go rewrite"))?;
        writer.execute(|write| write.delete(Facts, id))?;
        let entries = outbox(&writer)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(
            (entries[0].0.as_str(), entries[0].1.as_str()),
            ("facts", "")
        );
        // A sender that read the row before the edits must keep the entry.
        assert!(entries[0].2 > inserted[0].2);
        Ok(())
    }

    #[test]
    fn edits_to_a_synced_row_keep_one_entry_per_field_with_the_latest_clock() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = Uuid::now_v7();
        // Written before sync was on, so the server has it from the upload.
        writer.execute(|write| add_fact(write, id, "Led the Rust rewrite"))?;
        turn_sync_on(&writer)?;
        writer.execute(|write| write.set(facts::TEXT, id, &"Led the Go rewrite"))?;
        let first = outbox(&writer)?;
        writer.execute(|write| write.set(facts::TEXT, id, &"Led the Zig rewrite"))?;
        writer.execute(|write| write.set(facts::HIDDEN, id, &true))?;
        let entries = outbox(&writer)?;
        let fields: Vec<_> = entries.iter().map(|(_, field, _)| field.as_str()).collect();
        assert_eq!(fields, ["text", "hidden"]);
        assert!(entries[0].2 > first[0].2);
        Ok(())
    }

    #[test]
    fn delete_marks_the_row_and_syncs_the_mark() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = Uuid::now_v7();
        writer.execute(|write| add_fact(write, id, "Led the Rust rewrite"))?;
        turn_sync_on(&writer)?;
        let (now, _) = writer.execute(|write| {
            write.delete(Facts, id)?;
            Ok(write.now())
        })?;
        let deleted_at: Option<i64> =
            writer
                .kron()
                .query_row("SELECT deleted_at FROM facts WHERE id = ?1", [id], |row| {
                    row.get(0)
                })?;
        assert_eq!(deleted_at, Some(now));
        let fields: Vec<_> = outbox(&writer)?
            .into_iter()
            .map(|(_, field, _)| field)
            .collect();
        assert_eq!(fields, ["deleted_at"]);
        Ok(())
    }

    #[test]
    fn a_device_setting_stays_on_the_device() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        turn_sync_on(&writer)?;
        let ((), changes) =
            writer.execute(|write| write.set(device_settings::SIDEBAR_COLLAPSED, 1, &true))?;
        let collapsed: bool = writer.device().query_row(
            "SELECT sidebar_collapsed FROM device_settings",
            [],
            |row| row.get(0),
        )?;
        assert!(collapsed);
        assert!(outbox(&writer)?.is_empty());
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].table, "device_settings");
        Ok(())
    }

    #[test]
    fn a_failed_command_keeps_none_of_its_device_writes() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let failed = writer.execute(|write| {
            write.set(device_settings::SIDEBAR_COLLAPSED, 1, &true)?;
            write.set(facts::TEXT, Uuid::now_v7(), &"No such fact")
        });
        assert!(matches!(failed, Err(Error::RowNotFound { .. })));
        let collapsed: bool = writer.device().query_row(
            "SELECT sidebar_collapsed FROM device_settings",
            [],
            |row| row.get(0),
        )?;
        assert!(!collapsed);
        Ok(())
    }

    #[test]
    fn update_changes_columns_a_check_ties_together_at_once() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        turn_sync_on(&writer)?;
        let (job, fact) = (Uuid::now_v7(), Uuid::now_v7());
        writer.execute(|write| {
            add_job(write, job)?;
            add_fact(write, fact, "Led the Rust rewrite")?;
            write.insert(
                FactOverrides,
                (job, fact),
                &[fact_overrides::INCLUSION.value(&Some("exclude"))],
            )
        })?;
        // One column at a time, the row would briefly have neither, which
        // its CHECK refuses.
        let ((), changes) = writer.execute(|write| {
            write.update(
                FactOverrides,
                (job, fact),
                &[
                    fact_overrides::INCLUSION.value(&None::<&str>),
                    fact_overrides::TEXT.value(&Some("Led the Go rewrite")),
                ],
            )
        })?;
        let fields: Vec<_> = changes.iter().map(|change| change.field).collect();
        assert_eq!(fields, [Some("inclusion"), Some("text")]);
        Ok(())
    }

    #[test]
    fn nullable_columns_take_options_and_clear() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = Uuid::now_v7();
        writer.execute(|write| add_fact(write, id, "Led the Rust rewrite"))?;
        let note = |writer: &Writer| -> rusqlite::Result<Option<String>> {
            writer
                .kron()
                .query_row("SELECT review_note FROM facts WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
        };
        writer.execute(|write| write.set(facts::REVIEW_NOTE, id, &Some("hyphenated")))?;
        assert_eq!(note(&writer)?.as_deref(), Some("hyphenated"));
        writer.execute(|write| write.clear(facts::REVIEW_NOTE, id))?;
        assert_eq!(note(&writer)?, None);
        Ok(())
    }

    #[test]
    fn a_failed_command_keeps_nothing() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        turn_sync_on(&writer)?;
        let id = Uuid::now_v7();
        let result = writer.execute(|write| {
            add_fact(write, id, "Led the Rust rewrite")?;
            write.set(facts::TEXT, Uuid::now_v7(), &"no such fact")
        });
        assert!(matches!(result, Err(Error::RowNotFound { table: "facts" })));
        assert_eq!(fact_text(&writer, id)?, None);
        assert!(outbox(&writer)?.is_empty());
        Ok(())
    }

    #[test]
    fn two_column_and_singleton_keys() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        turn_sync_on(&writer)?;
        let (job, fact) = (Uuid::now_v7(), Uuid::now_v7());
        let ((), changes) = writer.execute(|write| {
            add_job(write, job)?;
            add_fact(write, fact, "Led the Rust rewrite")?;
            write.insert(
                FactOverrides,
                (job, fact),
                &[fact_overrides::INCLUSION.value(&Some("exclude"))],
            )?;
            write.set(settings::APPEARANCE, 1, &"dark")
        })?;
        let mut joined = job.as_bytes().to_vec();
        joined.extend_from_slice(fact.as_bytes());
        assert_eq!(&*changes[2].key, joined.as_slice());
        assert_eq!(&*changes[3].key, 1i64.to_be_bytes().as_slice());
        assert_eq!(changes[3].table, "settings");
        let appearance: String =
            writer
                .kron()
                .query_row("SELECT appearance FROM settings", [], |row| row.get(0))?;
        assert_eq!(appearance, "dark");
        Ok(())
    }

    #[test]
    fn a_key_ending_in_text_joins_its_bytes() -> Result<(), Error> {
        let id = Uuid::now_v7();
        let mut expected = id.as_bytes().to_vec();
        expected.extend_from_slice(b"main.typ");
        assert_eq!(row_key(&[&id, &"main.typ"])?, expected);
        Ok(())
    }

    #[test]
    fn prepare_refuses_writes() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        writer.execute(|write| {
            for sql in ["UPDATE facts SET text = ''", "DELETE FROM outbox"] {
                assert!(matches!(write.prepare(sql), Err(Error::Misuse(_))));
            }
            Ok(())
        })?;
        Ok(())
    }

    #[test]
    fn reads_see_the_command_s_own_writes() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let (count, _) = writer.execute(|write| {
            add_fact(write, Uuid::now_v7(), "Led the Rust rewrite")?;
            let count: i64 = write
                .prepare("SELECT count(*) FROM facts")?
                .query_row([], |row| row.get(0))?;
            Ok(count)
        })?;
        assert_eq!(count, 1);
        Ok(())
    }

    #[test]
    fn the_clock_never_goes_back() {
        let mut clock = Clock { last: 0 };
        let first = clock.next(1_000);
        let same_ms = clock.next(1_000);
        let wall_clock_went_back = clock.next(500);
        assert_eq!(first, 1_000 << 16);
        assert_eq!(same_ms, first + 1);
        assert_eq!(wall_clock_went_back, first + 2);
        assert_eq!(clock.next(2_000), 2_000 << 16);
    }

    #[test]
    fn the_clock_is_saved_and_resumes_past_the_outbox() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        turn_sync_on(&writer)?;
        writer.execute(|write| add_fact(write, Uuid::now_v7(), "Led the Rust rewrite"))?;
        let saved: i64 = writer
            .device()
            .query_row("SELECT clock FROM device", [], |row| row.get(0))?;
        assert_eq!(saved, outbox(&writer)?[0].2);

        // A crash after the records committed but before the clock was saved.
        writer.device().execute("UPDATE device SET clock = 0", [])?;
        let clock = Clock::recover(writer.kron(), writer.device())?;
        assert_eq!(clock.last, saved);
        Ok(())
    }

    #[test]
    fn search_follows_facts_written_through_write() -> Result<(), Error> {
        let mut writer = Writer::open_in_memory()?;
        let id = Uuid::now_v7();
        writer.execute(|write| add_fact(write, id, "Led the Rust rewrite"))?;
        writer.execute(|write| write.set(facts::TEXT, id, &"Led the Go rewrite"))?;
        let matches = |term: &str| -> rusqlite::Result<i64> {
            writer.kron().query_row(
                "SELECT count(*) FROM facts_search WHERE facts_search MATCH ?1",
                [term],
                |row| row.get(0),
            )
        };
        assert_eq!(matches("rust")?, 0);
        assert_eq!(matches("go")?, 1);
        Ok(())
    }
}

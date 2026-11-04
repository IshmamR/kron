// The made-up name sent to AI models in place of the user's, in Settings.
// Saved on Save or Enter, not as typed, so a name that's in the user's facts
// can be refused with why.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};

use crate::database::{Changed, Database, schema};
use crate::model::settings::{self, Refused};

const FIELD_WIDTH: gpui_kit::Pixels = px(380.);

pub struct StandInName {
    database: Entity<Database>,
    name: Entity<InputState>,
    /// As saved.
    stored: SharedString,
    status: Status,
    task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Idle,
    Saved,
    Failed(SharedString),
}

impl Render for StandInName {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let typed = self.name.read(cx).value();
        let unchanged = typed.trim().is_empty() || typed.trim() == self.stored.as_ref();
        v_flex()
            .w(FIELD_WIDTH)
            .max_w_full()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.name).small()))
                    .child(
                        Button::new("save-stand-in")
                            .label("Save")
                            .primary()
                            .small()
                            .disabled(unchanged)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
            .map(|this| match &self.status {
                Status::Idle => this,
                Status::Saved => this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Saved."),
                ),
                Status::Failed(message) => this.child(
                    div()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(message.clone()),
                ),
            })
    }
}

impl StandInName {
    pub fn new(database: Entity<Database>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let stored: SharedString = settings::stand_in_name(database.read(cx).connection())
            .unwrap_or_else(|error| {
                eprintln!("stand-in name: couldn't read it: {error}");
                String::new()
            })
            .into();
        let name = cx.new(|cx| InputState::new(window, cx).default_value(stored.clone()));
        let subscriptions = vec![
            cx.subscribe_in(&name, window, |this, _, event, _, cx| match event {
                InputEvent::PressEnter { .. } => this.save(cx),
                InputEvent::Change => {
                    this.status = Status::Idle;
                    cx.notify();
                }
                _ => {}
            }),
            // Read back, so a change synced from another device shows,
            // unless the user is writing another.
            cx.subscribe_in(
                &database,
                window,
                |this, _, changed: &Changed, window, cx| {
                    if changed.touches(schema::Settings) {
                        this.reload(window, cx);
                    }
                },
            ),
        ];
        Self {
            database,
            name,
            stored,
            status: Status::Idle,
            task: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_owned();
        if name == self.stored.as_ref() {
            return;
        }
        let saved = self
            .database
            .read(cx)
            .write(cx, move |write| settings::set_stand_in_name(write, &name));
        self.task = cx.spawn(async move |this, cx| {
            let status = match saved.await {
                Ok(None) => Status::Saved,
                Ok(Some(Refused::Empty)) => {
                    Status::Failed("Write a name to send in place of yours.".into())
                }
                Ok(Some(Refused::InFacts)) => Status::Failed(
                    "That name is in your facts, so yours couldn't be put back. Pick another."
                        .into(),
                ),
                Err(error) => Status::Failed(format!("Couldn't save it: {error}.").into()),
            };
            if let Err(error) = this.update(cx, |this, cx| {
                this.status = status;
                cx.notify();
            }) {
                eprintln!("stand-in name: {error}");
            }
        });
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stored: SharedString =
            match settings::stand_in_name(self.database.read(cx).connection()) {
                Ok(stored) => stored.into(),
                Err(error) => {
                    eprintln!("stand-in name: couldn't read it: {error}");
                    return;
                }
            };
        let typed = self.name.read(cx).value();
        if typed.trim() == stored.as_ref() || typed == self.stored {
            self.name
                .update(cx, |name, cx| name.set_value(stored.clone(), window, cx));
        }
        self.stored = stored;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::model::commands;
    use crate::workspace::testing;

    fn type_and_save(
        field: &Entity<StandInName>,
        database: &Entity<Database>,
        name: &str,
        cx: &mut VisualTestContext,
    ) {
        let input = field.read_with(cx, |field, _| field.name.clone());
        input.update_in(cx, |input, window, cx| input.set_value(name, window, cx));
        field.update(cx, |field, cx| field.save(cx));
        testing::settle(cx, database);
    }

    fn shown(field: &Entity<StandInName>, cx: &mut VisualTestContext) -> String {
        field.read_with(cx, |field, cx| field.name.read(cx).value().to_string())
    }

    #[test]
    fn a_name_in_the_facts_is_refused_and_another_saved() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_empty(&mut app);
        let (field, cx) = app.add_window_view({
            let database = database.clone();
            |window, cx| StandInName::new(database, window, cx)
        });
        assert_eq!(shown(&field, cx), "Jordan Ellis");
        testing::run(cx, &database, |write| {
            commands::add_note(write, "Mentored Sam Okafor.").map(|_| ())
        });

        type_and_save(&field, &database, "Sam Okafor", cx);
        assert!(
            matches!(
                field.read_with(cx, |field, _| field.status.clone()),
                Status::Failed(_)
            ),
            "says why it wasn't saved"
        );
        type_and_save(&field, &database, "Riley Novak ", cx);
        let stored = database.read_with(cx, |database, _| {
            settings::stand_in_name(database.connection())
        });
        assert_eq!(stored.ok().as_deref(), Some("Riley Novak"));
        assert_eq!(shown(&field, cx), "Riley Novak");

        // As a sync would bring it.
        testing::run(cx, &database, |write| {
            settings::set_stand_in_name(write, "Ana Lima").map(|_| ())
        });
        assert_eq!(
            shown(&field, cx),
            "Ana Lima",
            "a change made elsewhere shows"
        );
    }
}

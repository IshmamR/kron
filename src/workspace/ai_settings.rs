// The API key for the AI provider, in Settings. It's saved in the OS keyring
// and never shown again: the field only says whether one is saved.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::{
    App, AppContext, Context, Div, Entity, IntoElement, SharedString, Subscription, Task, Window,
    div, prelude::*, px,
};

use super::parts::text_field;
use crate::database::Database;
use crate::llm::{self, Purpose};

/// Wide enough for a key to show it's all there.
const FIELD_WIDTH: gpui_kit::Pixels = px(380.);

pub struct AiSettings {
    database: Entity<Database>,
    key: Entity<InputState>,
    status: KeyStatus,
    editing: bool,
    /// Reading, saving or removing the key; replaced, the one before stops.
    task: Task<()>,
    _subscription: Subscription,
}

impl Render for AiSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let working = matches!(self.status, KeyStatus::Checking | KeyStatus::Working);
        let (status, color) = match &self.status {
            KeyStatus::Checking => ("Checking the keyring…".into(), theme.muted_foreground),
            KeyStatus::Working => ("Saving…".into(), theme.muted_foreground),
            KeyStatus::Saved => (
                SharedString::from("A key is saved in your system keyring."),
                theme.muted_foreground,
            ),
            KeyStatus::Missing => (
                SharedString::from("No key saved. Get one at platform.deepseek.com."),
                theme.muted_foreground,
            ),
            KeyStatus::Failed(message) => (message.clone(), theme.danger),
        };
        h_flex()
            .flex_wrap()
            .justify_end()
            .gap_3()
            .child(div().text_xs().text_color(color).child(status))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("add-api-key")
                            .label(if self.status == KeyStatus::Saved {
                                "Replace API key…"
                            } else {
                                "Add API key…"
                            })
                            .outline()
                            .small()
                            .disabled(working)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_editor(window, cx)),
                            ),
                    )
                    .when(self.status == KeyStatus::Saved, |this| {
                        this.child(
                            Button::new("remove-key")
                                .label("Remove")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
                        )
                    }),
            )
    }
}

impl AiSettings {
    pub fn new(database: Entity<Database>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste your API key")
        });
        let subscription =
            cx.subscribe_in(&key, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.save(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            });
        let mut settings = Self {
            database,
            key,
            status: KeyStatus::Checking,
            editing: false,
            task: Task::ready(()),
            _subscription: subscription,
        };
        settings.check(cx);
        settings
    }

    fn open_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = true;
        let settings = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let content = settings.clone();
            let closed = settings.clone();
            dialog
                .title("API key")
                .w(FIELD_WIDTH + px(48.))
                .on_close(move |_, window, cx| {
                    closed.update(cx, |this, cx| this.close_editor(window, cx))
                })
                .content(move |dialog, _, cx| {
                    dialog.child(content.update(cx, |this, cx| this.render_editor(cx)))
                })
        });
        self.key.update(cx, |key, cx| key.focus(window, cx));
        cx.notify();
    }

    fn render_editor(&mut self, cx: &mut Context<Self>) -> Div {
        let working = matches!(self.status, KeyStatus::Checking | KeyStatus::Working);
        let empty = self.key.read(cx).value().trim().is_empty();
        let failed = match &self.status {
            KeyStatus::Failed(message) => Some(message.clone()),
            _ => None,
        };
        v_flex()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Saved in your system keyring. The key is never shown again."),
            )
            .child(text_field(
                Input::new(&self.key).mask_toggle(),
                &self.key,
                cx,
            ))
            .when_some(failed, |this, message| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(message))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-api-key")
                            .label("Cancel")
                            .outline()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_editor(window, cx);
                                window.close_dialog(cx);
                            })),
                    )
                    .child(
                        Button::new("save-key")
                            .label(if working { "Saving…" } else { "Save" })
                            .primary()
                            .small()
                            .disabled(working || empty)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }

    /// Forgets what was typed, so the key isn't kept in memory.
    fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = false;
        self.key.update(cx, |key, cx| key.set_value("", window, cx));
        cx.notify();
    }

    fn provider(&self, cx: &App) -> Option<llm::Provider> {
        llm::assigned(
            self.database.read(cx).device_connection(),
            Purpose::ExtractFacts,
        )
        .unwrap_or_else(|error| {
            eprintln!("ai settings: couldn't read the provider: {error}");
            None
        })
    }

    /// Looks in the keyring for the provider's key.
    fn check(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.provider(cx) else {
            self.status = KeyStatus::Missing;
            return;
        };
        self.status = KeyStatus::Checking;
        let found = cx.background_spawn(async move { llm::key(provider.id) });
        self.task = cx.spawn(async move |this, cx| {
            let status = match found.await {
                Ok(Some(_)) => KeyStatus::Saved,
                Ok(None) => KeyStatus::Missing,
                Err(error) => {
                    KeyStatus::Failed(format!("Couldn't read the keyring: {error}.").into())
                }
            };
            if let Err(error) = this.update(cx, |this, cx| {
                this.status = status;
                cx.notify();
            }) {
                eprintln!("ai settings: {error}");
            }
        });
    }

    /// Saves the key typed, making its provider the default if none is.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.key.read(cx).value().trim().to_owned();
        if key.is_empty() || matches!(self.status, KeyStatus::Working) {
            return;
        }
        let (vendor, model) = match self.provider(cx) {
            Some(provider) => (provider.vendor, provider.model),
            None => (llm::VENDORS[0].0.to_owned(), llm::DEFAULT_MODEL.to_owned()),
        };
        self.status = KeyStatus::Working;
        let provider = self
            .database
            .read(cx)
            .write(cx, move |write| llm::set_default(write, &vendor, &model));
        self.task = cx.spawn_in(window, async move |this, cx| {
            let saved = match provider.await {
                Ok(provider) => cx
                    .background_spawn(async move { llm::set_key(provider, Some(&key)) })
                    .await
                    .map_err(|error| format!("Couldn't save to the keyring: {error}.")),
                Err(error) => Err(format!("Couldn't save the provider: {error}.")),
            };
            let updated = this.update_in(cx, |this, window, cx| {
                match saved {
                    Ok(()) => {
                        this.status = KeyStatus::Saved;
                        this.key.update(cx, |key, cx| key.set_value("", window, cx));
                        if this.editing {
                            this.editing = false;
                            window.defer(cx, |window, cx| window.close_dialog(cx));
                        }
                    }
                    Err(message) => this.status = KeyStatus::Failed(message.into()),
                }
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("ai settings: {error}");
            }
        });
        cx.notify();
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.provider(cx) else {
            return;
        };
        self.status = KeyStatus::Working;
        let removed = cx.background_spawn(async move { llm::set_key(provider.id, None) });
        self.task = cx.spawn(async move |this, cx| {
            let status = match removed.await {
                Ok(()) => KeyStatus::Missing,
                Err(error) => KeyStatus::Failed(
                    format!("Couldn't remove it from the keyring: {error}.").into(),
                ),
            };
            if let Err(error) = this.update(cx, |this, cx| {
                this.status = status;
                cx.notify();
            }) {
                eprintln!("ai settings: {error}");
            }
        });
        cx.notify();
    }
}

#[derive(Clone, Debug, PartialEq)]
enum KeyStatus {
    Checking,
    Saved,
    Missing,
    Working,
    Failed(SharedString),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::testing;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{TestAppContext, VisualTestContext};

    #[test]
    fn cancelling_the_key_dialog_discards_the_draft() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_empty(&mut app);
        let (handle, settings) = app
            .update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| AiSettings::new(database, window, cx))
                })
            })
            .unwrap_or_else(|error| panic!("couldn't open key settings: {error}"));
        let mut cx = VisualTestContext::from_window(handle, &app);
        cx.run_until_parked();
        cx.update(|window, cx| window.click("add-api-key", cx));
        cx.run_until_parked();
        cx.update(|window, _| assert!(window.find("save-key").visible()));
        cx.simulate_input("sk-not-a-real-key");
        cx.run_until_parked();
        assert_eq!(
            settings.read_with(&cx, |settings, cx| settings.key.read(cx).value()),
            "sk-not-a-real-key"
        );
        cx.update(|window, cx| window.click("cancel-api-key", cx));
        cx.run_until_parked();
        assert!(settings.read_with(&cx, |settings, cx| settings.key.read(cx).value().is_empty()));
        cx.update(|window, cx| window.click("add-api-key", cx));
        cx.run_until_parked();
        assert!(settings.read_with(&cx, |settings, cx| settings.key.read(cx).value().is_empty()));
        cx.simulate_input("sk-another-draft");
        assert_eq!(
            settings.read_with(&cx, |settings, cx| settings.key.read(cx).value()),
            "sk-another-draft"
        );
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(settings.read_with(&cx, |settings, cx| settings.key.read(cx).value().is_empty()));
    }
}

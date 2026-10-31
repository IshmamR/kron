// The API key for the AI provider, in Settings. It's saved in the OS keyring
// and never shown again: the field only says whether one is saved.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::{
    App, AppContext, Context, Entity, IntoElement, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};

use crate::database::Database;
use crate::llm::{self, Purpose};

/// Wide enough for a key to show it's all there.
const FIELD_WIDTH: gpui_kit::Pixels = px(380.);

pub struct AiSettings {
    database: Entity<Database>,
    key: Entity<InputState>,
    status: KeyStatus,
    /// Reading, saving or removing the key; replaced, the one before stops.
    task: Task<()>,
    _subscription: Subscription,
}

#[derive(Clone, Debug, PartialEq)]
enum KeyStatus {
    Checking,
    Saved,
    Missing,
    Working,
    Failed(SharedString),
}

impl Render for AiSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let working = matches!(self.status, KeyStatus::Checking | KeyStatus::Working);
        let empty = self.key.read(cx).value().trim().is_empty();
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
        v_flex()
            .w(FIELD_WIDTH)
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.key).small().mask_toggle()),
                    )
                    .child(
                        Button::new("save-key")
                            .label("Save")
                            .primary()
                            .small()
                            .disabled(working || empty)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
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
            .child(div().text_xs().text_color(color).child(status))
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
            task: Task::ready(()),
            _subscription: subscription,
        };
        settings.check(cx);
        settings
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

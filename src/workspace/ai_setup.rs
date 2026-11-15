// Asks for an AI provider on import, when none is set up, before the text is
// checked: one provider recommended, how to get its key, and what it costs.
// The others are in Settings, which it opens.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, IntoElement,
    SharedString, Subscription, Task, Window, div, prelude::*, px,
};

use super::SetUpAi;
use super::ai_settings::save_key;
use super::parts::{caption, notice, text_field};
use crate::database::schema::LlmAssignments;
use crate::database::{Changed, Database};
use crate::llm::{self, Purpose, Vendor};

/// Recommended to a new user: the name they know from ChatGPT, and its API
/// isn't used for training unless the user opts in.
const RECOMMENDED: &str = "openai";

pub struct AiSetup {
    database: Entity<Database>,
    key: Entity<InputState>,
    /// Why the key couldn't be saved.
    problem: Option<SharedString>,
    saving: Task<()>,
    working: bool,
    _subscriptions: Vec<Subscription>,
}

pub enum AiSetupEvent {
    /// A provider is set up, here or in Settings.
    Ready,
    /// The user goes on without one; facts are found later.
    Skip,
}

impl EventEmitter<AiSetupEvent> for AiSetup {}

impl Focusable for AiSetup {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.key.focus_handle(cx)
    }
}

impl Render for AiSetup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(vendor) = llm::vendor(RECOMMENDED) else {
            return div().into_any_element();
        };
        let empty = self.key.read(cx).value().trim().is_empty();
        v_flex()
            .id("ai-setup")
            .size_full()
            .overflow_y_scrollbar()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Choose an AI to read your CV"),
                    )
                    .child(caption(
                        "Next you'll choose what to hide. The rest goes to an AI model, which \
                         finds the facts of your work in it. Kron uses your own key, so you pay \
                         the provider only for what you use.",
                        cx,
                    )),
            )
            .child(self.render_recommended(vendor, empty, cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("other-provider")
                            .label("Use another provider")
                            .ghost()
                            .small()
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(SetUpAi), cx)
                            }),
                    )
                    .child(
                        Button::new("skip-ai")
                            .label("Skip, find the facts later")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(AiSetupEvent::Skip))),
                    ),
            )
            .into_any_element()
    }
}

impl AiSetup {
    pub fn new(database: Entity<Database>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste your API key")
        });
        let subscriptions = vec![
            cx.subscribe_in(&key, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.save(window, cx),
                InputEvent::Change => cx.notify(),
                InputEvent::Focus | InputEvent::Blur => {}
            }),
            // Set up in Settings instead.
            cx.subscribe(&database, |this, database, changed: &Changed, cx| {
                if changed.touches(LlmAssignments) && !this.working {
                    match llm::assigned(
                        database.read(cx).device_connection(),
                        Purpose::ExtractFacts,
                    ) {
                        Ok(Some(_)) => cx.emit(AiSetupEvent::Ready),
                        Ok(None) => {}
                        Err(error) => eprintln!("ai setup: couldn't read the AI provider: {error}"),
                    }
                }
            }),
        ];
        Self {
            database,
            key,
            problem: None,
            saving: Task::ready(()),
            working: false,
            _subscriptions: subscriptions,
        }
    }

    /// Adds the recommended provider with the key typed, which runs every
    /// task on its default model.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.key.read(cx).value().trim().to_owned();
        let Some(vendor) = llm::vendor(RECOMMENDED) else {
            return;
        };
        if key.is_empty() || self.working {
            return;
        }
        self.working = true;
        self.problem = None;
        let database = self.database.clone();
        let added = database.read(cx).write(cx, move |write| {
            llm::add_provider(write, vendor.name, vendor.id, None, vendor.model)
        });
        self.saving = cx.spawn_in(window, async move |this, cx| {
            let saved = async {
                let provider = added
                    .await
                    .map_err(|error| format!("Couldn't save the provider: {error}."))?;
                save_key(provider, key, true, &database, cx).await
            }
            .await;
            let updated = this.update(cx, |this, cx| {
                this.working = false;
                match saved {
                    Ok(()) => cx.emit(AiSetupEvent::Ready),
                    Err(problem) => this.problem = Some(problem.into()),
                }
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("ai setup: {error}");
            }
        });
        cx.notify();
    }

    fn render_recommended(
        &self,
        vendor: &'static Vendor,
        empty: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let step = |number: &str, text: String| {
            h_flex()
                .gap_2()
                .items_start()
                .text_sm()
                .child(
                    div()
                        .w(px(18.))
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(number.to_owned()),
                )
                .child(div().flex_1().min_w_0().child(text))
        };
        v_flex()
            .p_4()
            .gap_3()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(vendor.name))
                    .child(caption("Recommended", cx)),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(step(
                        "1.",
                        format!(
                            "Sign up at {}. It's separate from a ChatGPT plan.",
                            vendor.keys_at
                        ),
                    ))
                    .child(step(
                        "2.",
                        "Add credit under Billing; $5 is the least it takes. Reading a CV or \
                         writing a résumé costs a few cents."
                            .to_owned(),
                    ))
                    .child(step(
                        "3.",
                        "Create a key under API keys, and paste it here.".to_owned(),
                    )),
            )
            .children(
                self.problem
                    .clone()
                    .map(|problem| notice(problem, theme.danger)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        text_field(Input::new(&self.key), &self.key, cx)
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        Button::new("save-key")
                            .label("Save key")
                            .primary()
                            .small()
                            .loading(self.working)
                            .disabled(empty || self.working)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
            .child(caption("The key is kept in this computer's keyring.", cx))
    }
}

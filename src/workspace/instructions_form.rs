// What the user asks of a résumé before it's written, like "lead with the
// open-source work". It's optional, and starts from what the job's last
// résumé was asked, so regenerating keeps it.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, IntoElement, SharedString, Window, div,
    prelude::*,
};

use super::parts::caption;

/// The longest instructions may be, in characters, as the database keeps them.
const INSTRUCTIONS_LENGTH: usize = 2048;

pub enum InstructionsFormEvent {
    /// Write the résumé, with these instructions if any.
    Generate(Option<String>),
    Cancelled,
}

pub struct InstructionsForm {
    instructions: Entity<TextareaState>,
    /// Why it can't be written yet.
    problem: Option<SharedString>,
}

impl EventEmitter<InstructionsFormEvent> for InstructionsForm {}

impl Render for InstructionsForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .gap_3()
            .child(caption(
                "Anything to steer it, like what to lead with or leave out. It can change \
                 what's shown and in what order, never make a line say more than your facts.",
                cx,
            ))
            .child(Textarea::new(&self.instructions))
            .children(
                self.problem
                    .clone()
                    .map(|problem| div().text_sm().text_color(theme.danger).child(problem)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-generate")
                            .label("Cancel")
                            .small()
                            .on_click(
                                cx.listener(|_, _, _, cx| {
                                    cx.emit(InstructionsFormEvent::Cancelled)
                                }),
                            ),
                    )
                    .child(
                        Button::new("confirm-generate")
                            .label("Generate")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.generate(cx))),
                    ),
            )
    }
}

impl InstructionsForm {
    /// Starts from `last`, the instructions the job's last résumé was
    /// written with.
    pub fn new(last: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instructions = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 10)
                .placeholder("Lead with the open-source work. Keep it to one page.")
                .default_value(last.unwrap_or_default())
        });
        Self {
            instructions,
            problem: None,
        }
    }

    /// Puts the cursor in the box. Once the dialog it's in has opened: the
    /// dialog takes focus as it opens.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.instructions
            .update(cx, |instructions, cx| instructions.focus(window, cx));
    }

    fn generate(&mut self, cx: &mut Context<Self>) {
        let instructions = self.instructions.read(cx).value().trim().to_owned();
        if instructions.chars().count() > INSTRUCTIONS_LENGTH {
            self.problem = Some(
                format!("That's longer than Kron keeps, {INSTRUCTIONS_LENGTH} characters.").into(),
            );
            cx.notify();
            return;
        }
        cx.emit(InstructionsFormEvent::Generate(
            Some(instructions).filter(|instructions| !instructions.is_empty()),
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::TestAppContext;

    use super::*;

    #[test]
    fn blank_instructions_are_none_and_too_long_ones_are_refused() {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let (form, cx) = app.add_window_view(|window, cx| {
            InstructionsForm::new(Some("Keep it short.".into()), window, cx)
        });
        let sent: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
        cx.update(|_, cx| {
            let sent = sent.clone();
            cx.subscribe(&form, move |_, event: &InstructionsFormEvent, _| {
                if let InstructionsFormEvent::Generate(instructions) = event {
                    sent.borrow_mut().push(instructions.clone());
                }
            })
            .detach();
        });
        let generate_with = |text: &str, cx: &mut gpui_kit::VisualTestContext| {
            let textarea = form.read_with(cx, |form, _| form.instructions.clone());
            textarea.update_in(cx, |textarea, window, cx| {
                textarea.set_value(text, window, cx)
            });
            form.update(cx, |form, cx| form.generate(cx));
        };

        assert_eq!(
            form.read_with(cx, |form, cx| form
                .instructions
                .read(cx)
                .value()
                .to_string()),
            "Keep it short.",
            "it starts from the last résumé's"
        );
        generate_with("  \n ", cx);
        generate_with(" Lead with the talks.\n", cx);
        generate_with(&"x".repeat(INSTRUCTIONS_LENGTH + 1), cx);
        assert_eq!(
            *sent.borrow(),
            [None, Some("Lead with the talks.".to_owned())]
        );
        assert!(
            form.read_with(cx, |form, _| form.problem.is_some()),
            "says why"
        );
    }
}

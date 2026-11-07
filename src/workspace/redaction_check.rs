// What an AI model would be sent of a document: its text with the user's
// name, usernames and contact details hidden. Each one hidden is
// highlighted; a click sends it as written, another hides it again. A word
// or phrase it missed, like a colleague's name, can be hidden too. Nothing
// here is stored: the text is redacted from what's shown when it's sent.

use std::collections::HashSet;
use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle,
    InteractiveText, IntoElement, KeyBinding, SharedString, StyledText, Subscription,
    UnderlineStyle, Window, actions, div, prelude::*, px,
};

use super::parts::{caption, text_field};
use crate::model::redaction::{self, Found, Identifier, Redaction};

const KEY_CONTEXT: &str = "RedactionCheck";

actions!(redaction_check, [Confirm]);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "secondary-enter",
        Confirm,
        Some(KEY_CONTEXT),
    )]);
}

pub struct RedactionCheck {
    text: SharedString,
    names: Vec<String>,
    /// The made-up name the user's is sent as.
    stand_in: String,
    /// Words and phrases the user added to hide.
    hidden: Vec<String>,
    found: Vec<Found>,
    /// What the user chose to send as written, by where it is in the text.
    kept: HashSet<Range<usize>>,
    /// A word or phrase to hide.
    also: Entity<InputState>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

pub enum RedactionCheckEvent {
    Confirmed,
    Dismissed,
}

impl EventEmitter<RedactionCheckEvent> for RedactionCheck {}

impl Focusable for RedactionCheck {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RedactionCheck {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let hidden_style = HighlightStyle {
            color: Some(theme.foreground),
            background_color: Some(theme.warning.opacity(0.3)),
            ..Default::default()
        };
        let kept_style = HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(theme.muted_foreground),
                wavy: false,
            }),
            ..Default::default()
        };
        let ranges: Vec<Range<usize>> =
            self.found.iter().map(|found| found.range.clone()).collect();
        let highlights = self.found.iter().map(|found| {
            let style = if self.kept.contains(&found.range) {
                kept_style
            } else {
                hidden_style
            };
            (found.range.clone(), style)
        });
        let name = redaction::name_sent_as(&self.text, &self.stand_in);
        let tips: Vec<(Range<usize>, SharedString)> = self
            .found
            .iter()
            .map(|found| {
                let sent_as = match found.identifier {
                    Identifier::Name => name,
                    identifier => identifier.placeholder(),
                };
                let tip = if self.kept.contains(&found.range) {
                    format!("Sent as written. Click to send it as {sent_as}.")
                } else {
                    format!("Sent as {sent_as}. Click to send it as written.")
                };
                (found.range.clone(), tip.into())
            })
            .collect();
        let this = cx.entity().downgrade();
        let text = InteractiveText::new(
            "text",
            StyledText::new(self.text.clone()).with_highlights(highlights),
        )
        .on_click(ranges, move |index, _window, cx| {
            if let Err(error) = this.update(cx, |this, cx| this.toggle(index, cx)) {
                eprintln!("redaction check: {error}");
            }
        })
        .tooltip(move |index, window, cx| {
            let (_, tip) = tips.iter().find(|(range, _)| range.contains(&index))?;
            Some(Tooltip::new(tip.clone()).build(window, cx))
        });

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|_, _: &Confirm, _, cx| cx.emit(RedactionCheckEvent::Confirmed)))
            .size_full()
            .gap_4()
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Privacy review"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("not-now")
                                    .label("Not now")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(RedactionCheckEvent::Dismissed)
                                    })),
                            )
                            .child(
                                Button::new("confirm")
                                    .label("Confirm")
                                    .primary()
                                    .small()
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(RedactionCheckEvent::Confirmed)
                                    })),
                            ),
                    ),
            )
            .child(caption(
                "Before your text is sent to an AI model to find your facts, what's highlighted \
                 is hidden. Click one to send it as written; click again to hide it.",
                cx,
            ))
            .child(
                div()
                    .p_3()
                    .rounded_lg()
                    .bg(theme.list_active)
                    .text_sm()
                    .child(self.summary()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(text_field(Input::new(&self.also).small(), &self.also, cx).flex_1())
                    .child(
                        Button::new("hide")
                            .label("Hide")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.hide(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("text-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_5()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .text_sm()
                    .child(text),
            )
    }
}

impl RedactionCheck {
    /// Checks `text`, hiding the names in `names`, as the user's Name facts
    /// and the document give them, behind `stand_in`.
    pub fn new(
        text: SharedString,
        names: Vec<String>,
        stand_in: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let also = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Also hide a word or name, like a colleague's")
        });
        let subscription = cx.subscribe_in(&also, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.hide(window, cx);
            }
        });
        let mut check = Self {
            text,
            names,
            stand_in,
            hidden: Vec::new(),
            found: Vec::new(),
            kept: HashSet::new(),
            also,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        };
        check.find();
        check
    }

    /// The text checked, as written.
    pub fn text(&self) -> SharedString {
        self.text.clone()
    }

    /// The text as it would be sent.
    pub fn redaction(&self) -> Redaction {
        let hide: Vec<Found> = self
            .found
            .iter()
            .filter(|found| !self.kept.contains(&found.range))
            .cloned()
            .collect();
        Redaction::new(&self.text, &hide, &self.stand_in)
    }

    fn find(&mut self) {
        let names: Vec<&str> = self.names.iter().map(String::as_str).collect();
        let hidden: Vec<&str> = self.hidden.iter().map(String::as_str).collect();
        self.found = redaction::find(&self.text, &names, &hidden);
    }

    /// Sends the `index`th found as written, or hides it again.
    fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(found) = self.found.get(index) else {
            return;
        };
        if !self.kept.remove(&found.range) {
            self.kept.insert(found.range.clone());
        }
        cx.notify();
    }

    /// Hides the phrase typed, wherever it's written.
    fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let phrase = self.also.read(cx).value().trim().to_owned();
        if phrase.is_empty() {
            return;
        }
        if !self.hidden.contains(&phrase) {
            self.hidden.push(phrase);
            self.find();
            // Added to hide, so not sent as written where it was kept.
            for found in &self.found {
                if found.identifier == Identifier::Hidden {
                    self.kept.remove(&found.range);
                }
            }
        }
        self.also
            .update(cx, |also, cx| also.set_value("", window, cx));
        cx.notify();
    }

    /// How many of each kind are hidden: "Hidden: 1 name, 2 emails".
    fn summary(&self) -> String {
        let kinds = [
            (Identifier::Name, "name", "names"),
            (Identifier::Handle, "username", "usernames"),
            (Identifier::Email, "email", "emails"),
            (Identifier::Phone, "phone number", "phone numbers"),
            (Identifier::Link, "link", "links"),
            (Identifier::Hidden, "word you added", "words you added"),
        ];
        let counts: Vec<String> = kinds
            .iter()
            .filter_map(|&(identifier, one, many)| {
                let count = self
                    .found
                    .iter()
                    .filter(|found| {
                        found.identifier == identifier && !self.kept.contains(&found.range)
                    })
                    .count();
                match count {
                    0 => None,
                    1 => Some(format!("1 {one}")),
                    _ => Some(format!("{count} {many}")),
                }
            })
            .collect();
        if counts.is_empty() {
            "Nothing is hidden.".to_owned()
        } else {
            format!("Hidden: {}.", counts.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;

    const TEXT: &str = "Alex Rahman\nalex@example.com · github.com/alexr\nLed Kafka at Northwind.";

    fn open(app: &mut TestAppContext) -> (Entity<RedactionCheck>, &mut VisualTestContext) {
        app.update(|cx| {
            gpui_kit::init(cx);
            init(cx);
        });
        app.add_window_view(|window, cx| {
            RedactionCheck::new(
                TEXT.into(),
                vec!["Alex Rahman".to_owned()],
                "Jordan Ellis".to_owned(),
                window,
                cx,
            )
        })
    }

    fn sent(check: &Entity<RedactionCheck>, cx: &mut VisualTestContext) -> String {
        check.read_with(cx, |check, _| check.redaction().text)
    }

    #[test]
    fn the_name_and_contacts_are_hidden_until_kept() {
        let mut app = TestAppContext::single();
        let (check, cx) = open(&mut app);
        assert_eq!(
            sent(&check, cx),
            "Jordan Ellis\n[EMAIL_1] · [LINK_1]\nLed Kafka at Northwind."
        );
        check.update(cx, |check, cx| check.toggle(1, cx));
        assert_eq!(
            sent(&check, cx),
            "Jordan Ellis\nalex@example.com · [LINK_1]\nLed Kafka at Northwind."
        );
        assert_eq!(
            check.read_with(cx, |check, _| check.summary()),
            "Hidden: 1 name, 1 link."
        );
        check.update(cx, |check, cx| check.toggle(1, cx));
        assert_eq!(
            sent(&check, cx),
            "Jordan Ellis\n[EMAIL_1] · [LINK_1]\nLed Kafka at Northwind.",
            "a second click hides it again"
        );
    }

    #[test]
    fn a_phrase_typed_is_hidden_everywhere() {
        let mut app = TestAppContext::single();
        let (check, cx) = open(&mut app);
        let also = check.read_with(cx, |check, _| check.also.clone());
        also.update_in(cx, |also, window, cx| {
            also.set_value("northwind", window, cx)
        });
        check.update_in(cx, |check, window, cx| check.hide(window, cx));
        assert_eq!(
            sent(&check, cx),
            "Jordan Ellis\n[EMAIL_1] · [LINK_1]\nLed Kafka at [HIDDEN_1]."
        );
        assert_eq!(also.read_with(cx, |also, _| also.value().to_string()), "");

        check.update(cx, |check, cx| check.toggle(1, cx));
        also.update_in(cx, |also, window, cx| {
            also.set_value("alex@example.com", window, cx)
        });
        check.update_in(cx, |check, window, cx| check.hide(window, cx));
        assert_eq!(
            sent(&check, cx),
            "Jordan Ellis\n[HIDDEN_1] · [LINK_1]\nLed Kafka at [HIDDEN_2].",
            "what's kept is hidden once the user asks to hide it"
        );
    }

    #[test]
    fn ctrl_enter_confirms() {
        let mut app = TestAppContext::single();
        let (check, cx) = open(&mut app);
        let confirmed = std::rc::Rc::new(std::cell::Cell::new(false));
        cx.update(|window, cx| {
            let confirmed = confirmed.clone();
            cx.subscribe(&check, move |_, event, _| {
                if let RedactionCheckEvent::Confirmed = event {
                    confirmed.set(true);
                }
            })
            .detach();
            let also = check.read(cx).also.clone();
            also.update(cx, |also, cx| also.focus(window, cx));
        });
        cx.simulate_keystrokes("ctrl-enter");
        assert!(confirmed.get(), "from the box to hide more, too");
    }
}

use gpui_kit::base::actions::{SelectDown, SelectUp};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, App, AsKeystroke, FontWeight, IntoElement, Keystroke, Window, div,
    prelude::*, px,
};

use super::ToggleSidebar;
use super::facts_page::{
    AddFacts, CancelEdit, ConfirmFact, Dismiss, FocusList, FocusSearch, ToggleHidden, UndoMerge,
};
use super::application_view::{CancelEvent, SaveEvent, UndoDelete};
use super::job_view::{
    Back, SelectNextLine, SelectPreviousLine, ShowApplicationTab, ShowResumeTab,
};
use super::resume_grid::{OpenSelected, SelectLeft, SelectRight, ShowFilter};
use super::{ShowShortcuts, resume_grid};

/// Two columns, so every shortcut fits without scrolling on most screens.
const WIDTH: f32 = 900.;
/// Room left above and below the dialog.
const VERTICAL_MARGIN: f32 = 200.;
/// The kit list's key context, where its ↑ ↓ bindings live.
const LIST_CONTEXT: &str = "List";
/// ⌘ on macOS, Ctrl elsewhere: what `secondary-` bindings use.
const SECONDARY_CLICK: &str = if cfg!(target_os = "macos") {
    "⌘-click"
} else {
    "Ctrl+click"
};

/// Opens the list of keyboard shortcuts. Keys are read from the keymap, so
/// the list shows whatever is actually bound.
pub fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, _| {
        dialog
            .title("Keyboard shortcuts")
            .w(px(WIDTH))
            .content(|content, window, cx| {
                content.child(
                    div()
                        .id("shortcuts")
                        .max_h(window.viewport_size().height - px(VERTICAL_MARGIN))
                        .overflow_y_scrollbar()
                        .child(
                            h_flex()
                                .items_start()
                                .gap_8()
                                .pr_3()
                                .children(columns(groups()).into_iter().map(|column| {
                                    v_flex().flex_1().min_w_0().gap_5().children(
                                        column.into_iter().map(|group| group.render(window, cx)),
                                    )
                                })),
                        ),
                )
            })
    });
}

/// Splits the groups, in order, into two columns of about the same height.
fn columns(groups: Vec<Group>) -> [Vec<Group>; 2] {
    // A group's title takes about a row.
    let rows = |group: &Group| group.shortcuts.len() + 1;
    let half = groups.iter().map(rows).sum::<usize>() / 2;
    let mut columns = [Vec::new(), Vec::new()];
    let mut filled = 0;
    for group in groups {
        let column = usize::from(filled >= half);
        filled += rows(&group);
        columns[column].push(group);
    }
    columns
}

struct Group {
    title: &'static str,
    shortcuts: Vec<Shortcut>,
}

struct Shortcut {
    description: &'static str,
    keys: Vec<Keys>,
}

/// One way to trigger a shortcut; a shortcut can have a few, like ↑ ↓.
enum Keys {
    Action(Box<dyn Action>),
    /// An action components share, like the kit's list movement, as bound in
    /// one key context.
    ActionIn(Box<dyn Action>, &'static str),
    /// Keys handled by a component rather than one of our actions.
    Keystroke(&'static str),
    /// A mouse gesture.
    Gesture(&'static str),
}

fn groups() -> Vec<Group> {
    use Keys::{Action, ActionIn, Gesture, Keystroke};
    let shortcut = |description, keys| Shortcut { description, keys };
    vec![
        Group {
            title: "General",
            shortcuts: vec![
                shortcut("Show or hide the sidebar", vec![Action(Box::new(ToggleSidebar))]),
                shortcut("Show keyboard shortcuts", vec![Action(Box::new(ShowShortcuts))]),
            ],
        },
        Group {
            title: "Résumés",
            shortcuts: vec![
                shortcut(
                    "Move between résumés",
                    vec![
                        Action(Box::new(SelectLeft)),
                        Action(Box::new(SelectRight)),
                        Action(Box::new(resume_grid::SelectUp)),
                        Action(Box::new(resume_grid::SelectDown)),
                    ],
                ),
                shortcut(
                    "Open the selected résumé",
                    vec![Action(Box::new(OpenSelected)), Gesture("Double-click")],
                ),
                shortcut(
                    "Switch the filter",
                    vec![
                        Action(Box::new(ShowFilter { index: 0 })),
                        Gesture("to"),
                        Action(Box::new(ShowFilter { index: 4 })),
                    ],
                ),
            ],
        },
        Group {
            title: "Résumé",
            shortcuts: vec![
                shortcut(
                    "Move between lines",
                    vec![
                        Action(Box::new(SelectPreviousLine)),
                        Action(Box::new(SelectNextLine)),
                    ],
                ),
                shortcut(
                    "Clear the selection, then go back",
                    vec![Action(Box::new(Back))],
                ),
                shortcut("Show the résumé", vec![Action(Box::new(ShowResumeTab))]),
                shortcut("Show the application", vec![Action(Box::new(ShowApplicationTab))]),
            ],
        },
        Group {
            title: "Application",
            shortcuts: vec![
                shortcut("Save the event", vec![Action(Box::new(SaveEvent))]),
                shortcut("Cancel logging", vec![Action(Box::new(CancelEvent))]),
                shortcut("Undo deleting an event", vec![Action(Box::new(UndoDelete))]),
            ],
        },
        Group {
            title: "Facts",
            shortcuts: vec![
                shortcut(
                    "Move between facts",
                    vec![
                        ActionIn(Box::new(SelectUp), LIST_CONTEXT),
                        ActionIn(Box::new(SelectDown), LIST_CONTEXT),
                    ],
                ),
                shortcut(
                    "Confirm, and go to the next fact to review",
                    vec![Action(Box::new(ConfirmFact))],
                ),
                shortcut("Hide or show the fact", vec![Action(Box::new(ToggleHidden))]),
                shortcut("Search facts", vec![Action(Box::new(FocusSearch))]),
                shortcut(
                    "Go from the search to the results",
                    vec![Action(Box::new(FocusList))],
                ),
                shortcut("Mark facts to merge", vec![Gesture(SECONDARY_CLICK)]),
                shortcut(
                    "Pick the wording to keep, while merging",
                    vec![Keystroke("1"), Gesture("to"), Keystroke("9")],
                ),
                shortcut("Merge, while merging", vec![Action(Box::new(ConfirmFact))]),
                shortcut("Clear merge marks", vec![Action(Box::new(Dismiss))]),
                shortcut("Undo the merge", vec![Action(Box::new(UndoMerge))]),
                shortcut("Save the fact's text, while editing it", vec![Keystroke("enter")]),
                shortcut("Add a line break, while editing", vec![Keystroke("shift-enter")]),
                shortcut("Cancel editing", vec![Action(Box::new(CancelEdit))]),
                shortcut("Add the facts you wrote", vec![Action(Box::new(AddFacts))]),
                shortcut("Close the add form", vec![Action(Box::new(Dismiss))]),
            ],
        },
    ]
}

impl Group {
    fn render(self, window: &Window, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .gap_1()
            .child(
                div()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(self.title.to_uppercase()),
            )
            .children(self.shortcuts.into_iter().map(|shortcut| {
                h_flex()
                    .justify_between()
                    .gap_4()
                    .py_1()
                    .text_sm()
                    .child(shortcut.description)
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .children(shortcut.keys.iter().map(|keys| keys.render(window, cx))),
                    )
            }))
    }
}

/// The keys bound to `action`, most preferred first.
pub fn keystrokes_for(action: &dyn Action, cx: &App) -> Vec<Keystroke> {
    let keystrokes: Vec<Keystroke> = cx
        .key_bindings()
        .borrow()
        .bindings_for_action(action)
        // Later bindings take precedence.
        .rev()
        .filter_map(|binding| binding.keystrokes().first())
        .map(|keystroke| keystroke.as_keystroke().clone())
        .collect();
    let mut unique = Vec::new();
    for keystroke in keystrokes {
        if !unique.contains(&keystroke) {
            unique.push(keystroke);
        }
    }
    unique
}

impl Keys {
    fn render(&self, window: &Window, cx: &App) -> AnyElement {
        let keystrokes = match self {
            Keys::Action(action) => keystrokes_for(action.as_ref(), cx),
            Keys::ActionIn(action, context) => {
                return match Kbd::binding_for_action(action.as_ref(), Some(context), window) {
                    Some(kbd) => kbd.into_any_element(),
                    None => unbound(cx),
                };
            }
            Keys::Keystroke(keystroke) => Keystroke::parse(keystroke).ok().into_iter().collect(),
            Keys::Gesture(gesture) => {
                return div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(*gesture)
                    .into_any_element();
            }
        };
        if keystrokes.is_empty() {
            return unbound(cx);
        }
        h_flex()
            .gap_1()
            .children(keystrokes.into_iter().map(Kbd::new))
            .into_any_element()
    }
}

/// A listed action nothing is bound to: a bug, so make it visible.
fn unbound(cx: &App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().danger)
        .child("Unbound")
        .into_any_element()
}

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::list::{List, ListEvent, ListState};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::resizable::{ResizableState, h_resizable, resizable_panel};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, App, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyBinding, Keystroke, Pixels, ScrollStrategy, SharedString, Subscription, Window, actions, div, prelude::*, px,
};

use super::OpenLine;
use super::fact_list::{FactList, Filter};
use super::parts::{
    caption, card, fact_kind_label, fact_status, heading, notice, source_excerpt, source_label,
};
use crate::database::Database;
use crate::model::commands::{self, Merge};
use crate::model::{Fact, FactId, Library};

const KEY_CONTEXT: &str = "FactsPage";
/// Keys meant for the list itself, so they don't fire while typing.
const LIST_KEY_CONTEXT: &str = "FactsPage > List";
// Each text box is wrapped in its own context, so its keys can differ.
const SEARCH_CONTEXT: &str = "FactSearch";
const FACT_EDITOR_CONTEXT: &str = "FactEditor";
const NOTE_EDITOR_CONTEXT: &str = "NoteEditor";
const LIST_WIDTH: Pixels = px(480.);
const LIST_MIN_WIDTH: Pixels = px(320.);
const LIST_MAX_WIDTH: Pixels = px(760.);
/// Past this, lines of fact text get too long to read comfortably.
const DETAIL_MAX_WIDTH: Pixels = px(760.);

actions!(
    facts_page,
    [
        ConfirmFact,
        ToggleHidden,
        Dismiss,
        FocusSearch,
        FocusList,
        CancelEdit,
        AddFacts,
        UndoMerge
    ]
);

/// Picks the `index`th wording to keep while merging, by its number key.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = facts_page, no_json)]
pub struct KeepWording {
    pub index: usize,
}

/// Number keys pick a merge wording; past nine, click.
const WORDING_KEYS: usize = 9;

pub fn init(cx: &mut App) {
    cx.bind_keys((0..WORDING_KEYS).map(|index| {
        KeyBinding::new(&(index + 1).to_string(), KeepWording { index }, Some(LIST_KEY_CONTEXT))
    }));
    let input_in = |context: &str| format!("{context} > Input");
    // These replace the list's and the text boxes' own keys, which run after
    // the kit's: Enter and Esc in the list only select, and Esc in a text box
    // does nothing.
    cx.bind_keys([
        KeyBinding::new("secondary-f", FocusSearch, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", ConfirmFact, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("backspace", ToggleHidden, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("delete", ToggleHidden, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("secondary-z", UndoMerge, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("down", FocusList, Some(&input_in(SEARCH_CONTEXT))),
        KeyBinding::new("escape", FocusList, Some(&input_in(SEARCH_CONTEXT))),
        KeyBinding::new("escape", CancelEdit, Some(&input_in(FACT_EDITOR_CONTEXT))),
        KeyBinding::new("escape", Dismiss, Some(&input_in(NOTE_EDITOR_CONTEXT))),
        KeyBinding::new("secondary-enter", AddFacts, Some(&input_in(NOTE_EDITOR_CONTEXT))),
    ]);
}

/// Everything the app knows about the user, to review, correct, hide, merge
/// and add to.
pub struct FactsPage {
    library: Entity<Library>,
    database: Entity<Database>,
    list: Entity<ListState<FactList>>,
    search: Entity<InputState>,
    /// The list's width.
    layout: Entity<ResizableState>,
    selected: Option<FactId>,
    /// A fact to select once the list has it: one just added, which the
    /// library has once it's reloaded after the write.
    select_when_listed: Option<FactId>,
    adding: bool,
    /// The wording picked to keep when merging.
    keep: Option<FactId>,
    fact_editor: Entity<TextareaState>,
    /// The fact `fact_editor` holds.
    editing: Option<FactId>,
    note_editor: Entity<TextareaState>,
    /// The last merge, to undo. Undo is offered while the fact it merged
    /// into is selected and nothing else has changed.
    undo_merge: Option<Merge>,
    _subscriptions: Vec<Subscription>,
}

impl Render for FactsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_fact_editor(window, cx);
        let theme = cx.theme();

        let detail = if self.adding {
            self.render_add(cx).into_any_element()
        } else if self.marked(cx).len() > 1 {
            self.render_merge(cx)
        } else {
            match self.selected.and_then(|id| self.library.read(cx).fact(id)) {
                Some(fact) => self.render_fact(fact, cx),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(caption("Select a fact to review it.", cx))
                    .into_any_element(),
            }
        };

        div()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::confirm_fact))
            .on_action(cx.listener(Self::toggle_hidden))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search
                    .update(cx, |search, cx| search.focus(window, cx))
            }))
            .on_action(cx.listener(|this, _: &FocusList, window, cx| this.focus_list(window, cx)))
            .on_action(cx.listener(Self::cancel_edit))
            .on_action(cx.listener(|this, _: &UndoMerge, window, cx| this.undo_merge(window, cx)))
            .on_action(cx.listener(|this, action: &KeepWording, _, cx| {
                if let Some(&fact) = this.marked(cx).get(action.index) {
                    this.keep = Some(fact);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &AddFacts, window, cx| this.add_note(window, cx)))
            .size_full()
            .child(
                h_resizable("facts-page")
                    .with_state(&self.layout)
                    .child(
                        resizable_panel()
                            .size(LIST_WIDTH)
                            .size_range(LIST_MIN_WIDTH..LIST_MAX_WIDTH)
                            .child(
                                v_flex()
                                    .size_full()
                                    .child(self.render_toolbar(cx))
                                    .child(
                                        div()
                                            .flex_none()
                                            .px_2()
                                            .py_1()
                                            .border_b_1()
                                            .border_color(theme.border)
                                            .key_context(SEARCH_CONTEXT)
                                            .child(
                                                Input::new(&self.search)
                                                    .prefix(
                                                        Icon::new(IconName::Search)
                                                            .text_color(theme.muted_foreground),
                                                    )
                                                    .cleanable(true)
                                                    .appearance(false),
                                            ),
                                    )
                                    .child(List::new(&self.list).flex_1().min_h_0()),
                            ),
                    )
                    .child(
                        resizable_panel().child(
                            div()
                                .id("fact-detail")
                                .size_full()
                                .overflow_y_scrollbar()
                                .child(div().max_w(DETAIL_MAX_WIDTH).p_4().child(detail)),
                        ),
                    ),
            )
    }
}

impl Focusable for FactsPage {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.list.focus_handle(cx)
    }
}

impl FactsPage {
    pub fn new(
        library: Entity<Library>,
        database: Entity<Database>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| ListState::new(FactList::new(library.clone(), cx), window, cx));
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search facts (Ctrl+F)"));
        let fact_editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .submit_on_enter(true)
        });
        let note_editor = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(4, 14).placeholder(
                "In your own words, one thing per line. For example:\n\
                 I organized Code Fest 2019 with the University of Dhaka Programming Club.",
            )
        });

        let subscriptions = vec![
            cx.observe_in(&library, window, |this, _, window, cx| {
                this.rebuild_list(window, cx)
            }),
            cx.subscribe_in(&list, window, |this, list, event, _, cx| match event {
                ListEvent::Select(index) | ListEvent::Confirm(index) => {
                    let fact = list.read(cx).delegate().fact_at(*index);
                    if fact != this.selected {
                        // Before the editor is loaded with the new fact.
                        this.save_fact_text(cx);
                        this.selected = fact;
                    }
                    this.adding = false;
                    cx.notify();
                }
                ListEvent::Cancel => {}
            }),
            cx.observe(&list, |_, _, cx| cx.notify()),
            cx.subscribe_in(&search, window, |this, search, event, window, cx| match event {
                InputEvent::Change => {
                    let query = search.read(cx).value();
                    this.list
                        .update(cx, |list, cx| list.delegate_mut().set_query(&query, cx));
                    this.rebuild_list(window, cx);
                }
                InputEvent::PressEnter { .. } => this.focus_list(window, cx),
                InputEvent::Focus | InputEvent::Blur => {}
            }),
            cx.subscribe_in(&fact_editor, window, |this, _, event, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => {
                        this.save_fact_text(cx);
                        this.list.focus_handle(cx).focus(window, cx);
                    }
                    InputEvent::Blur => this.save_fact_text(cx),
                    InputEvent::Change | InputEvent::Focus => {}
                }
            }),
        ];

        Self {
            library,
            database,
            list,
            search,
            layout: cx.new(|_| ResizableState::default()),
            selected: None,
            select_when_listed: None,
            adding: false,
            keep: None,
            fact_editor,
            editing: None,
            note_editor,
            undo_merge: None,
            _subscriptions: subscriptions,
        }
    }

    /// Selects the fact, clearing the filter and search if they hide it.
    pub fn show_fact(&mut self, fact: FactId, window: &mut Window, cx: &mut Context<Self>) {
        self.adding = false;
        // The library may have changed elsewhere since.
        self.undo_merge = None;
        if self.list.read(cx).delegate().index_of(fact).is_none() {
            self.search
                .update(cx, |search, cx| search.set_value("", window, cx));
            self.list.update(cx, |list, cx| {
                list.delegate_mut().set_query("", cx);
                list.delegate_mut().set_filter(Filter::All, cx);
            });
        }
        self.select(fact, window, cx);
        self.list.focus_handle(cx).focus(window, cx);
    }

    fn select(&mut self, fact: FactId, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected != Some(fact) {
            self.save_fact_text(cx);
        }
        self.selected = Some(fact);
        self.list.update(cx, |list, cx| {
            let index = list.delegate().index_of(fact);
            list.set_selected_index(index, window, cx);
            if let Some(index) = index {
                list.scroll_to_item(index, ScrollStrategy::Center, window, cx);
            }
        });
        cx.notify();
    }

    /// Regroups the list. If the selected fact is no longer listed, the first
    /// listed fact is selected instead, so the details and keys follow what's
    /// on screen.
    fn rebuild_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected;
        let selected = self.list.update(cx, |list, cx| {
            list.delegate_mut().rebuild(cx);
            let delegate = list.delegate();
            let fact = selected
                .filter(|&fact| delegate.index_of(fact).is_some())
                .or_else(|| selected.and_then(|_| delegate.facts().next()));
            let index = fact.and_then(|fact| list.delegate().index_of(fact));
            list.set_selected_index(index, window, cx);
            cx.notify();
            fact
        });
        if selected != self.selected {
            self.save_fact_text(cx);
            self.selected = selected;
        }
        if let Some(fact) = self.select_when_listed
            && self.list.read(cx).delegate().index_of(fact).is_some()
        {
            self.select_when_listed = None;
            self.select(fact, window, cx);
        }
    }

    /// Moves to the list, selecting the first fact if none is.
    fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let first = self.list.read(cx).delegate().facts().next();
        if self.selected.is_none()
            && let Some(first) = first
        {
            self.select(first, window, cx);
        }
        self.list.focus_handle(cx).focus(window, cx);
    }

    fn marked(&self, cx: &App) -> Vec<FactId> {
        self.list.read(cx).delegate().marked().to_vec()
    }

    fn set_marked(&mut self, marked: Vec<FactId>, cx: &mut Context<Self>) {
        self.list.update(cx, |list, cx| {
            list.delegate_mut().set_marked(marked);
            cx.notify();
        });
    }

    fn set_filter(&mut self, filter: Filter, window: &mut Window, cx: &mut Context<Self>) {
        self.list
            .update(cx, |list, cx| list.delegate_mut().set_filter(filter, cx));
        self.rebuild_list(window, cx);
    }

    /// ↩: confirms the fact and moves on to the next one to review.
    /// ↩: confirms the fact and moves on to the next one to review, or,
    /// while merging, merges.
    fn confirm_fact(&mut self, _: &ConfirmFact, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked(cx).len() > 1 {
            self.merge(window, cx);
            return;
        }
        let Some(fact) = self.selected else {
            return;
        };
        let next = self.next_to_review(fact, cx);
        self.undo_merge = None;
        self.database
            .read(cx)
            .queue(move |write| commands::confirm_fact(write, fact));
        if let Some(next) = next {
            self.select(next, window, cx);
        }
    }

    /// ⌫: hides the fact, or shows it again.
    fn toggle_hidden(&mut self, _: &ToggleHidden, _: &mut Window, cx: &mut Context<Self>) {
        let Some(fact) = self.selected else {
            return;
        };
        let Some(hidden) = self.library.read(cx).fact(fact).map(|fact| fact.hidden) else {
            return;
        };
        self.undo_merge = None;
        self.database
            .read(cx)
            .queue(move |write| commands::set_hidden(write, fact, !hidden));
    }

    /// Esc: closes the add form, then drops merge marks.
    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.adding {
            self.close_add_form(window, cx);
        } else if !self.marked(cx).is_empty() {
            self.set_marked(Vec::new(), cx);
        }
    }

    /// Esc while editing a fact: puts its text back and leaves the editor.
    fn cancel_edit(&mut self, _: &CancelEdit, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .editing
            .and_then(|id| self.library.read(cx).fact(id))
            .map(|fact| fact.text.clone());
        if let Some(text) = text {
            self.fact_editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
        }
        self.list.focus_handle(cx).focus(window, cx);
    }

    /// Closes the add form, throwing away what was typed.
    fn close_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adding = false;
        self.note_editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        self.list.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn next_to_review(&self, after: FactId, cx: &App) -> Option<FactId> {
        let library = self.library.read(cx);
        self.list
            .read(cx)
            .delegate()
            .facts()
            .skip_while(|&id| id != after)
            .skip(1)
            .find(|&id| {
                library
                    .fact(id)
                    .is_some_and(|fact| fact.review.is_some())
            })
    }

    /// Loads the selected fact into the editor when the selection changes, or
    /// when its text changed elsewhere while the editor wasn't in use.
    fn sync_fact_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fact) = self.selected.and_then(|id| self.library.read(cx).fact(id)) else {
            return;
        };
        let (id, text) = (fact.id, fact.text.clone());
        let editor = self.fact_editor.read(cx);
        let stale = self.editing != Some(id)
            || (!editor.focus_handle(cx).is_focused(window) && editor.value() != text);
        if stale {
            self.editing = Some(id);
            self.fact_editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
        }
    }

    fn save_fact_text(&mut self, cx: &mut Context<Self>) {
        let Some(fact) = self.editing else {
            return;
        };
        let text = self.fact_editor.read(cx).value();
        let unchanged = self
            .library
            .read(cx)
            .fact(fact)
            .is_none_or(|fact| fact.text == text.trim() || text.trim().is_empty());
        if unchanged {
            return;
        }
        self.undo_merge = None;
        // Shown now, so the editor doesn't flash the old text until the
        // library is reloaded after the write.
        self.library.update(cx, |library, cx| {
            if let Some(fact) = library.facts.iter_mut().find(|each| each.id == fact) {
                fact.text = text.trim().to_string();
                cx.notify();
            }
        });
        let text = text.to_string();
        self.database
            .read(cx)
            .queue(move |write| commands::edit_fact(write, fact, &text));
    }

    fn add_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.note_editor.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.undo_merge = None;
        self.close_add_form(window, cx);
        if self.list.read(cx).delegate().filter() == Filter::Hidden {
            self.set_filter(Filter::All, window, cx);
        }
        let added = self
            .database
            .read(cx)
            .write(cx, move |write| commands::add_note(write, &text));
        cx.spawn_in(window, async move |this, cx| {
            let first = match added.await {
                Ok(added) => added.first().copied(),
                Err(error) => {
                    eprintln!("couldn't add the note: {error}");
                    None
                }
            };
            if let Some(first) = first
                && let Err(error) = this.update_in(cx, |this, window, cx| {
                    this.select_when_listed = Some(first);
                    this.rebuild_list(window, cx);
                })
            {
                eprintln!("facts page: {error}");
            }
        })
        .detach();
    }

    fn merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let marked = self.marked(cx);
        let Some(keep) = self.kept_wording(&marked, cx) else {
            return;
        };
        self.undo_merge = None;
        self.set_marked(Vec::new(), cx);
        self.keep = None;
        self.select(keep, window, cx);
        let merged = self
            .database
            .read(cx)
            .write(cx, move |write| commands::merge_facts(write, keep, &marked));
        cx.spawn(async move |this, cx| match merged.await {
            Ok(merge) => {
                if let Err(error) = this.update(cx, |this, cx| {
                    this.undo_merge = Some(merge);
                    cx.notify();
                }) {
                    eprintln!("facts page: {error}");
                }
            }
            Err(error) => eprintln!("couldn't merge the facts: {error}"),
        })
        .detach();
    }

    fn undo_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(merge) = self.undo_merge.take() else {
            return;
        };
        let keep = merge.keep;
        self.database
            .read(cx)
            .queue(move |write| commands::undo_merge(write, &merge));
        self.select(keep, window, cx);
    }

    /// The wording picked to keep, or else the first that needs no review.
    fn kept_wording(&self, marked: &[FactId], cx: &App) -> Option<FactId> {
        let library = self.library.read(cx);
        self.keep
            .filter(|keep| marked.contains(keep))
            .or_else(|| {
                marked.iter().copied().find(|&id| {
                    library
                        .fact(id)
                        .is_some_and(|fact| fact.review.is_none())
                })
            })
            .or(marked.first().copied())
    }

    fn render_toolbar(&self, cx: &Context<Self>) -> impl IntoElement {
        let library = self.library.read(cx);
        let count = |filter: Filter| {
            library
                .facts
                .iter()
                .filter(|fact| filter.includes(fact))
                .count()
        };
        let filter = self.list.read(cx).delegate().filter();
        let selected_index = Filter::ALL
            .iter()
            .position(|&each| each == filter)
            .unwrap_or_default();

        h_flex()
            .flex_none()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                TabBar::new("fact-filter")
                    .segmented()
                    .small()
                    .flex_1()
                    .selected_index(selected_index)
                    .children(Filter::ALL.map(|filter| {
                        Tab::new().label(format!("{} {}", filter.label(), count(filter)))
                    }))
                    .on_click(cx.listener(|this, index: &usize, window, cx| {
                        if let Some(&filter) = Filter::ALL.get(*index) {
                            this.set_filter(filter, window, cx);
                        }
                    })),
            )
            .child(
                Button::new("add-facts")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip("Add facts")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.save_fact_text(cx);
                        this.adding = true;
                        this.note_editor
                            .update(cx, |editor, cx| editor.focus(window, cx));
                        cx.notify();
                    })),
            )
    }

    fn render_fact(&self, fact: &Fact, cx: &Context<Self>) -> AnyElement {
        let library = self.library.read(cx);
        let theme = cx.theme();
        let id = fact.id;
        let uses = library.fact_uses(id);
        let duplicates = library.possible_duplicates(id);

        let actions = h_flex()
            .gap_2()
            .when(fact.review.is_some(), |this| {
                this.child(
                    Button::new("confirm")
                        .label("Confirm")
                        .primary()
                        .small()
                        .tooltip("Confirm and go to the next fact to review (↩)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.confirm_fact(&ConfirmFact, window, cx)
                        })),
                )
            })
            .child(
                Button::new("toggle-hidden")
                    .label(if fact.hidden {
                        "Show"
                    } else {
                        "Hide"
                    })
                    .icon(if fact.hidden {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .outline()
                    .small()
                    .tooltip("Hidden facts are never used in résumés (⌫)")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_hidden(&ToggleHidden, window, cx)
                    })),
            );

        let merged_here = self
            .undo_merge
            .as_ref()
            .is_some_and(|merge| merge.keep == id);

        v_flex()
            .gap_4()
            .when(merged_here, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .justify_between()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .bg(theme.secondary)
                        .text_sm()
                        .child(format!(
                            "Merged. This fact now has {} sources.",
                            fact.sources.len()
                        ))
                        .child(
                            Button::new("undo-merge")
                                .label("Undo")
                                .icon(IconName::Undo2)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.undo_merge(window, cx)
                                })),
                        ),
                )
            })
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(fact_kind_label(fact.kind)),
                            )
                            .when(fact.review.is_some() || fact.hidden, |this| {
                                this.child(div().text_color(theme.muted_foreground).child("·"))
                                    .child(fact_status(fact, cx))
                            })
                            .when_some(
                                fact.entry.and_then(|entry| library.entry_label(entry)),
                                |this, entry| {
                                    this.child(div().text_color(theme.muted_foreground).child("·"))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_color(theme.muted_foreground)
                                                .child(entry),
                                        )
                                },
                            ),
                    )
                    .child(
                        div()
                            .key_context(FACT_EDITOR_CONTEXT)
                            .child(Textarea::new(&self.fact_editor)),
                    )
                    .child(caption("↩ saves, ⇧↩ adds a line break, Esc cancels.", cx))
                    .when(fact.is_edited(), |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .items_start()
                                .child(
                                    div().flex_1().min_w_0().child(caption(
                                        format!("Extracted as “{}”", fact.extracted),
                                        cx,
                                    )),
                                )
                                .child(
                                    Button::new("revert")
                                        .label("Revert")
                                        .icon(IconName::Undo2)
                                        .ghost()
                                        .xsmall()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.undo_merge = None;
                                            this.database.read(cx).queue(move |write| {
                                                commands::revert_fact(write, id)
                                            });
                                        })),
                                ),
                        )
                    })
                    .when_some(fact.review.as_ref(), |this, review| {
                        this.child(notice(
                            format!(
                                "Needs review: {} Fix the text if it's wrong, then confirm.",
                                review.description()
                            ),
                            theme.warning,
                        ))
                    })
                    .when(fact.hidden, |this| {
                        this.child(notice(
                            "Hidden. Résumés generated from now on won't use it.",
                            theme.muted_foreground,
                        ))
                    })
                    .child(actions),
            )
            .child(
                v_flex()
                    .child(heading("Sources", Some(fact.sources.len()), cx))
                    .child(
                        v_flex().gap_3().children(
                            fact.sources
                                .iter()
                                .map(|source| source_excerpt(library, source, cx)),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .child(heading("Used in", Some(uses.len()), cx))
                    .when(uses.is_empty(), |this| {
                        this.child(caption("No résumé uses this fact yet.", cx))
                    })
                    .child(v_flex().gap_1().children(uses.iter().enumerate().map(
                        |(index, (job, line))| {
                            let outdated = job
                                .current_resume()
                                .is_some_and(|resume| library.fact_change(resume, id).is_some());
                            let (job_id, line_id) = (job.id, line.id);
                            card(cx)
                                .id(("use", index))
                                .gap_0p5()
                                .cursor_pointer()
                                .hover(|this| this.bg(theme.secondary_hover))
                                .on_click(move |_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(OpenLine {
                                            job: job_id,
                                            line: line_id,
                                        }),
                                        cx,
                                    )
                                })
                                .child(
                                    div()
                                        .text_sm()
                                        .child(format!("{} · {}", job.title, job.company)),
                                )
                                .child(caption(line.text.clone(), cx))
                                .when(outdated, |this| {
                                    this.child(
                                        h_flex()
                                            .gap_1()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(
                                                Icon::new(IconName::TriangleAlert)
                                                    .xsmall()
                                                    .text_color(theme.warning),
                                            )
                                            .child("Generated before this change"),
                                    )
                                })
                        },
                    ))),
            )
            .when(!duplicates.is_empty(), |this| {
                this.child(
                    v_flex()
                        .child(heading("Possible duplicates", Some(duplicates.len()), cx))
                        .child(v_flex().gap_1().children(duplicates.iter().enumerate().filter_map(
                            |(index, &other)| {
                                let other_fact = library.fact(other)?;
                                Some(
                                    card(cx)
                                        .gap_1()
                                        .child(div().text_sm().child(other_fact.text.clone()))
                                        .children(other_fact.sources.first().map(|source| {
                                            caption(source_label(library, source), cx)
                                        }))
                                        .child(
                                            h_flex().child(
                                                Button::new(("merge-with", index))
                                                    .label("Merge…")
                                                    .outline()
                                                    .xsmall()
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.keep = Some(id);
                                                            this.set_marked(vec![id, other], cx);
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                        ),
                                )
                            },
                        ))),
                )
            })
            .into_any_element()
    }

    fn render_merge(&self, cx: &Context<Self>) -> AnyElement {
        let library = self.library.read(cx);
        let theme = cx.theme();
        let marked = self.marked(cx);
        let keep = self.kept_wording(&marked, cx);
        let uses: usize = marked.iter().map(|&id| library.fact_uses(id).len()).sum();

        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Merge {} facts", marked.len())),
                    )
                    .child(caption(
                        "Pick the wording to keep. The merged fact keeps every source, and \
                         résumé lines that cite any of these will cite it.",
                        cx,
                    )),
            )
            .child(v_flex().gap_2().children(marked.iter().enumerate().filter_map(
                |(index, &id)| {
                    let fact = library.fact(id)?;
                    let sources: Vec<String> = fact
                        .sources
                        .iter()
                        .map(|source| source_label(library, source))
                        .collect();
                    let kept = keep == Some(id);
                    // On the page background, as in a form: on a card's grey
                    // an empty radio barely shows.
                    Some(
                        v_flex()
                            .id(("merge-option", index))
                            .p_2()
                            .gap_1()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(if kept {
                                theme.list_active_border
                            } else {
                                theme.border
                            })
                            .when(kept, |this| this.bg(theme.list_active))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.keep = Some(id);
                                cx.notify();
                            }))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_start()
                                    .child(Radio::new(("keep", index)).checked(kept).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.keep = Some(id);
                                            cx.notify();
                                        }),
                                    ))
                                    .children(
                                        (index < WORDING_KEYS)
                                            .then(|| Keystroke::parse(&(index + 1).to_string()).ok())
                                            .flatten()
                                            .map(Kbd::new),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(div().text_sm().child(fact.text.clone()))
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .text_xs()
                                                    .when(
                                                        fact.review.is_some() || fact.hidden,
                                                        |this| {
                                                            this.child(fact_status(fact, cx))
                                                                .child(caption("·", cx))
                                                        },
                                                    )
                                                    .child(caption(sources.join(", "), cx)),
                                            ),
                                    ),
                            ),
                    )
                },
            )))
            .child(caption(
                match uses {
                    0 => "No résumé uses these facts.".to_string(),
                    1 => "1 résumé line cites these. It stays as generated until you \
                          regenerate."
                        .to_string(),
                    _ => format!(
                        "{uses} résumé lines cite these. They stay as generated until you \
                         regenerate."
                    ),
                },
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("merge")
                            .label("Merge")
                            .primary()
                            .small()
                            .tooltip("↩")
                            .on_click(cx.listener(|this, _, window, cx| this.merge(window, cx))),
                    )
                    .child(
                        Button::new("cancel-merge")
                            .label("Cancel")
                            .ghost()
                            .small()
                            .tooltip("Esc")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_marked(Vec::new(), cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(caption(
                "Number keys pick a wording. Ctrl-click facts in the list to add or remove \
                 them. You can undo a merge right after it.",
                cx,
            ))
            .into_any_element()
    }

    fn render_add(&self, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Add facts"),
            )
            .child(
                div()
                    .key_context(NOTE_EDITOR_CONTEXT)
                    .child(Textarea::new(&self.note_editor)),
            )
            .child(caption(
                SharedString::from(
                    "Each line becomes a fact, and lines written together are grouped. The note \
                     is kept as their source. Ctrl+↩ adds them, Esc cancels.",
                ),
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("add-note")
                            .label("Add")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.add_note(window, cx))),
                    )
                    .child(
                        Button::new("cancel-add")
                            .label("Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_add_form(window, cx)
                            })),
                    ),
            )
    }
}

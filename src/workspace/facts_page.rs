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
use rusqlite::{Connection, OptionalExtension};

use super::OpenLine;
use super::fact_list::{FactList, Filter, ListedFact};
use super::parts::{
    Source, caption, card, fact_kind_label, fact_status, heading, notice, source_excerpt, sources,
};
use crate::database::schema::{
    Blocks, Documents, Entries, FactSources, FactVersions, Facts, Generations, Jobs, LineCitations,
    Lines, Sections,
};
use crate::database::{Changed, Database, Error};
use crate::model::commands;
use crate::model::{FactId, JobId, LineId, possible_duplicate};

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
        AddFacts
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
    database: Entity<Database>,
    list: Entity<ListState<FactList>>,
    search: Entity<InputState>,
    /// The list's width.
    layout: Entity<ResizableState>,
    selected: Option<FactId>,
    /// What's shown of the selected fact beyond the list's row. Read when
    /// first shown, and again after the database changes.
    detail: Option<Detail>,
    /// What's shown of the facts marked to merge, read the same way.
    merging: Option<Merging>,
    /// A fact to select once the list has it: one just added, which the
    /// list has once it's read again after the write.
    select_when_listed: Option<FactId>,
    adding: bool,
    /// The wording picked to keep when merging.
    keep: Option<FactId>,
    fact_editor: Entity<TextareaState>,
    /// The fact `fact_editor` holds.
    editing: Option<FactId>,
    note_editor: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl Render for FactsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_fact_editor(window, cx);
        let marked = self.marked(cx);
        if !self.adding && marked.len() > 1 {
            self.read_merging(marked, cx);
        } else if !self.adding {
            self.read_detail(cx);
        }
        let theme = cx.theme();

        let detail = if self.adding {
            self.render_add(cx).into_any_element()
        } else if let Some(merging) = &self.merging
            && merging.facts == self.marked(cx)
        {
            self.render_merge(merging, cx)
        } else {
            let list = self.list.read(cx).delegate();
            let fact = self.selected.and_then(|id| list.fact(id));
            match (fact, &self.detail) {
                (Some(fact), Some(detail)) if detail.fact == fact.id => {
                    self.render_fact(fact, detail, cx)
                }
                _ => div()
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
    pub fn new(database: Entity<Database>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut facts = FactList::new();
        if let Err(error) = facts.load(database.read(cx).connection()) {
            eprintln!("facts page: couldn't read the facts: {error}");
        }
        let list = cx.new(|cx| ListState::new(facts, window, cx));
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
            cx.subscribe_in(&database, window, |this, _, changed: &Changed, window, cx| {
                if shows(changed) {
                    this.reload(window, cx);
                }
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
                        .update(cx, |list, _| list.delegate_mut().set_query(&query));
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
            database,
            list,
            search,
            layout: cx.new(|_| ResizableState::default()),
            selected: None,
            detail: None,
            merging: None,
            select_when_listed: None,
            adding: false,
            keep: None,
            fact_editor,
            editing: None,
            note_editor,
            _subscriptions: subscriptions,
        }
    }

    #[cfg(test)]
    pub fn selected(&self) -> Option<FactId> {
        self.selected
    }

    /// Selects the fact, clearing the filter and search if they hide it.
    pub fn show_fact(&mut self, fact: FactId, window: &mut Window, cx: &mut Context<Self>) {
        self.adding = false;
        // The library may have changed elsewhere since.
        if self.list.read(cx).delegate().index_of(fact).is_none() {
            self.search
                .update(cx, |search, cx| search.set_value("", window, cx));
            self.list.update(cx, |list, _| {
                list.delegate_mut().set_query("");
                list.delegate_mut().set_filter(Filter::All);
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

    /// Reads the facts again, and what's shown of them, after the database
    /// changed.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let database = self.database.clone();
        self.list.update(cx, |list, cx| {
            if let Err(error) = list.delegate_mut().load(database.read(cx).connection()) {
                eprintln!("facts page: couldn't read the facts: {error}");
            }
        });
        self.detail = None;
        self.merging = None;
        self.rebuild_list(window, cx);
    }

    /// Updates the list's selection after its facts changed. If the selected
    /// fact is no longer listed, the first listed fact is selected instead, so
    /// the details and keys follow what's on screen.
    fn rebuild_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected;
        let selected = self.list.update(cx, |list, cx| {
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
            .update(cx, |list, _| list.delegate_mut().set_filter(filter));
        self.rebuild_list(window, cx);
    }

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
        let Some(hidden) = self.list.read(cx).delegate().fact(fact).map(|fact| fact.hidden) else {
            return;
        };
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
        let list = self.list.read(cx).delegate();
        let text = self
            .editing
            .and_then(|id| list.fact(id))
            .map(|fact| fact.text.clone());
        if let Some(text) = text {
            self.fact_editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
        }
        self.list.focus_handle(cx).focus(window, cx);
    }

    fn open_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_fact_text(cx);
        self.adding = true;
        self.note_editor
            .update(cx, |editor, cx| editor.focus(window, cx));
        cx.notify();
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
        let list = self.list.read(cx).delegate();
        list.facts()
            .skip_while(|&id| id != after)
            .skip(1)
            .find(|&id| list.fact(id).is_some_and(|fact| fact.review.is_some()))
    }

    /// Loads the selected fact into the editor when the selection changes, or
    /// when its text changed elsewhere while the editor wasn't in use.
    fn sync_fact_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.list.read(cx).delegate();
        let Some(fact) = self.selected.and_then(|id| list.fact(id)) else {
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
        let text = self.fact_editor.read(cx).value().trim().to_string();
        let unchanged = self
            .list
            .read(cx)
            .delegate()
            .fact(fact)
            .is_none_or(|fact| fact.text == text || text.is_empty());
        if unchanged {
            return;
        }
        self.list.update(cx, |list, cx| {
            list.delegate_mut().set_text(fact, &text);
            cx.notify();
        });
        self.database
            .read(cx)
            .queue(move |write| commands::edit_fact(write, fact, &text));
    }

    fn add_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.note_editor.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
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
                    this.reload(window, cx);
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
        self.set_marked(Vec::new(), cx);
        self.keep = None;
        self.select(keep, window, cx);
        self.database
            .read(cx)
            .queue(move |write| commands::merge_facts(write, keep, &marked));
    }

    fn revert_fact(&mut self, fact: FactId, cx: &mut Context<Self>) {
        self.database
            .read(cx)
            .queue(move |write| commands::revert_fact(write, fact));
    }

    /// Marks the fact and a possible duplicate of it to merge, keeping its
    /// wording.
    fn merge_with(&mut self, fact: FactId, other: FactId, cx: &mut Context<Self>) {
        self.keep = Some(fact);
        self.set_marked(vec![fact, other], cx);
        cx.notify();
    }

    /// The wording picked to keep, or else the first that needs no review.
    fn kept_wording(&self, marked: &[FactId], cx: &App) -> Option<FactId> {
        let list = self.list.read(cx).delegate();
        self.keep
            .filter(|keep| marked.contains(keep))
            .or_else(|| {
                marked
                    .iter()
                    .copied()
                    .find(|&id| list.fact(id).is_some_and(|fact| fact.review.is_none()))
            })
            .or(marked.first().copied())
    }

    /// Reads what's shown of the selected fact, unless it's read already.
    fn read_detail(&mut self, cx: &App) {
        let Some(fact) = self.selected else {
            return;
        };
        if self.detail.as_ref().is_some_and(|detail| detail.fact == fact) {
            return;
        }
        let connection = self.database.read(cx).connection();
        let facts = self.list.read(cx).delegate().all();
        self.detail = Some(Detail::read(connection, fact, facts).unwrap_or_else(|error| {
            eprintln!("facts page: couldn't read the fact: {error}");
            Detail::empty(fact)
        }));
    }

    /// Reads what's shown of the facts marked to merge, unless it's read
    /// already.
    fn read_merging(&mut self, marked: Vec<FactId>, cx: &App) {
        if self
            .merging
            .as_ref()
            .is_some_and(|merging| merging.facts == marked)
        {
            return;
        }
        let connection = self.database.read(cx).connection();
        self.merging = Some(Merging::read(connection, &marked).unwrap_or_else(|error| {
            eprintln!("facts page: couldn't read the facts to merge: {error}");
            Merging {
                sources: vec![Vec::new(); marked.len()],
                facts: marked,
                uses: 0,
            }
        }));
    }

    fn render_toolbar(&self, cx: &Context<Self>) -> impl IntoElement {
        let list = self.list.read(cx).delegate();
        let count = |filter: Filter| list.all().iter().filter(|fact| filter.includes(fact)).count();
        let filter = list.filter();
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
                    .on_click(cx.listener(|this, _, window, cx| this.open_add_form(window, cx))),
            )
    }

    fn render_fact(&self, fact: &ListedFact, detail: &Detail, cx: &Context<Self>) -> AnyElement {
        let list = self.list.read(cx).delegate();
        let theme = cx.theme();
        let id = fact.id;

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

        v_flex()
            .gap_4()
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
                                    .child(fact_status(fact.review.is_some(), fact.hidden, cx))
                            })
                            .when_some(
                                fact.entry.and_then(|entry| list.entry_label(entry)),
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
                    .when(fact.text != detail.extracted, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .items_start()
                                .child(
                                    div().flex_1().min_w_0().child(caption(
                                        format!("Extracted as “{}”", detail.extracted),
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
                                            this.revert_fact(id, cx)
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
                    .child(heading("Sources", Some(detail.sources.len()), cx))
                    .child(
                        v_flex().gap_3().children(
                            detail
                                .sources
                                .iter()
                                .map(|source| source_excerpt(source, cx)),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .child(heading("Used in", Some(detail.uses.len()), cx))
                    .when(detail.uses.is_empty(), |this| {
                        this.child(caption("No résumé uses this fact yet.", cx))
                    })
                    .child(v_flex().gap_1().children(detail.uses.iter().enumerate().map(
                        |(index, used)| {
                            let (job_id, line_id) = (used.job, used.line);
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
                                .child(div().text_sm().child(used.job_title.clone()))
                                .child(caption(used.text.clone(), cx))
                                .when(used.outdated, |this| {
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
            .when(!detail.duplicates.is_empty(), |this| {
                this.child(
                    v_flex()
                        .child(heading("Possible duplicates", Some(detail.duplicates.len()), cx))
                        .child(v_flex().gap_1().children(detail.duplicates.iter().enumerate().filter_map(
                            |(index, duplicate)| {
                                let other = duplicate.fact;
                                let other_fact = list.fact(other)?;
                                Some(
                                    card(cx)
                                        .gap_1()
                                        .child(div().text_sm().child(other_fact.text.clone()))
                                        .children(duplicate.source.clone().map(|source| {
                                            caption(source, cx)
                                        }))
                                        .child(
                                            h_flex().child(
                                                Button::new(("merge-with", index))
                                                    .label("Merge…")
                                                    .outline()
                                                    .xsmall()
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.merge_with(id, other, cx)
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

    fn render_merge(&self, merging: &Merging, cx: &Context<Self>) -> AnyElement {
        let list = self.list.read(cx).delegate();
        let theme = cx.theme();
        let marked = &merging.facts;
        let keep = self.kept_wording(marked, cx);
        let uses = merging.uses;

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
                    let fact = list.fact(id)?;
                    let sources = merging.sources.get(index)?;
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
                                                            this.child(fact_status(
                                                                fact.review.is_some(),
                                                                fact.hidden,
                                                                cx,
                                                            ))
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
                 them. Ctrl+Z undoes a merge.",
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

/// What the page shows of a fact beyond its row in the list.
struct Detail {
    fact: FactId,
    extracted: String,
    sources: Vec<Source>,
    /// The lines of each job's current résumé that cite it.
    uses: Vec<Use>,
    duplicates: Vec<Duplicate>,
}

impl Detail {
    /// `facts` are the list's, to look for duplicates among.
    fn read(connection: &Connection, fact: FactId, facts: &[ListedFact]) -> Result<Self, Error> {
        let extracted: Option<String> = connection
            .prepare_cached("SELECT extracted FROM facts WHERE id = ?1")?
            .query_row([fact.0], |row| row.get(0))
            .optional()?;
        let Some(extracted) = extracted else {
            return Ok(Self::empty(fact));
        };
        let mut duplicates = Vec::new();
        if let Some(listed) = facts.iter().find(|listed| listed.id == fact) {
            let duplicates_it = possible_duplicate(&listed.text);
            for other in facts.iter().filter(|other| {
                other.id != fact && other.kind == listed.kind && duplicates_it(&other.text)
            }) {
                duplicates.push(Duplicate {
                    fact: other.id,
                    source: sources(connection, other.id)?
                        .into_iter()
                        .next()
                        .map(|source| source.label),
                });
            }
        }
        Ok(Self {
            fact,
            extracted,
            sources: sources(connection, fact)?,
            uses: uses(connection, fact)?,
            duplicates,
        })
    }

    /// For a fact that couldn't be read: its row, without the rest.
    fn empty(fact: FactId) -> Self {
        Self {
            fact,
            extracted: String::new(),
            sources: Vec::new(),
            uses: Vec::new(),
            duplicates: Vec::new(),
        }
    }
}

struct Use {
    job: JobId,
    /// "Backend Engineer · Ledgerly"
    job_title: String,
    line: LineId,
    text: String,
    /// The fact changed since the résumé was generated.
    outdated: bool,
}

/// A fact that may say the same, and where it was first found.
struct Duplicate {
    fact: FactId,
    source: Option<String>,
}

/// What the page shows of the facts marked to merge beyond their rows.
struct Merging {
    facts: Vec<FactId>,
    /// Each fact's sources' labels.
    sources: Vec<Vec<String>>,
    /// Résumé lines citing any of them.
    uses: usize,
}

impl Merging {
    fn read(connection: &Connection, facts: &[FactId]) -> Result<Self, Error> {
        let mut sources_of = Vec::new();
        let mut used = 0;
        for &fact in facts {
            sources_of.push(
                sources(connection, fact)?
                    .into_iter()
                    .map(|source| source.label)
                    .collect(),
            );
            used += uses(connection, fact)?.len();
        }
        Ok(Self {
            facts: facts.to_vec(),
            sources: sources_of,
            uses: used,
        })
    }
}

/// Whether the change is to anything the page shows.
fn shows(changed: &Changed) -> bool {
    changed.touches(Facts)
        || changed.touches(FactSources)
        || changed.touches(Documents)
        || changed.touches(Entries)
        || changed.touches(Jobs)
        || changed.touches(Generations)
        || changed.touches(Sections)
        || changed.touches(Blocks)
        || changed.touches(Lines)
        || changed.touches(LineCitations)
        || changed.touches(FactVersions)
}

/// The lines of each job's current résumé that cite the fact, or a fact
/// merged into it, in the order of the board and then the page.
fn uses(connection: &Connection, fact: FactId) -> Result<Vec<Use>, Error> {
    let mut statement = connection.prepare_cached(
        "WITH RECURSIVE members(id) AS (
             SELECT ?1
             UNION
             SELECT facts.id FROM facts JOIN members ON facts.merged_into = members.id
         )
         SELECT jobs.id, jobs.title, jobs.company, lines.id, lines.text,
                EXISTS (
                    SELECT 1 FROM fact_versions
                    JOIN members ON members.id = fact_versions.fact_id
                    WHERE fact_versions.generation_id = generations.id
                      AND (fact_versions.hidden != facts.hidden
                           OR fact_versions.text != facts.text)
                )
         FROM lines
         JOIN blocks ON blocks.id = lines.block_id AND blocks.deleted_at IS NULL
         JOIN sections ON sections.id = blocks.section_id AND sections.deleted_at IS NULL
         JOIN generations ON generations.id = sections.generation_id
         JOIN jobs ON jobs.id = generations.job_id AND jobs.deleted_at IS NULL
         JOIN facts ON facts.id = ?1
         WHERE lines.deleted_at IS NULL
           AND lines.id IN (
               SELECT line_citations.line_id FROM members
               CROSS JOIN line_citations ON line_citations.fact_id = members.id
           )
           -- Its job's current résumé. Not the current_generations view, which
           -- ranks every job's generations to find each one's.
           AND generations.id = (
               SELECT current.id FROM generations AS current
               WHERE current.job_id = generations.job_id AND current.kind = 'resume'
                 AND current.deleted_at IS NULL
               ORDER BY current.generated_at DESC, current.id DESC LIMIT 1
           )
         ORDER BY jobs.board_sort_key, jobs.id, sections.sort_key, blocks.sort_key,
                  lines.sort_key",
    )?;
    let uses = statement
        .query_map([fact.0], |row| {
            let title: String = row.get(1)?;
            let company: String = row.get(2)?;
            Ok(Use {
                job: JobId(row.get(0)?),
                job_title: format!("{title} · {company}"),
                line: LineId(row.get(3)?),
                text: row.get(4)?,
                outdated: row.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(uses)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use gpui_kit::{TestAppContext, VisualTestContext};
    use rusqlite::types::FromSql;

    use super::*;
    use crate::database::schema::facts;
    use crate::database::{TemporaryDirectory, Write, Writer};
    use crate::model::{EntryId, FactKind, ResumeId, Review, ReviewReason, sample};
    use crate::workspace::testing::{self, write_sample};

    /// What the page shows of a sample fact: its entry, where it was found,
    /// and the lines that use it, with how many are outdated.
    fn shown(
        connection: &Connection,
        list: &FactList,
        start: &str,
    ) -> Result<(Option<String>, Vec<String>, usize, usize), Error> {
        let fact = sample::fact(connection, start)?;
        let entry = list
            .fact(fact)
            .and_then(|listed| listed.entry)
            .and_then(|entry| list.entry_label(entry));
        let detail = Detail::read(connection, fact, list.all())?;
        let sources = detail.sources.iter().map(|source| source.label.clone()).collect();
        let outdated = detail.uses.iter().filter(|used| used.outdated).count();
        Ok((entry, sources, detail.uses.len(), outdated))
    }

    #[test]
    fn the_list_reads_every_live_fact_as_stored() -> Result<(), Error> {
        let writer = sample::written()?;
        let connection = writer.kron();
        let mut list = FactList::new();
        list.load(connection)?;
        type Row = (FactId, FactKind, String, Option<EntryId>, bool, Option<Review>);
        let mut stored: Vec<Row> = connection
            .prepare(
                "SELECT id, kind, text, entry_id, hidden, review, review_note FROM facts
                 WHERE deleted_at IS NULL AND merged_into IS NULL",
            )?
            .query_map([], |row| {
                let reason: Option<ReviewReason> = row.get(5)?;
                Ok((
                    FactId(row.get(0)?),
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, Option<uuid::Uuid>>(3)?.map(EntryId),
                    row.get(4)?,
                    reason.map(|reason| Review {
                        reason,
                        note: row.get(6).ok().flatten(),
                    }),
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut listed: Vec<Row> = list
            .all()
            .iter()
            .map(|fact| {
                (fact.id, fact.kind, fact.text.clone(), fact.entry, fact.hidden, fact.review.clone())
            })
            .collect();
        stored.sort_by_key(|row| row.0);
        listed.sort_by_key(|row| row.0);
        assert_eq!(listed.len(), 83);
        assert_eq!(listed, stored);
        Ok(())
    }

    #[test]
    fn a_fact_shows_its_entry_sources_and_uses() -> Result<(), Error> {
        let writer = sample::written()?;
        let connection = writer.kron();
        let mut list = FactList::new();
        list.load(connection)?;
        let northwind = Some("Senior Software Engineer, Northwind Logistics".to_string());
        let cv = || "From alex_rahman_cv.pdf".to_string();
        // Found in both documents, as if duplicates had been merged.
        assert_eq!(
            shown(connection, &list, "Tracing async Rust")?,
            (None, vec![cv(), "From linkedin_profile.pdf".into()], 1, 0)
        );
        assert_eq!(shown(connection, &list, "Cut p99 latency")?, (northwind.clone(), vec![cv()], 5, 0));
        assert_eq!(shown(connection, &list, "Led the migration")?, (northwind.clone(), vec![cv()], 6, 0));
        // The likely duplicate, which no résumé uses.
        assert_eq!(
            shown(connection, &list, "Migrated 12 backend")?,
            (northwind, vec!["From linkedin_profile.pdf".into()], 0, 0)
        );
        // Written in a note, which is dated rather than named.
        let (entry, sources, uses, _) = shown(connection, &list, "Gave a guest lecture")?;
        assert_eq!((entry, uses), (None, 0));
        assert!(
            matches!(sources.as_slice(), [only] if only.starts_with("From your note, ")),
            "{sources:?}"
        );
        Ok(())
    }

    /// The facts page and each résumé agree on which facts changed since
    /// the résumé was generated from them.
    #[test]
    fn a_fact_changed_since_a_resume_shows_its_use_there_as_outdated() -> Result<(), Error> {
        let writer = sample::written()?;
        let connection = writer.kron();
        let mut list = FactList::new();
        list.load(connection)?;
        let current: Vec<(ResumeId, JobId)> = connection
            .prepare("SELECT generation_id, job_id FROM current_generations WHERE kind = 'resume'")?
            .query_map([], |row| Ok((ResumeId(row.get(0)?), JobId(row.get(1)?))))?
            .collect::<Result<_, _>>()?;
        let mut checked = 0;
        for (resume, job) in current {
            for (fact, _) in crate::model::fact_changes(connection, resume)? {
                let detail = Detail::read(connection, fact, list.all())?;
                for used in detail.uses.iter().filter(|used| used.job == job) {
                    assert!(used.outdated, "{:?}", used.text);
                    checked += 1;
                }
            }
        }
        assert!(checked > 0);
        Ok(())
    }

    #[test]
    fn the_queries_follow_a_chain_of_merges() -> Result<(), Error> {
        let mut writer = sample::written()?;
        let migration = sample::fact(writer.kron(), "Led the migration")?;
        let linkedin = sample::fact(writer.kron(), "Migrated 12 backend")?;
        let latency = sample::fact(writer.kron(), "Cut p99 latency")?;
        let used_by = |connection: &Connection, facts: [FactId; 3]| -> Result<BTreeSet<LineId>, Error> {
            let mut lines = BTreeSet::new();
            for fact in facts {
                let mut statement = connection.prepare(
                    "SELECT line_citations.line_id FROM line_citations
                     JOIN lines ON lines.id = line_citations.line_id
                     JOIN blocks ON blocks.id = lines.block_id
                     JOIN sections ON sections.id = blocks.section_id
                     JOIN current_generations
                       ON current_generations.generation_id = sections.generation_id
                     WHERE line_citations.fact_id = ?1",
                )?;
                for line in statement.query_map([fact.0], |row| Ok(LineId(row.get(0)?)))? {
                    lines.insert(line?);
                }
            }
            Ok(lines)
        };
        let expected = used_by(writer.kron(), [migration, linkedin, latency])?;
        let mut list = FactList::new();
        list.load(writer.kron())?;
        let before = list.all().len();

        // A chain: the LinkedIn bullet into the migration, which then goes
        // into the latency fact.
        writer.execute(|write| commands::merge_facts(write, migration, &[linkedin]))?;
        writer.execute(|write| commands::merge_facts(write, latency, &[migration]))?;
        writer.execute(|write| commands::edit_fact(write, latency, "Cut p99 latency by 80%."))?;

        list.load(writer.kron())?;
        assert_eq!(list.all().len(), before - 2);
        assert!(list.fact(migration).is_none() && list.fact(linkedin).is_none());
        let detail = Detail::read(writer.kron(), latency, list.all())?;
        assert_eq!(detail.sources.len(), 3, "its own and both merged into it");
        // Every line that cited any of the three cites it now, and was
        // generated before its new wording.
        let uses: BTreeSet<LineId> = detail.uses.iter().map(|used| used.line).collect();
        assert_eq!(uses, expected);
        assert!(detail.uses.iter().all(|used| used.outdated));
        Ok(())
    }

    #[test]
    fn the_queries_read_an_empty_database() -> Result<(), Error> {
        let writer = Writer::open_in_memory()?;
        let mut list = FactList::new();
        list.load(writer.kron())?;
        assert!(list.all().is_empty());
        assert_eq!(list.facts().count(), 0);
        let missing = FactId::new();
        let detail = Detail::read(writer.kron(), missing, list.all())?;
        assert!(detail.sources.is_empty() && detail.uses.is_empty());
        Ok(())
    }

    // The page in a window, driven by keys as a person would, on a database
    // on disk with the sample.

    struct Page<'a> {
        page: Entity<FactsPage>,
        database: Entity<Database>,
        cx: &'a mut VisualTestContext,
        _directory: TemporaryDirectory,
    }

    fn open(app: &mut TestAppContext) -> Page<'_> {
        let (database, directory) = testing::open_sample(app);
        let (page, cx) =
            app.add_window_view(|window, cx| FactsPage::new(database.clone(), window, cx));
        cx.update(|window, cx| page.focus_handle(cx).focus(window, cx));
        cx.run_until_parked();
        Page {
            page,
            database,
            cx,
            _directory: directory,
        }
    }

    impl Page<'_> {
        /// Lets queued commands commit, delivers what they changed, and draws.
        fn settle(&mut self) {
            testing::settle(self.cx, &self.database);
        }

        fn keys(&mut self, keystrokes: &str) {
            self.cx.simulate_keystrokes(keystrokes);
            self.settle();
        }

        /// Ctrl+Z, as the workspace handles it.
        fn undo(&mut self) {
            testing::undo(self.cx, &self.database);
        }

        /// A command from elsewhere in the app, like the inspector's.
        fn run(&mut self, command: impl FnOnce(&mut Write) -> Result<(), Error> + Send + 'static) {
            testing::run(self.cx, &self.database, command);
        }

        fn update<R>(
            &mut self,
            update: impl FnOnce(&mut FactsPage, &mut Window, &mut Context<FactsPage>) -> R,
        ) -> R {
            let result = self.page.update_in(self.cx, update);
            self.settle();
            result
        }

        fn read<R>(&mut self, read: impl FnOnce(&FactsPage, &App) -> R) -> R {
            self.page.read_with(self.cx, read)
        }

        fn fact(&mut self, start: &str) -> FactId {
            let found = self.read(|page, cx| {
                page.list
                    .read(cx)
                    .delegate()
                    .all()
                    .iter()
                    .find(|fact| fact.text.starts_with(start))
                    .map(|fact| fact.id)
            });
            match found {
                Some(fact) => fact,
                None => panic!("no fact starts with {start:?}"),
            }
        }

        fn row(&mut self, fact: FactId) -> Option<ListedFact> {
            self.read(|page, cx| page.list.read(cx).delegate().fact(fact).cloned())
        }

        fn listed(&mut self) -> Vec<FactId> {
            self.read(|page, cx| page.list.read(cx).delegate().facts().collect())
        }

        fn count(&mut self, filter: Filter) -> usize {
            self.read(|page, cx| {
                page.list
                    .read(cx)
                    .delegate()
                    .all()
                    .iter()
                    .filter(|fact| filter.includes(fact))
                    .count()
            })
        }

        fn selected(&mut self) -> Option<FactId> {
            self.read(|page, _| page.selected)
        }

        fn marked(&mut self) -> Vec<FactId> {
            self.read(|page, cx| page.marked(cx))
        }

        fn show(&mut self, fact: FactId) {
            self.update(|page, window, cx| page.show_fact(fact, window, cx));
        }

        fn filter(&mut self, filter: Filter) {
            self.update(|page, window, cx| page.set_filter(filter, window, cx));
        }

        /// Types the search, in place of what's there.
        fn search(&mut self, query: &str) {
            self.keys("ctrl-f ctrl-a backspace");
            self.cx.simulate_input(query);
            self.settle();
        }

        /// Types into the fact editor in place of its text.
        fn type_fact_text(&mut self, text: &str) {
            self.update(|page, window, cx| {
                page.fact_editor.update(cx, |editor, cx| {
                    editor.set_value("", window, cx);
                    editor.focus(window, cx);
                })
            });
            self.cx.simulate_input(text);
            self.settle();
        }

        fn editor_text(&mut self) -> String {
            self.read(|page, cx| page.fact_editor.read(cx).value().to_string())
        }

        fn detail<R>(&mut self, read: impl FnOnce(&Detail) -> R) -> Option<R> {
            self.read(|page, _| page.detail.as_ref().map(read))
        }

        /// A column of the fact's row in the database.
        fn stored<T: FromSql>(&mut self, column: &str, fact: FactId) -> T {
            let sql = format!("SELECT {column} FROM facts WHERE id = ?1");
            let value = self.database.read_with(self.cx, |database, _| {
                database
                    .connection()
                    .query_row(&sql, [fact.0], |row| row.get(0))
            });
            match value {
                Ok(value) => value,
                Err(error) => panic!("{sql}: {error}"),
            }
        }

        fn needs_review(&mut self, facts: &[FactId]) {
            let facts = facts.to_vec();
            self.run(move |write| {
                for fact in facts {
                    write.set(facts::REVIEW, fact.0, &Some("unsure"))?;
                }
                Ok(())
            });
        }
    }

    #[test]
    fn the_page_lists_the_sample() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        assert_eq!(page.count(Filter::All), 83);
        assert_eq!(page.count(Filter::NeedsReview), 1);
        assert_eq!(page.count(Filter::Hidden), 2);
        assert_eq!(page.listed().len(), 83);
        let titles = page.read(|page, cx| {
            page.list
                .read(cx)
                .delegate()
                .groups()
                .map(|(title, _)| title.to_string())
                .collect::<Vec<_>>()
        });
        assert_eq!(titles.first().map(String::as_str), Some("Profile"));
        assert!(titles.iter().any(|title| title.ends_with(", Northwind Logistics")));
        assert_eq!(page.selected(), None, "nothing until the user picks");
    }

    #[test]
    fn arrow_keys_select_and_show_the_fact() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        page.keys("down");
        let first = page.listed().first().copied();
        assert_eq!(page.selected(), first);
        assert_eq!(page.detail(|detail| detail.fact), first, "read once shown");
        page.keys("down");
        let second = page.listed().get(1).copied();
        assert_eq!(page.selected(), second);
        assert_eq!(page.detail(|detail| detail.fact), second);
        page.keys("up");
        assert_eq!(page.selected(), first);
    }

    #[test]
    fn the_detail_shows_sources_uses_and_duplicates() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let migration = page.fact("Led the migration");
        let linkedin = page.fact("Migrated 12 backend");
        page.show(migration);
        assert_eq!(page.detail(|detail| detail.sources.len()), Some(1));
        assert!(page.detail(|detail| !detail.uses.is_empty()) == Some(true));
        assert_eq!(
            page.detail(|detail| detail
                .duplicates
                .iter()
                .map(|duplicate| duplicate.fact)
                .collect::<Vec<_>>()),
            Some(vec![linkedin])
        );
    }

    #[test]
    fn enter_confirms_and_moves_to_the_next_to_review() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let listed = page.listed();
        let (first, second) = (listed[3], listed[10]);
        page.needs_review(&[first, second]);
        assert_eq!(page.count(Filter::NeedsReview), 3);
        page.show(first);
        page.keys("enter");
        assert_eq!(page.stored::<Option<String>>("review", first), None);
        assert_eq!(page.selected(), Some(second), "the next one to review");
        assert_eq!(page.count(Filter::NeedsReview), 2);
        assert!(page.row(first).is_some_and(|fact| fact.review.is_none()));
    }

    #[test]
    fn confirming_the_last_to_review_empties_that_filter() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        page.filter(Filter::NeedsReview);
        assert_eq!(page.listed().len(), 1);
        page.keys("down");
        page.keys("enter");
        assert_eq!(page.count(Filter::NeedsReview), 0);
        assert!(page.listed().is_empty());
        assert_eq!(page.selected(), None, "nothing listed to select");
    }

    #[test]
    fn backspace_hides_and_shows_again() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        page.show(latency);
        page.keys("backspace");
        assert!(page.stored::<bool>("hidden", latency));
        assert_eq!(page.count(Filter::Hidden), 3);
        assert_eq!(page.selected(), Some(latency), "still listed under All");
        page.keys("delete");
        assert!(!page.stored::<bool>("hidden", latency));
        assert_eq!(page.count(Filter::Hidden), 2);
    }

    #[test]
    fn showing_a_fact_under_hidden_moves_to_the_first_still_listed() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        page.filter(Filter::Hidden);
        let listed = page.listed();
        assert_eq!(listed.len(), 2);
        page.show(listed[0]);
        page.keys("backspace");
        assert_eq!(page.listed(), vec![listed[1]]);
        assert_eq!(page.selected(), Some(listed[1]));
        assert_eq!(page.detail(|detail| detail.fact), Some(listed[1]));
    }

    #[test]
    fn an_edit_shows_at_once_and_saves_on_enter() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        page.needs_review(&[latency]);
        page.show(latency);
        page.type_fact_text("Cut p99 latency by 80%.");
        page.cx.simulate_keystrokes("enter");
        // Before the write commits: the list's own copy.
        assert_eq!(
            page.row(latency).map(|fact| fact.text),
            Some("Cut p99 latency by 80%.".into())
        );
        page.settle();
        assert_eq!(page.stored::<String>("text", latency), "Cut p99 latency by 80%.");
        assert_eq!(page.stored::<Option<String>>("review", latency), None, "rewording reviews");
        assert!(
            page.detail(|detail| detail.uses.iter().all(|used| used.outdated)) == Some(true),
            "the résumés using it were generated before"
        );
        assert!(page.detail(|detail| detail.extracted.starts_with("Cut p99 latency")) == Some(true));

        page.update(|page, _, cx| page.revert_fact(latency, cx));
        let extracted: String = page.stored("extracted", latency);
        assert_eq!(page.stored::<String>("text", latency), extracted);
        assert_eq!(page.editor_text(), extracted, "the editor follows a revert");
    }

    #[test]
    fn escape_puts_the_text_back() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        let before: String = page.stored("text", latency);
        page.show(latency);
        page.type_fact_text("Something else");
        page.keys("escape");
        assert_eq!(page.editor_text(), before);
        // Leaving the editor saves, so Esc must have put it back first.
        page.keys("down");
        assert_eq!(page.stored::<String>("text", latency), before);
    }

    #[test]
    fn an_empty_or_unchanged_edit_saves_nothing() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        page.needs_review(&[latency]);
        let before: String = page.stored("text", latency);
        page.show(latency);
        page.type_fact_text("   ");
        page.keys("enter");
        assert_eq!(page.stored::<String>("text", latency), before);
        page.type_fact_text(&format!("  {before} "));
        page.keys("enter");
        assert_eq!(page.stored::<String>("text", latency), before);
        assert!(
            page.stored::<Option<String>>("review", latency).is_some(),
            "nothing changed, so nothing was reviewed"
        );
    }

    #[test]
    fn moving_away_saves_an_edit() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        let other = page.fact("Led the migration");
        page.show(latency);
        page.type_fact_text("Cut p99 latency by 80%.");
        page.show(other);
        assert_eq!(page.stored::<String>("text", latency), "Cut p99 latency by 80%.");
        assert_eq!(page.editor_text(), page.stored::<String>("text", other));
    }

    #[test]
    fn a_note_adds_its_lines_as_facts_and_selects_the_first() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        page.filter(Filter::Hidden);
        page.update(|page, window, cx| page.open_add_form(window, cx));
        page.cx.simulate_input("Organized Code Fest 2019.");
        page.keys("shift-enter");
        page.cx.simulate_input("Ran a team of 15.");
        page.keys("ctrl-enter");
        page.settle();
        assert!(!page.read(|page, _| page.adding), "the form closed");
        assert_eq!(page.count(Filter::All), 85);
        assert_eq!(page.read(|page, cx| page.list.read(cx).delegate().filter()), Filter::All);
        let first = page.fact("Organized Code Fest 2019.");
        let second = page.fact("Ran a team of 15.");
        assert_eq!(page.selected(), Some(first));
        let (first_row, second_row) = (page.row(first), page.row(second));
        assert!(first_row.as_ref().is_some_and(|fact| fact.review.is_none()));
        assert_eq!(
            first_row.and_then(|fact| fact.entry),
            second_row.and_then(|fact| fact.entry),
            "written together, grouped together"
        );
        let titles = page.read(|page, cx| {
            page.list
                .read(cx)
                .delegate()
                .groups()
                .map(|(title, _)| title.to_string())
                .collect::<Vec<_>>()
        });
        assert!(titles.iter().any(|title| title.starts_with("Your note, ")));
    }

    #[test]
    fn escape_closes_the_note_without_adding_it() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        page.update(|page, window, cx| page.open_add_form(window, cx));
        page.cx.simulate_input("Gave a guest lecture.");
        page.keys("escape");
        assert!(!page.read(|page, _| page.adding));
        assert_eq!(page.count(Filter::All), 83);
        page.update(|page, window, cx| page.open_add_form(window, cx));
        let typed = page.read(|page, cx| page.note_editor.read(cx).value().to_string());
        assert_eq!(typed, "", "what was typed is thrown away");
        page.keys("ctrl-enter");
        assert_eq!(page.count(Filter::All), 83, "an empty note adds nothing");
    }

    #[test]
    fn ctrl_enter_marks_facts_and_enter_merges_them_keeping_the_picked_wording() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let migration = page.fact("Led the migration");
        let linkedin = page.fact("Migrated 12 backend");
        page.show(migration);
        page.show(linkedin);
        page.keys("ctrl-enter");
        assert_eq!(page.marked(), vec![migration, linkedin]);
        assert_eq!(
            page.read(|page, _| page.merging.as_ref().map(|merging| merging.facts.clone())),
            Some(vec![migration, linkedin])
        );
        assert!(page.read(|page, _| page.merging.as_ref().is_some_and(|merging| merging.uses > 0)));
        page.keys("1");
        assert_eq!(page.read(|page, _| page.keep), Some(migration));
        page.keys("enter");

        let merged_into: Option<uuid::Uuid> = page.stored("merged_into", linkedin);
        assert_eq!(merged_into, Some(migration.0));
        assert_eq!(page.row(linkedin).map(|fact| fact.id), None);
        assert_eq!(page.count(Filter::All), 82);
        assert_eq!(page.selected(), Some(migration));
        assert!(page.marked().is_empty());
        assert_eq!(page.detail(|detail| detail.sources.len()), Some(2));

        page.undo();
        let merged_into: Option<uuid::Uuid> = page.stored("merged_into", linkedin);
        assert_eq!(merged_into, None);
        assert_eq!(page.count(Filter::All), 83);
        assert_eq!(page.detail(|detail| detail.sources.len()), Some(1));
        page.undo();
        assert_eq!(page.count(Filter::All), 83, "one undo per merge");
    }

    #[test]
    fn a_ctrl_enter_again_unmarks_and_escape_drops_the_marks() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let listed = page.listed();
        page.show(listed[0]);
        page.show(listed[1]);
        page.keys("ctrl-enter");
        page.keys("down");
        page.keys("ctrl-enter");
        assert_eq!(page.marked(), vec![listed[0], listed[1], listed[2]]);
        page.keys("ctrl-enter");
        assert_eq!(page.marked(), vec![listed[0], listed[1]]);
        page.keys("escape");
        assert!(page.marked().is_empty());
        assert_eq!(page.count(Filter::All), 83);
    }

    #[test]
    fn a_possible_duplicate_merges_into_the_fact_shown() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let migration = page.fact("Led the migration");
        let linkedin = page.fact("Migrated 12 backend");
        page.show(migration);
        page.update(|page, _, cx| page.merge_with(migration, linkedin, cx));
        assert_eq!(page.marked(), vec![migration, linkedin]);
        page.keys("enter");
        let merged_into: Option<uuid::Uuid> = page.stored("merged_into", linkedin);
        assert_eq!(merged_into, Some(migration.0));
    }

    #[test]
    fn a_merge_keeps_the_wording_that_needs_no_review() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let migration = page.fact("Led the migration");
        let linkedin = page.fact("Migrated 12 backend");
        page.needs_review(&[migration]);
        page.show(migration);
        page.show(linkedin);
        page.keys("ctrl-enter");
        page.keys("enter");
        let merged_into: Option<uuid::Uuid> = page.stored("merged_into", migration);
        assert_eq!(merged_into, Some(linkedin.0));
        assert_eq!(page.stored::<Option<String>>("review", linkedin), None);
    }

    #[test]
    fn undo_takes_back_the_latest_change_first() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let migration = page.fact("Led the migration");
        let linkedin = page.fact("Migrated 12 backend");
        page.show(migration);
        page.show(linkedin);
        page.keys("ctrl-enter");
        page.keys("enter");
        page.keys("backspace");
        assert!(page.stored::<bool>("hidden", migration));
        page.undo();
        assert!(!page.stored::<bool>("hidden", migration));
        let merged_into: Option<uuid::Uuid> = page.stored("merged_into", linkedin);
        assert_eq!(merged_into, Some(migration.0), "the merge stays");
    }

    #[test]
    fn a_search_narrows_the_list_and_showing_a_fact_clears_it() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        page.search("P99");
        assert_eq!(page.listed(), vec![latency], "case doesn't matter");
        page.search("no fact says this");
        assert!(page.listed().is_empty());
        let migration = page.fact("Led the migration");
        page.show(migration);
        assert_eq!(page.listed().len(), 83);
        assert_eq!(page.selected(), Some(migration));
        let query = page.read(|page, cx| page.search.read(cx).value().to_string());
        assert_eq!(query, "");
    }

    #[test]
    fn a_search_hiding_the_selected_fact_selects_the_first_match() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let migration = page.fact("Led the migration");
        let latency = page.fact("Cut p99 latency");
        page.show(migration);
        page.search("p99");
        assert_eq!(page.selected(), Some(latency));
    }

    #[test]
    fn a_change_from_elsewhere_shows() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        page.show(latency);
        page.run(move |write| commands::set_hidden(write, latency, true));
        assert!(page.row(latency).is_some_and(|fact| fact.hidden));
        page.run(move |write| commands::edit_fact(write, latency, "Halved p99 latency."));
        assert_eq!(page.editor_text(), "Halved p99 latency.");
    }

    #[test]
    fn a_change_the_page_doesnt_show_doesnt_read_it_again() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        page.show(latency);
        assert!(page.read(|page, _| page.detail.is_some()));
        // Settings: nothing the page shows.
        page.run(|write| {
            write.set(
                crate::database::schema::settings::APPEARANCE,
                1,
                &"dark",
            )
        });
        assert!(page.read(|page, _| page.detail.is_some()));
    }

    #[test]
    fn keys_do_nothing_with_no_fact_selected() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        page.keys("enter backspace delete 1 ctrl-enter escape");
        assert_eq!(page.count(Filter::All), 83);
        assert_eq!(page.count(Filter::NeedsReview), 1);
        assert_eq!(page.count(Filter::Hidden), 2);
        assert!(page.marked().is_empty());
    }

    #[test]
    fn three_facts_merge_with_the_wording_a_number_picks() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let listed = page.listed();
        let (first, second, third) = (listed[20], listed[21], listed[22]);
        page.show(first);
        page.show(second);
        page.keys("ctrl-enter down ctrl-enter");
        assert_eq!(page.marked(), vec![first, second, third]);
        page.keys("9");
        assert_eq!(page.read(|page, _| page.keep), None, "no ninth wording");
        page.keys("3 enter");
        for merged in [first, second] {
            let merged_into: Option<uuid::Uuid> = page.stored("merged_into", merged);
            assert_eq!(merged_into, Some(third.0));
        }
        assert_eq!(page.count(Filter::All), 81);
        assert_eq!(page.selected(), Some(third));
        page.undo();
        assert_eq!(page.count(Filter::All), 83);
    }

    #[test]
    fn undo_after_editing_the_merged_fact_takes_back_the_edit_first() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let migration = page.fact("Led the migration");
        let linkedin = page.fact("Migrated 12 backend");
        page.show(migration);
        page.show(linkedin);
        page.keys("ctrl-enter enter");
        let before = page.stored::<String>("text", migration);
        page.type_fact_text("Led the migration of 12 services.");
        page.keys("enter");
        assert_eq!(
            page.stored::<String>("text", migration),
            "Led the migration of 12 services."
        );
        page.undo();
        assert_eq!(page.stored::<String>("text", migration), before);
        let merged_into: Option<uuid::Uuid> = page.stored("merged_into", linkedin);
        assert_eq!(merged_into, Some(migration.0), "the merge stays");
    }

    #[test]
    fn opening_the_note_form_saves_an_edit() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        page.show(latency);
        page.type_fact_text("Cut p99 latency by 80%.");
        page.update(|page, window, cx| page.open_add_form(window, cx));
        assert_eq!(page.stored::<String>("text", latency), "Cut p99 latency by 80%.");
    }

    #[test]
    fn showing_a_fact_the_filter_leaves_out_lists_all_again() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        page.filter(Filter::Hidden);
        let latency = page.fact("Cut p99 latency");
        page.show(latency);
        assert_eq!(page.read(|page, cx| page.list.read(cx).delegate().filter()), Filter::All);
        assert_eq!(page.selected(), Some(latency));
    }

    #[test]
    fn down_from_the_search_goes_to_the_first_match() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        let latency = page.fact("Cut p99 latency");
        page.search("p99");
        page.keys("down");
        assert_eq!(page.selected(), Some(latency));
        page.keys("backspace");
        assert!(page.stored::<bool>("hidden", latency), "the list has the keys again");
    }

    #[test]
    fn erasing_everything_empties_the_page_and_the_sample_comes_back() {
        let mut app = TestAppContext::single();
        let mut page = open(&mut app);
        page.keys("down");
        page.run(|write| write.erase_everything());
        assert_eq!(page.count(Filter::All), 0);
        assert_eq!(page.selected(), None);
        assert!(page.listed().is_empty());
        page.run(write_sample);
        assert_eq!(page.count(Filter::All), 83);
    }
}

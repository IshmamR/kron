use chrono::Local;
use gpui_kit::component::list::{ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, IndexPath, Sizable, h_flex};
use gpui_kit::{
    App, Context, Entity, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
};

use super::parts::fact_kind_label;
use crate::model::{DocumentKind, EntryId, Extraction, Fact, FactId, FactKind, FactStatus, Library};

/// Which facts the list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    NeedsReview,
    Hidden,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::All, Filter::NeedsReview, Filter::Hidden];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::NeedsReview => "Needs review",
            Filter::Hidden => "Hidden",
        }
    }

    pub fn includes(self, status: FactStatus) -> bool {
        match self {
            Filter::All => true,
            Filter::NeedsReview => status == FactStatus::Unreviewed,
            Filter::Hidden => status == FactStatus::Hidden,
        }
    }
}

/// The facts page's list: facts grouped by the role, project or degree they
/// belong to, with the profile first and loose facts last: from notes, then
/// from documents.
pub struct FactList {
    library: Entity<Library>,
    filter: Filter,
    /// Lowercased search text.
    query: String,
    groups: Vec<Group>,
    selected: Option<IndexPath>,
    /// Selected before the latest click, so a first ctrl-click marks both.
    previously_selected: Option<IndexPath>,
    /// Facts picked to merge, with ctrl-click (⌘-click on macOS).
    marked: Vec<FactId>,
}

impl ListDelegate for FactList {
    type Item = ListItem;

    fn sections_count(&self, _cx: &App) -> usize {
        self.groups.len()
    }

    fn items_count(&self, section: usize, _cx: &App) -> usize {
        self.groups.get(section).map_or(0, |group| group.facts.len())
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let group = self.groups.get(section)?;
        Some(
            div()
                .px_3()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(group.title.clone()),
        )
    }

    fn render_item(
        &mut self,
        index: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let id = self.fact_at(index)?;
        let marked = self.marked.contains(&id);
        let library = self.library.read(cx);
        let fact = library.fact(id)?;
        let theme = cx.theme();
        let hidden = fact.status == FactStatus::Hidden;
        let flagged = matches!(fact.extraction, Extraction::Flagged { .. });

        Some(
            ListItem::new(index)
                .px_3()
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .text_sm()
                        // A check when marked to merge; otherwise, like an unread
                        // mail, a dot when it's left to review.
                        .child(
                            div()
                                .flex_none()
                                .w(px(12.))
                                .flex()
                                .justify_center()
                                .map(|this| {
                                    if marked {
                                        this.child(
                                            Icon::new(IconName::Check)
                                                .xsmall()
                                                .text_color(theme.foreground),
                                        )
                                    } else if fact.status == FactStatus::Unreviewed {
                                        this.child(
                                            div().size(px(7.)).rounded_full().bg(theme.blue),
                                        )
                                    } else {
                                        this
                                    }
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when(hidden, |this| {
                                    this.line_through().text_color(theme.muted_foreground)
                                })
                                .child(fact.text.clone()),
                        )
                        .when(flagged, |this| {
                            this.child(
                                Icon::new(IconName::TriangleAlert)
                                    .xsmall()
                                    .text_color(theme.warning),
                            )
                        })
                        .when(fact.kind != FactKind::Bullet, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(fact_kind_label(fact.kind)),
                            )
                        }),
                ),
        )
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let text = if !self.query.is_empty() {
            "No facts match."
        } else {
            match self.filter {
                Filter::All => "No facts yet. Add some with +.",
                Filter::NeedsReview => "Nothing left to review.",
                Filter::Hidden => "No hidden facts.",
            }
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(text)
    }

    fn set_selected_index(
        &mut self,
        index: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
        if index != self.selected {
            self.previously_selected = self.selected;
        }
        self.selected = index;
    }

    /// Clicks land here. A ctrl-click marks the fact for merging; a plain
    /// click drops the marks, like selection in a file manager.
    fn confirm(&mut self, secondary: bool, _window: &mut Window, cx: &mut Context<ListState<Self>>) {
        if !secondary {
            self.marked.clear();
            cx.notify();
            return;
        }
        if self.marked.is_empty()
            && let Some(previous) = self.previously_selected.and_then(|index| self.fact_at(index))
        {
            self.marked.push(previous);
        }
        if let Some(fact) = self.selected.and_then(|index| self.fact_at(index)) {
            match self.marked.iter().position(|&marked| marked == fact) {
                Some(position) => {
                    self.marked.remove(position);
                }
                None => self.marked.push(fact),
            }
        }
        cx.notify();
    }
}

impl FactList {
    pub fn new(library: Entity<Library>, cx: &App) -> Self {
        let mut list = Self {
            library,
            filter: Filter::default(),
            query: String::new(),
            groups: Vec::new(),
            selected: None,
            previously_selected: None,
            marked: Vec::new(),
        };
        list.rebuild(cx);
        list
    }

    /// Regroups the facts, after the library, filter or search changed.
    pub fn rebuild(&mut self, cx: &App) {
        let library = self.library.read(cx);
        self.groups = groups(library, self.filter, &self.query);
        self.marked.retain(|&id| library.fact(id).is_some());
    }

    pub fn filter(&self) -> Filter {
        self.filter
    }

    pub fn set_filter(&mut self, filter: Filter, cx: &App) {
        self.filter = filter;
        self.rebuild(cx);
    }

    pub fn set_query(&mut self, query: &str, cx: &App) {
        self.query = query.trim().to_lowercase();
        self.rebuild(cx);
    }

    pub fn fact_at(&self, index: IndexPath) -> Option<FactId> {
        self.groups
            .get(index.section)?
            .facts
            .get(index.row)
            .copied()
    }

    pub fn index_of(&self, fact: FactId) -> Option<IndexPath> {
        self.groups
            .iter()
            .enumerate()
            .find_map(|(section, group)| {
                let row = group.facts.iter().position(|&id| id == fact)?;
                Some(IndexPath::new(row).section(section))
            })
    }

    /// The listed facts, top to bottom.
    pub fn facts(&self) -> impl Iterator<Item = FactId> + '_ {
        self.groups.iter().flat_map(|group| group.facts.iter().copied())
    }

    pub fn marked(&self) -> &[FactId] {
        &self.marked
    }

    pub fn set_marked(&mut self, marked: Vec<FactId>) {
        self.marked = marked;
    }
}

struct Group {
    title: SharedString,
    facts: Vec<FactId>,
}

fn groups(library: &Library, filter: Filter, query: &str) -> Vec<Group> {
    let listed = |fact: &&Fact| {
        filter.includes(fact.status) && (query.is_empty() || fact.text.to_lowercase().contains(query))
    };
    let entry_of = |fact: &Fact| fact.entry.filter(|&entry| library.entry(entry).is_some());
    let collect = |belongs: &dyn Fn(&Fact) -> bool| -> Vec<FactId> {
        library
            .facts
            .iter()
            .filter(listed)
            .filter(|fact| belongs(fact))
            .map(|fact| fact.id)
            .collect()
    };
    let is_profile = |fact: &Fact| {
        matches!(
            fact.kind,
            FactKind::Name | FactKind::Contact | FactKind::Summary | FactKind::Skill
        )
    };

    let mut groups = vec![Group {
        title: "Profile".into(),
        facts: collect(&|fact| entry_of(fact).is_none() && is_profile(fact)),
    }];
    for entry in &library.entries {
        groups.push(Group {
            title: entry_title(library, entry.id).into(),
            facts: collect(&|fact| entry_of(fact) == Some(entry.id)),
        });
    }
    let from_note = |fact: &Fact| {
        fact.sources
            .first()
            .and_then(|source| library.document(source.document))
            .is_some_and(|document| document.kind == DocumentKind::Note)
    };
    let loose = |fact: &Fact| entry_of(fact).is_none() && !is_profile(fact);
    groups.push(Group {
        title: "Your notes".into(),
        facts: collect(&|fact| loose(fact) && from_note(fact)),
    });
    groups.push(Group {
        title: "Other".into(),
        facts: collect(&|fact| loose(fact) && !from_note(fact)),
    });
    groups
}

/// The entry's title and organization, or, for facts written together in a
/// note, where they were written.
fn entry_title(library: &Library, entry: EntryId) -> String {
    if let Some(label) = library.entry_label(entry) {
        return label;
    }
    let note_date = library
        .facts
        .iter()
        .find(|fact| fact.entry == Some(entry))
        .and_then(|fact| fact.sources.first())
        .and_then(|source| library.document(source.document))
        .filter(|document| document.kind == DocumentKind::Note)
        .map(|document| document.added_at.with_timezone(&Local).format("%b %-d"));
    match note_date {
        Some(date) => format!("Your note, {date}"),
        None => "Untitled".into(),
    }
}

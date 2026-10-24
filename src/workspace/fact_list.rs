use std::collections::HashMap;

use chrono::{DateTime, Local, Utc};
use gpui_kit::component::list::{ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, IndexPath, Sizable, h_flex};
use gpui_kit::{
    App, Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
};
use rusqlite::Connection;

use super::parts::{EntryLabel, fact_kind_label};
use crate::database::Error;
use crate::model::{EntryId, FactId, FactKind, Review, ReviewReason};

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

    pub fn includes(self, fact: &ListedFact) -> bool {
        match self {
            Filter::All => true,
            Filter::NeedsReview => fact.review.is_some(),
            Filter::Hidden => fact.hidden,
        }
    }
}

/// A fact as the list shows it.
#[derive(Clone, Debug)]
pub struct ListedFact {
    pub id: FactId,
    pub kind: FactKind,
    pub text: String,
    /// The role, project or degree it belongs to, unless that was deleted.
    pub entry: Option<EntryId>,
    pub hidden: bool,
    pub review: Option<Review>,
    /// When the note it was first found in was written, if it came from one.
    noted_at: Option<DateTime<Utc>>,
}

/// The facts page's list: facts grouped by the role, project or degree they
/// belong to, with the profile first and loose facts last: from notes, then
/// from documents.
pub struct FactList {
    /// Every fact not merged into another, oldest first.
    facts: Vec<ListedFact>,
    /// Oldest first, which is the order of their groups.
    entries: Vec<EntryId>,
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
        let fact = self.fact(id)?;
        let theme = cx.theme();
        let hidden = fact.hidden;

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
                                    } else if fact.review.is_some() {
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
    pub fn new() -> Self {
        Self {
            facts: Vec::new(),
            entries: Vec::new(),
            filter: Filter::default(),
            query: String::new(),
            groups: Vec::new(),
            selected: None,
            previously_selected: None,
            marked: Vec::new(),
        }
    }

    /// Reads the facts again, after the database changed.
    pub fn load(&mut self, connection: &Connection) -> Result<(), Error> {
        self.facts = listed_facts(connection)?;
        self.entries = connection
            .prepare_cached("SELECT id FROM entries WHERE deleted_at IS NULL ORDER BY id")?
            .query_map([], |row| Ok(EntryId(row.get(0)?)))?
            .collect::<Result<_, _>>()?;
        let facts = &self.facts;
        self.marked
            .retain(|&id| facts.iter().any(|fact| fact.id == id));
        self.regroup();
        Ok(())
    }

    /// Every fact, listed or not.
    pub fn all(&self) -> &[ListedFact] {
        &self.facts
    }

    pub fn fact(&self, id: FactId) -> Option<&ListedFact> {
        self.facts.iter().find(|fact| fact.id == id)
    }

    /// Shows the fact with new text before the database has it, so an edit
    /// doesn't flash the old text.
    pub fn set_text(&mut self, id: FactId, text: &str) {
        if let Some(fact) = self.facts.iter_mut().find(|fact| fact.id == id) {
            fact.text = text.to_string();
            self.regroup();
        }
    }

    pub fn filter(&self) -> Filter {
        self.filter
    }

    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.regroup();
    }

    pub fn set_query(&mut self, query: &str) {
        self.query = query.trim().to_lowercase();
        self.regroup();
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

    /// "Senior Software Engineer, Northwind Logistics", from the entry's title
    /// and organization facts.
    pub fn entry_label(&self, entry: EntryId) -> Option<String> {
        let mut label = EntryLabel::default();
        for fact in self.facts.iter().filter(|fact| fact.entry == Some(entry)) {
            label.add(fact.kind, &fact.text);
        }
        label.text()
    }

    /// Each group's title and its listed facts, top to bottom.
    #[cfg(test)]
    pub fn groups(&self) -> impl Iterator<Item = (&SharedString, &[FactId])> {
        self.groups
            .iter()
            .map(|group| (&group.title, group.facts.as_slice()))
    }

    /// Groups the facts again, after they, the filter or the search changed:
    /// the profile, each entry in order, then loose facts from notes and the
    /// rest.
    fn regroup(&mut self) {
        let mut namings: HashMap<EntryId, EntryNaming> = HashMap::new();
        for fact in &self.facts {
            if let Some(entry) = fact.entry {
                namings.entry(entry).or_default().add(fact);
            }
        }
        let mut groups = vec![Group::new("Profile")];
        let mut entry_groups = HashMap::new();
        for &entry in &self.entries {
            entry_groups.insert(entry, groups.len());
            let title = namings
                .get(&entry)
                .map_or_else(|| "Untitled".to_string(), EntryNaming::title);
            groups.push(Group::new(title));
        }
        let notes = groups.len();
        groups.push(Group::new("Your notes"));
        let other = groups.len();
        groups.push(Group::new("Other"));

        for fact in &self.facts {
            let listed = self.filter.includes(fact)
                && (self.query.is_empty() || fact.text.to_lowercase().contains(&self.query));
            if !listed {
                continue;
            }
            let profile = matches!(
                fact.kind,
                FactKind::Name | FactKind::Contact | FactKind::Summary | FactKind::Skill
            );
            let group = match fact.entry.and_then(|entry| entry_groups.get(&entry)) {
                Some(&group) => group,
                None if profile => 0,
                None if fact.noted_at.is_some() => notes,
                None => other,
            };
            groups[group].facts.push(fact.id);
        }
        // An empty group would still be a section to the list, which then
        // takes its first row for the first fact.
        groups.retain(|group| !group.facts.is_empty());
        self.groups = groups;
    }
}

struct Group {
    title: SharedString,
    facts: Vec<FactId>,
}

impl Group {
    fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            facts: Vec::new(),
        }
    }
}

/// What an entry is called, from its facts, oldest first.
#[derive(Default)]
struct EntryNaming<'a> {
    label: EntryLabel<'a>,
    /// When its first fact's note was written, if it came from one.
    noted_at: Option<Option<DateTime<Utc>>>,
}

impl<'a> EntryNaming<'a> {
    fn add(&mut self, fact: &'a ListedFact) {
        self.label.add(fact.kind, &fact.text);
        self.noted_at.get_or_insert(fact.noted_at);
    }

    /// The label, or, for facts written together in a note, when they were
    /// written.
    fn title(&self) -> String {
        if let Some(label) = self.label.text() {
            return label;
        }
        match self.noted_at.flatten() {
            Some(noted_at) => format!(
                "Your note, {}",
                noted_at.with_timezone(&Local).format("%b %-d")
            ),
            None => "Untitled".into(),
        }
    }
}

/// Every fact not merged into another, oldest first. A merged fact's sources
/// count for the fact at the end of its chain of merges.
fn listed_facts(connection: &Connection) -> Result<Vec<ListedFact>, Error> {
    let mut statement = connection.prepare_cached(
        "WITH RECURSIVE
             -- Each merged fact and the fact at the end of its chain of merges:
             -- up from the few merged facts, not down from every fact.
             chains(fact_id, root_id) AS (
                 SELECT id, merged_into FROM facts WHERE merged_into IS NOT NULL
                 UNION
                 SELECT chains.fact_id, facts.merged_into
                 FROM chains JOIN facts ON facts.id = chains.root_id
                 WHERE facts.merged_into IS NOT NULL
             ),
             merged(fact_id, root_id) AS (
                 SELECT chains.fact_id, chains.root_id
                 FROM chains JOIN facts ON facts.id = chains.root_id
                 WHERE facts.merged_into IS NULL
             ),
             -- min() makes the bare document_id the first source's.
             first_sources(root_id, document_id, source_id) AS (
                 SELECT coalesce(merged.root_id, fact_sources.fact_id), fact_sources.document_id,
                        min(fact_sources.id)
                 FROM fact_sources LEFT JOIN merged ON merged.fact_id = fact_sources.fact_id
                 WHERE fact_sources.deleted_at IS NULL
                 GROUP BY 1
             )
         SELECT facts.id, facts.kind, facts.text, entries.id, facts.hidden, facts.review,
                facts.review_note,
                CASE WHEN documents.file_name IS NULL AND documents.deleted_at IS NULL
                     THEN documents.added_at END
         FROM facts
         LEFT JOIN entries ON entries.id = facts.entry_id AND entries.deleted_at IS NULL
         LEFT JOIN first_sources ON first_sources.root_id = facts.id
         LEFT JOIN documents ON documents.id = first_sources.document_id
         WHERE facts.merged_into IS NULL AND facts.deleted_at IS NULL
         ORDER BY facts.id",
    )?;
    let facts = statement
        .query_map([], |row| {
            Ok(ListedFact {
                id: FactId(row.get(0)?),
                kind: row.get(1)?,
                text: row.get(2)?,
                entry: row.get::<_, Option<_>>(3)?.map(EntryId),
                hidden: row.get(4)?,
                review: row
                    .get::<_, Option<ReviewReason>>(5)?
                    .map(|reason| -> rusqlite::Result<Review> {
                        Ok(Review {
                            reason,
                            note: row.get(6)?,
                        })
                    })
                    .transpose()?,
                noted_at: row
                    .get::<_, Option<i64>>(7)?
                    .and_then(DateTime::from_timestamp_millis),
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(facts)
}

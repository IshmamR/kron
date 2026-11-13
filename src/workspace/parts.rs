// Small pieces shared by the pages.

use std::ops::Range;

use chrono::{DateTime, Local, Utc};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::InputState;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme, IconName, Root, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, App, Div, Entity, Focusable, FontWeight, HighlightStyle, Hsla, IntoElement,
    MouseButton, SharedString, StyledText, Window, div, prelude::*, px,
};

use rusqlite::Connection;

use super::shortcuts::keystroke;
use crate::database::Error;
use crate::model::{EntryId, FactId, FactKind};

pub fn heading(title: &str, count: Option<usize>, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .mb_2()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.to_owned())
        .when_some(count, |this, count| {
            this.child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(count.to_string()),
            )
        })
}

pub fn card(cx: &App) -> Div {
    v_flex()
        .p_2()
        .rounded(cx.theme().radius)
        .bg(cx.theme().muted)
}

pub fn caption(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// Keys drawn as key caps, then what they do: "[↑] [↓] move between lines".
/// `keys` are keystrokes as bindings spell them ("enter", "shift-enter");
/// an arrow is written as itself, which the cap shows as is.
pub fn key_hint(keys: &[&str], action: &str, cx: &App) -> Div {
    h_flex()
        .gap_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .children(
            keys.iter()
                .filter_map(|key| keystroke(key, cx))
                .map(|keystroke| Kbd::new(keystroke).outline()),
        )
        .child(div().ml_0p5().child(action.to_owned()))
}

pub fn notice(text: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded_sm()
        .border_l_2()
        .border_color(color)
        .bg(color.opacity(0.1))
        .text_xs()
        .child(text.into())
}

/// A page's name at its top, the same size on every page.
pub fn page_title(text: impl Into<SharedString>) -> Div {
    div()
        .text_2xl()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A text field drawn the quiet way, like Linear's: a soft fill and no border
/// at rest, a faint edge (the theme's ring) while typing in it.
///
/// A click anywhere in it, border included, puts the cursor in the text. The
/// kit's frame otherwise takes focus itself on a click past the text, lighting
/// up with nowhere to type.
pub fn text_field<F, S>(field: F, state: &Entity<S>, cx: &App) -> Div
where
    F: IntoElement + Styled,
    S: Focusable,
{
    let theme = cx.theme();
    let focus = state.focus_handle(cx);
    v_flex()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            focus.focus(window, cx)
        })
        .child(
            field
                .border_color(theme.transparent)
                .bg(theme.foreground.opacity(0.06)),
        )
}

/// The end of a search field: the shortcut to it while it's empty and the
/// cursor is elsewhere, and a button that clears it once there's text.
pub fn search_suffix(
    search: &Entity<InputState>,
    shortcut: &dyn Action,
    context: &str,
    clear: impl Fn(&mut Window, &mut App) + 'static,
    window: &Window,
    cx: &App,
) -> Option<AnyElement> {
    let search = search.read(cx);
    if !search.value().is_empty() {
        return Some(
            Button::new("clear-search")
                .ghost()
                .xsmall()
                .icon(IconName::Close)
                .tooltip("Clear search")
                .on_click(move |_, window, cx| clear(window, cx))
                .into_any_element(),
        );
    }
    if search.focus_handle(cx).is_focused(window) {
        return None;
    }
    Kbd::binding_for_action(shortcut, Some(context), window)
        .map(|kbd| kbd.outline().into_any_element())
}

pub fn fact_kind_label(kind: FactKind) -> &'static str {
    match kind {
        FactKind::Name => "Name",
        FactKind::Bullet => "Bullet",
        FactKind::Title => "Title",
        FactKind::Organization => "Organization",
        FactKind::Date => "Dates",
        FactKind::Skill => "Skill",
        FactKind::Contact => "Contact",
        FactKind::Summary => "Summary",
        FactKind::Location => "Location",
        FactKind::SkillGroup => "Skill group",
    }
}

/// Whether the fact needs review and whether it's hidden, each as a coloured
/// dot and plain text: coloured text, yellow especially, is hard to read on a
/// light background. Nothing for a fact that's fine as it is.
pub fn fact_status(needs_review: bool, hidden: bool, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let states = [
        needs_review.then_some(("Needs review", theme.warning)),
        hidden.then_some(("Hidden", theme.muted_foreground)),
    ];
    h_flex()
        .flex_none()
        .gap_2()
        .text_xs()
        .text_color(theme.muted_foreground)
        .children(states.into_iter().flatten().map(|(label, color)| {
            h_flex()
                .gap_1()
                .child(div().size(px(6.)).rounded_full().bg(color))
                .child(label)
        }))
}

/// Where a fact was found, as shown: the document's label, and the source
/// lines around it unless the document is gone.
pub struct Source {
    pub label: String,
    /// The lines, and where the fact falls within them.
    pub context: Option<(String, Range<usize>)>,
}

impl Source {
    /// `document` is the file's name, if it's a file, and when it was added;
    /// `None` once the document is deleted.
    pub fn new(
        document: Option<(Option<&str>, DateTime<Utc>)>,
        text: Option<&str>,
        span: &Range<usize>,
    ) -> Self {
        Self {
            label: match document {
                Some((Some(file_name), _)) => format!("From {file_name}"),
                Some((None, added_at)) => format!(
                    "From your note, {}",
                    added_at.with_timezone(&Local).format("%b %-d")
                ),
                None => "From a deleted document".into(),
            },
            context: document
                .and(text)
                .and_then(|text| source_context(text, span)),
        }
    }
}

/// Where a fact was found, with the surrounding lines for context.
pub fn source_excerpt(source: &Source, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    v_flex()
        .gap_1()
        .child(caption(source.label.clone(), cx))
        .when_some(source.context.clone(), |this, (text, highlight)| {
            this.child(
                div()
                    .p_2()
                    .rounded(theme.radius)
                    .bg(theme.background)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(StyledText::new(text).with_highlights([(
                        highlight,
                        HighlightStyle {
                            color: Some(theme.foreground),
                            background_color: Some(theme.selection),
                            ..Default::default()
                        },
                    )])),
            )
        })
}

/// Where the fact and the facts merged into it were found, in the order they
/// were found.
pub fn sources(connection: &Connection, fact: FactId) -> Result<Vec<Source>, Error> {
    let mut statement = connection.prepare_cached(
        "WITH RECURSIVE members(id) AS (
             SELECT ?1
             UNION
             SELECT facts.id FROM facts JOIN members ON facts.merged_into = members.id
         )
         SELECT documents.file_name, documents.added_at, documents.deleted_at IS NULL,
                document_texts.text, fact_sources.span_start, fact_sources.span_end
         FROM members
         -- CROSS JOIN keeps this order: by its index, not every source by id.
         CROSS JOIN fact_sources ON fact_sources.fact_id = members.id
         JOIN documents ON documents.id = fact_sources.document_id
         LEFT JOIN document_texts ON document_texts.document_id = documents.id
         WHERE fact_sources.deleted_at IS NULL
         ORDER BY fact_sources.id",
    )?;
    let sources = statement
        .query_map([fact.0], |row| {
            let file_name: Option<String> = row.get(0)?;
            let added_at = DateTime::from_timestamp_millis(row.get(1)?).unwrap_or_default();
            let kept: bool = row.get(2)?;
            let text: Option<String> = row.get(3)?;
            let span = offset(row.get(4)?)..offset(row.get(5)?);
            Ok(Source::new(
                kept.then_some((file_name.as_deref(), added_at)),
                text.as_deref(),
                &span,
            ))
        })?
        .collect::<Result<_, _>>()?;
    Ok(sources)
}

/// What an entry is called, from its facts, oldest first: "Senior Software
/// Engineer, Northwind Logistics".
#[derive(Default)]
pub struct EntryLabel<'a> {
    title: Option<&'a str>,
    skill_group: Option<&'a str>,
    organization: Option<&'a str>,
}

impl<'a> EntryLabel<'a> {
    pub fn add(&mut self, kind: FactKind, text: &'a str) {
        match kind {
            FactKind::Title => self.title = self.title.or(Some(text)),
            FactKind::SkillGroup => self.skill_group = self.skill_group.or(Some(text)),
            FactKind::Organization => self.organization = self.organization.or(Some(text)),
            _ => {}
        }
    }

    /// From the title, or else the skill group, and the organization.
    pub fn text(&self) -> Option<String> {
        match (self.title.or(self.skill_group), self.organization) {
            (Some(title), Some(organization)) => Some(format!("{title}, {organization}")),
            (Some(label), None) | (None, Some(label)) => Some(label.to_string()),
            (None, None) => None,
        }
    }
}

/// The entry's label, from the facts in it.
pub fn entry_label(connection: &Connection, entry: EntryId) -> Result<Option<String>, Error> {
    let mut statement = connection.prepare_cached(
        "SELECT kind, text FROM facts
         WHERE entry_id = ?1 AND merged_into IS NULL AND deleted_at IS NULL
           AND kind IN ('title', 'skill_group', 'organization')
         ORDER BY id",
    )?;
    let facts: Vec<(FactKind, String)> = statement
        .query_map([entry.0], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut label = EntryLabel::default();
    for (kind, text) in &facts {
        label.add(*kind, text);
    }
    Ok(label.text())
}

/// The source lines around `span`, and where `span` falls within them.
fn source_context(text: &str, span: &Range<usize>) -> Option<(String, Range<usize>)> {
    let start = text
        .get(..span.start)?
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let end = text
        .get(span.end..)?
        .find('\n')
        .map_or(text.len(), |newline| span.end + newline);
    let context = text.get(start..end)?;
    Some((context.to_string(), span.start - start..span.end - start))
}

/// Spans are kept non-negative by the schema.
fn offset(value: i64) -> usize {
    usize::try_from(value).unwrap_or_default()
}

/// Shows `notification`, where the window has the kit's Root for them: a
/// test's may not.
pub fn notify(notification: Notification, window: &mut Window, cx: &mut App) {
    if window.root::<Root>().flatten().is_some() {
        window.push_notification(notification, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_context_returns_the_whole_line() {
        let text = "first\n- Led the migration.\nlast";
        let start = text.find("migration").unwrap_or_default();
        let (context, highlight) = source_context(text, &(start..start + 9)).unwrap_or_default();
        assert_eq!(context, "- Led the migration.");
        assert_eq!(&context[highlight], "migration");
    }

    #[test]
    fn source_context_spans_lines_the_fact_spans() {
        let text = "a\nthe pipe-\nline works\nb";
        let start = text.find("pipe").unwrap_or_default();
        let end = text.find(" works").unwrap_or_default();
        let (context, _) = source_context(text, &(start..end)).unwrap_or_default();
        assert_eq!(context, "the pipe-\nline works");
    }

    #[test]
    fn source_context_rejects_spans_inside_a_character() {
        assert!(source_context("–", &(1..2)).is_none());
    }
}

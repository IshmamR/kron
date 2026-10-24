// Small pieces shared by the inspector and the facts page.

use std::ops::Range;

use chrono::{DateTime, Local, Utc};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    App, Div, FontWeight, HighlightStyle, Hsla, IntoElement, SharedString, StyledText, div,
    prelude::*, px,
};

use crate::model::{DocumentKind, FactKind, FactSource, Library};

pub fn heading(title: &str, count: Option<usize>, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_1()
        .mb_2()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(title.to_uppercase())
        .when_some(count, |this, count| this.child(format!("({count})")))
}

pub fn card(cx: &App) -> Div {
    v_flex()
        .p_2()
        .rounded(cx.theme().radius)
        .bg(cx.theme().secondary)
}

pub fn caption(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
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

    pub fn from_library(library: &Library, source: &FactSource) -> Self {
        let document = library.document(source.document);
        Self::new(
            document.map(|document| {
                let file_name = match &document.kind {
                    DocumentKind::File { file_name } => Some(file_name.as_str()),
                    DocumentKind::Note => None,
                };
                (file_name, document.added_at)
            }),
            document.map(|document| document.text.as_str()),
            &source.span,
        )
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

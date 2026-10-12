use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ElementId, Entity, IntoElement, SharedString, Window, div, prelude::*,
};

use super::ShowFact;
use super::parts::{
    caption, card, fact_kind_label, fact_status, heading, notice, source_excerpt,
};
use crate::model::{
    Extraction, Fact, FactChange, FactId, FactStatus, JobId, Library, Line, LineId, ResumeId,
    TailoredResume, UnsupportedClaim, UnsupportedReason,
};

/// The job view's right-hand panel: where the selected line came from, and
/// what the generator couldn't back with a fact.
#[derive(IntoElement)]
pub struct Inspector {
    library: Entity<Library>,
    job: JobId,
    resume: ResumeId,
    /// False for an older version, whose claims can't be acted on.
    current: bool,
    selected: Option<LineId>,
}

impl RenderOnce for Inspector {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let library = self.library.read(cx);
        let resume = library.job(self.job).and_then(|job| job.resume(self.resume));
        let selected_line = self
            .selected
            .zip(resume)
            .and_then(|(line, resume)| resume.line(line));

        let source = match (selected_line, resume) {
            (Some(line), Some(resume)) => sources(library, line, resume, cx).into_any_element(),
            // No-break spaces keep the arrows together.
            (None, Some(_)) => caption(
                "Select a line to see the facts it came from. ↑\u{a0}↓\u{a0}move between lines.",
                cx,
            )
            .into_any_element(),
            (_, None) => caption("Nothing generated yet.", cx).into_any_element(),
        };

        let claims: &[UnsupportedClaim] = resume.map_or(&[], |resume| &resume.unsupported_claims);
        let unsupported = if resume.is_none() {
            caption("Nothing generated yet.", cx).into_any_element()
        } else if claims.is_empty() {
            caption("No unsupported claims. Every line is backed by a fact.", cx).into_any_element()
        } else {
            v_flex()
                .gap_2()
                .children(
                    claims
                        .iter()
                        .enumerate()
                        .map(|(index, claim)| self.unsupported_claim(index, claim, cx)),
                )
                .into_any_element()
        };

        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_l_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("inspector-source")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_3()
                    .child(heading("Source", None, cx))
                    .child(source),
            )
            .child(
                div()
                    .id("inspector-unsupported")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(heading(
                        "Unsupported claims",
                        resume.map(|_| claims.len()),
                        cx,
                    ))
                    .child(unsupported),
            )
    }
}

impl Inspector {
    pub fn new(
        library: Entity<Library>,
        job: JobId,
        resume: ResumeId,
        current: bool,
        selected: Option<LineId>,
    ) -> Self {
        Self {
            library,
            job,
            resume,
            current,
            selected,
        }
    }

    /// The claim, why it was left out, and what the user can do about it:
    /// add it as a fact, or show the hidden fact it needed. Once they have,
    /// it says so, and that regenerating would use it.
    fn unsupported_claim(&self, index: usize, claim: &UnsupportedClaim, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let library = self.library.read(cx);
        let job = self.job;
        let hidden_fact = match claim.reason {
            UnsupportedReason::UsesHiddenFact(fact) => Some(fact),
            _ => None,
        };
        let resolved = claim.added_as.or(hidden_fact.filter(|&fact| {
            library
                .fact(fact)
                .is_some_and(|fact| fact.status != FactStatus::Hidden)
        }));

        let (reason, reason_color) = match (resolved, claim.added_as) {
            (Some(_), Some(_)) => (
                "Added to your facts. Review it, then regenerate to use it.".into(),
                theme.muted_foreground,
            ),
            (Some(_), None) => (
                "You showed the fact it needs again. Regenerate to use it.".into(),
                theme.muted_foreground,
            ),
            (None, _) => (unsupported_reason(&claim.reason), theme.danger),
        };
        let action = match (resolved, hidden_fact) {
            (Some(fact), _) => show_in_facts_button(("show-claim-fact", index), fact),
            (None, Some(fact)) => {
                let library = self.library.clone();
                Button::new(("show-hidden-fact", index))
                    .label("Show fact")
                    .icon(IconName::Eye)
                    .outline()
                    .xsmall()
                    .tooltip("Lets résumés use it again")
                    .on_click(move |_, _, cx| {
                        library.update(cx, |library, cx| {
                            library.set_fact_status(fact, FactStatus::Confirmed);
                            cx.notify();
                        });
                    })
            }
            (None, None) => {
                let library = self.library.clone();
                Button::new(("add-fact", index))
                    .label("Add as fact")
                    .outline()
                    .xsmall()
                    .on_click(move |_, _, cx| {
                        library.update(cx, |library, cx| {
                            library.add_unsupported_claim_as_fact(job, index);
                            cx.notify();
                        });
                    })
            }
        };

        card(cx)
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .when(resolved.is_none(), |this| this.line_through())
                    .child(claim.text.clone()),
            )
            .child(div().text_xs().text_color(reason_color).child(reason))
            .when(self.current, |this| this.child(h_flex().child(action)))
            .into_any_element()
    }
}

fn sources(
    library: &Library,
    line: &Line,
    resume: &TailoredResume,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .gap_3()
        .when(line.is_edited(), |this| {
            this.child(notice(
                "You edited this line after it was generated. Check it still says only what \
                 its facts say.",
                theme.warning,
            ))
        })
        .children(line.cites.iter().map(|&id| {
            match library.fact(id) {
                Some(fact) => fact_card(library, fact, resume, cx).into_any_element(),
                None => notice(
                    "This line cites a fact that no longer exists.",
                    theme.danger,
                )
                .into_any_element(),
            }
        }))
}

fn fact_card(
    library: &Library,
    fact: &Fact,
    resume: &TailoredResume,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let flag = match &fact.extraction {
        Extraction::Flagged { reason } => Some(reason.clone()),
        Extraction::Clean => None,
    };
    let changed = library
        .fact_change(resume, fact.id)
        .map(|change| match change {
            FactChange::Hidden => {
                "You hid this fact after the résumé was generated. Regenerate to leave it out."
            }
            FactChange::Shown => {
                "You showed this fact again after the résumé was generated, which left it out. \
                 Regenerate to use it."
            }
            FactChange::Reworded => {
                "You reworded this fact after the résumé was generated. Regenerate to use the \
                 new wording."
            }
        });
    let fact_id = fact.id;

    card(cx)
        .gap_1p5()
        .child(
            h_flex()
                .justify_between()
                .child(caption(fact_kind_label(fact.kind), cx))
                .child(fact_status(fact.status, cx)),
        )
        .child(div().text_sm().child(fact.text.clone()))
        .when_some(
            fact.entry.and_then(|entry| library.entry_label(entry)),
            |this, entry| this.child(caption(entry, cx)),
        )
        .when_some(changed, |this, changed| {
            this.child(notice(changed, theme.warning))
        })
        .when_some(flag, |this, reason| {
            this.child(notice(format!("Flagged: {reason}"), theme.warning))
        })
        .children(
            fact.sources
                .iter()
                .map(|source| source_excerpt(library, source, cx)),
        )
        .child(h_flex().child(show_in_facts_button(
            SharedString::from(format!("show-fact-{}", fact_id.0)),
            fact_id,
        )))
}

fn show_in_facts_button(id: impl Into<ElementId>, fact: FactId) -> Button {
    Button::new(id)
        .label("Show in Facts")
        .ghost()
        .xsmall()
        .on_click(move |_, window, cx| window.dispatch_action(Box::new(ShowFact { fact }), cx))
}

fn unsupported_reason(reason: &UnsupportedReason) -> String {
    match reason {
        UnsupportedReason::InventedFigure(figure) => {
            format!("“{figure}” isn't in any of your facts")
        }
        UnsupportedReason::InventedYear(year) => format!("{year} isn't in any of your facts"),
        UnsupportedReason::NoSupportingFact => "No fact supports this".into(),
        UnsupportedReason::UsesHiddenFact(_) => "Relies on a fact you hid".into(),
    }
}

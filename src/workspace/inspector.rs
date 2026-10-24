use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ElementId, Entity, IntoElement, SharedString, Window, div, prelude::*,
};
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use super::ShowFact;
use super::job_view::{ShownLine, ShownResume};
use super::parts::{
    Source, caption, card, entry_label, fact_kind_label, fact_status, heading, notice,
    source_excerpt, sources,
};
use crate::database::{Database, Error};
use crate::model::commands;
use crate::model::{
    ClaimId, EntryId, FactChange, FactId, FactKind, LineId, ResumeId, Review, ReviewReason,
    UnsupportedReason,
};

/// Longer merge chains than this are a loop, which merging never makes.
const MERGE_DEPTH: usize = 64;

/// The job view's right-hand panel: where the selected line came from, and
/// what the generator couldn't back with a fact.
#[derive(IntoElement)]
pub struct Inspector {
    database: Entity<Database>,
    resume: Rc<ShownResume>,
    inspection: Rc<Inspection>,
    /// False for an older version, whose claims can't be acted on.
    current: bool,
}

impl RenderOnce for Inspector {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let selected_line = self
            .inspection
            .line
            .and_then(|line| self.resume.lines.iter().find(|each| each.id == line));

        let source = match selected_line {
            Some(line) => self.sources(line, cx).into_any_element(),
            // No-break spaces keep the arrows together.
            None => caption(
                "Select a line to see the facts it came from. ↑\u{a0}↓\u{a0}move between lines.",
                cx,
            )
            .into_any_element(),
        };

        let claims = &self.inspection.claims;
        let unsupported = if claims.is_empty() {
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
                    .child(heading("Unsupported claims", Some(claims.len()), cx))
                    .child(unsupported),
            )
    }
}

impl Inspector {
    pub fn new(
        database: Entity<Database>,
        resume: Rc<ShownResume>,
        inspection: Rc<Inspection>,
        current: bool,
    ) -> Self {
        Self {
            database,
            resume,
            inspection,
            current,
        }
    }

    fn sources(&self, line: &ShownLine, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .gap_3()
            .when(line.is_edited(), |this| {
                this.child(notice(
                    "You edited this line after it was generated. Check it still says only \
                     what its facts say.",
                    theme.warning,
                ))
            })
            .children(self.inspection.facts.iter().map(|cited| match &cited.fact {
                Some(fact) => {
                    fact_card(fact, self.resume.change(cited.id), cx).into_any_element()
                }
                None => notice("This line cites a fact that no longer exists.", theme.danger)
                    .into_any_element(),
            }))
    }

    /// The claim, why it was left out, and what the user can do about it:
    /// add it as a fact, or show the hidden fact it needed. Once they have,
    /// it says so, and that regenerating would use it.
    fn unsupported_claim(&self, index: usize, claim: &Claim, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let hidden_fact = match claim.reason {
            UnsupportedReason::UsesHiddenFact(fact) => Some(fact),
            _ => None,
        };
        let resolved = claim
            .added_as
            .or(hidden_fact.filter(|_| claim.hidden_fact_shown));

        let (reason, reason_color) = match (resolved, claim.added_as) {
            (Some(_), Some(_)) => (
                "Added to your facts. Regenerate to use it.".into(),
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
                let database = self.database.clone();
                Button::new(("show-hidden-fact", index))
                    .label("Show fact")
                    .icon(IconName::Eye)
                    .outline()
                    .xsmall()
                    .tooltip("Lets résumés use it again")
                    .on_click(move |_, _, cx| {
                        database
                            .read(cx)
                            .queue(move |write| commands::set_hidden(write, fact, false));
                    })
            }
            (None, None) => {
                let database = self.database.clone();
                let claim = claim.id;
                Button::new(("add-fact", index))
                    .label("Add as fact")
                    .outline()
                    .xsmall()
                    .on_click(move |_, _, cx| {
                        database.read(cx).queue(move |write| {
                            commands::add_claim_as_fact(write, claim).map(drop)
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

/// What the inspector shows of a résumé beyond its lines: the selected
/// line's facts, and the claims left out of it.
pub struct Inspection {
    resume: ResumeId,
    pub(super) line: Option<LineId>,
    /// The selected line's facts, in the order it cites them.
    pub(super) facts: Vec<CitedFact>,
    pub(super) claims: Vec<Claim>,
}

pub(super) struct CitedFact {
    pub(super) id: FactId,
    /// `None` once it's deleted.
    pub(super) fact: Option<InspectedFact>,
}

pub(super) struct InspectedFact {
    id: FactId,
    pub(super) kind: FactKind,
    pub(super) text: String,
    pub(super) hidden: bool,
    pub(super) review: Option<Review>,
    pub(super) entry: Option<String>,
    pub(super) sources: Vec<Source>,
}

pub(super) struct Claim {
    pub(super) id: ClaimId,
    pub(super) text: String,
    /// A hidden fact it relies on counts as the fact merged into since.
    pub(super) reason: UnsupportedReason,
    /// The fact the user added from it, if they did.
    pub(super) added_as: Option<FactId>,
    /// The hidden fact it relies on has been shown again.
    pub(super) hidden_fact_shown: bool,
}

impl Inspection {
    pub fn read(
        connection: &Connection,
        resume: &ShownResume,
        line: Option<LineId>,
    ) -> Result<Self, Error> {
        let cites = line
            .and_then(|line| resume.lines.iter().find(|each| each.id == line))
            .map_or(&[][..], |line| &line.cites);
        let mut facts = Vec::new();
        for &fact in cites {
            facts.push(CitedFact {
                id: fact,
                fact: inspected_fact(connection, fact)?,
            });
        }
        Ok(Self {
            resume: resume.id,
            line,
            facts,
            claims: claims(connection, resume.id)?,
        })
    }

    /// For when it couldn't be read: nothing to show.
    pub fn empty(resume: ResumeId, line: Option<LineId>) -> Self {
        Self {
            resume,
            line,
            facts: Vec::new(),
            claims: Vec::new(),
        }
    }

    pub fn is_of(&self, resume: ResumeId, line: Option<LineId>) -> bool {
        self.resume == resume && self.line == line
    }
}

fn inspected_fact(connection: &Connection, fact: FactId) -> Result<Option<InspectedFact>, Error> {
    let row = connection
        .prepare_cached(
            "SELECT kind, text, hidden, review, review_note, entry_id FROM facts
             WHERE id = ?1 AND deleted_at IS NULL",
        )?
        .query_row([fact.0], |row| {
            let review: Option<ReviewReason> = row.get(3)?;
            Ok((
                row.get::<_, FactKind>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                review.map(|reason| (reason, row.get::<_, Option<String>>(4))),
                row.get::<_, Option<Uuid>>(5)?,
            ))
        })
        .optional()?;
    let Some((kind, text, hidden, review, entry)) = row else {
        return Ok(None);
    };
    let review = match review {
        Some((reason, note)) => Some(Review { reason, note: note? }),
        None => None,
    };
    let entry = match entry {
        Some(entry) => entry_label(connection, EntryId(entry))?,
        None => None,
    };
    Ok(Some(InspectedFact {
        id: fact,
        kind,
        text,
        hidden,
        review,
        entry,
        sources: sources(connection, fact)?,
    }))
}

/// The claims the generator left out of the résumé, in order.
fn claims(connection: &Connection, resume: ResumeId) -> Result<Vec<Claim>, Error> {
    type Row = (Uuid, String, String, Option<String>, Option<u16>, Option<Uuid>, Option<Uuid>);
    let rows: Vec<Row> = connection
        .prepare_cached(
            "SELECT id, text, reason, figure, year, hidden_fact_id, added_as
             FROM unsupported_claims
             WHERE generation_id = ?1 AND deleted_at IS NULL
             ORDER BY sort_key",
        )?
        .query_map([resume.0], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let mut claims = Vec::new();
    for (id, text, reason, figure, year, hidden_fact, added_as) in rows {
        let mut hidden_fact_shown = false;
        let reason = match (reason.as_str(), figure, year, hidden_fact) {
            ("invented_figure", Some(figure), _, _) => UnsupportedReason::InventedFigure(figure),
            ("invented_year", _, Some(year), _) => UnsupportedReason::InventedYear(year),
            ("no_supporting_fact", ..) => UnsupportedReason::NoSupportingFact,
            ("uses_hidden_fact", _, _, Some(fact)) => {
                let fact = root(connection, fact)?;
                hidden_fact_shown = connection
                    .prepare_cached("SELECT hidden FROM facts WHERE id = ?1 AND deleted_at IS NULL")?
                    .query_row([fact.0], |row| row.get::<_, bool>(0))
                    .optional()?
                    == Some(false);
                UnsupportedReason::UsesHiddenFact(fact)
            }
            (reason, ..) => {
                return Err(Error::Misuse(format!("unsupported_claims.reason: {reason}")));
            }
        };
        let added_as = match added_as {
            Some(fact) => Some(root(connection, fact)?),
            None => None,
        };
        claims.push(Claim {
            id: ClaimId(id),
            text,
            reason,
            added_as,
            hidden_fact_shown,
        });
    }
    Ok(claims)
}

/// The fact at the end of `fact`'s chain of merges: itself, unless it was
/// merged into another.
fn root(connection: &Connection, fact: Uuid) -> Result<FactId, Error> {
    let mut statement =
        connection.prepare_cached("SELECT merged_into FROM facts WHERE id = ?1")?;
    let mut fact = fact;
    for _ in 0..MERGE_DEPTH {
        let merged_into: Option<Uuid> = statement
            .query_row([fact], |row| row.get(0))
            .optional()?
            .flatten();
        match merged_into {
            Some(into) => fact = into,
            None => break,
        }
    }
    Ok(FactId(fact))
}

fn fact_card(fact: &InspectedFact, change: Option<FactChange>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let changed = change.map(|change| match change {
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
                .child(fact_status(fact.review.is_some(), fact.hidden, cx)),
        )
        .child(div().text_sm().child(fact.text.clone()))
        .when_some(fact.entry.clone(), |this, entry| this.child(caption(entry, cx)))
        .when_some(changed, |this, changed| {
            this.child(notice(changed, theme.warning))
        })
        .when_some(fact.review.as_ref(), |this, review| {
            this.child(notice(
                format!("Needs review: {}", review.description()),
                theme.warning,
            ))
        })
        .children(fact.sources.iter().map(|source| source_excerpt(source, cx)))
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

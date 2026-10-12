use std::cell::Cell;

use chrono::{DateTime, Utc};
use std::rc::Rc;

use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, App, Context, Div, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Hsla, IntoElement, KeyBinding, MouseButton, MouseDownEvent, ObjectFit, Pixels,
    Subscription, Window, actions, canvas, div, img, prelude::*, px, white,
};

use super::application_view;
use super::previews::{self, Previews, Render as PageRender};
use crate::model::{Job, JobId, Library, Nudge, Stage};

const KEY_CONTEXT: &str = "ResumeGrid";
const CARD_WIDTH: Pixels = px(200.);
/// The card minus the selection padding around the thumbnail.
const THUMBNAIL_WIDTH: Pixels = px(192.);
/// US Letter proportions (8.5 × 11), so the page fills the box.
const THUMBNAIL_HEIGHT: Pixels = px(248.);
/// Matches `gap_6` and `px_6` below; used to work out how many cards fit in a
/// row for ↑ and ↓.
const CARD_GAP: Pixels = px(24.);
const GRID_PADDING: Pixels = px(24.);
/// White text needs the accent this dark or darker to read (4.5:1).
const TITLE_ACCENT_MAX_LIGHTNESS: f32 = 0.45;
/// Résumé pages are paper, light in both themes.
const PAPER: Hsla = Hsla {
    h: 0.,
    s: 0.,
    l: 0.98,
    a: 1.,
};

actions!(
    resume_grid,
    [SelectLeft, SelectRight, SelectUp, SelectDown, OpenSelected]
);

/// Shows the `index`th filter: All, Active and so on.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = resume_grid, no_json)]
pub struct ShowFilter {
    pub index: usize,
}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", SelectLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(KEY_CONTEXT)),
    ]);
    cx.bind_keys((0..JobFilter::ALL.len()).map(|index| {
        KeyBinding::new(
            &format!("secondary-{}", index + 1),
            ShowFilter { index },
            Some(KEY_CONTEXT),
        )
    }));
}

pub enum ResumeGridEvent {
    Open(JobId),
}

/// Home screen: one card per job, showing its résumé's first page.
pub struct ResumeGrid {
    library: Entity<Library>,
    previews: Entity<Previews>,
    selected: Option<JobId>,
    filter: JobFilter,
    focus_handle: FocusHandle,
    /// Width available to the cards at the last layout.
    grid_width: Rc<Cell<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ResumeGridEvent> for ResumeGrid {}

impl Render for ResumeGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pixels_per_point = previews::pixels_per_point(THUMBNAIL_WIDTH, window);
        let now = Utc::now();
        let jobs = self.visible_jobs(cx, now);
        let renders: Vec<Option<PageRender>> = jobs
            .iter()
            .map(|&job| {
                self.previews
                    .update(cx, |previews, cx| previews.get(job, pixels_per_point, cx))
            })
            .collect();

        let library = self.library.read(cx);
        let job_count = library.jobs.len();
        let counts = JobFilter::ALL.map(|filter| {
            library
                .jobs
                .iter()
                .filter(|job| filter.includes(job, now))
                .count()
        });
        let empty = jobs.is_empty();
        let cards: Vec<AnyElement> = jobs
            .iter()
            .filter_map(|&id| library.job(id))
            .zip(renders)
            .map(|(job, render)| {
                let outdated = job
                    .current_resume()
                    .is_some_and(|resume| !library.changed_facts(resume).is_empty());
                self.render_card(job, render, outdated, now, cx)
                    .into_any_element()
            })
            .collect();
        let grid_width = self.grid_width.clone();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| {
                let columns = this.columns() as isize;
                this.move_selection(-columns, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| {
                let columns = this.columns() as isize;
                this.move_selection(columns, cx)
            }))
            .on_action(cx.listener(|this, action: &ShowFilter, _, cx| {
                this.set_filter(action.index, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, _, cx| {
                if let Some(job) = this.selected {
                    cx.emit(ResumeGridEvent::Open(job));
                }
            }))
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .items_baseline()
                    .gap_2()
                    .px(GRID_PADDING)
                    .pt_5()
                    .pb_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Résumés"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} · most recent activity first",
                                job_count_label(jobs.len(), job_count, self.filter)
                            )),
                    )
                    .child(div().flex_1())
                    .child(
                        TabBar::new("job-filter")
                            .segmented()
                            .small()
                            .selected_index(
                                JobFilter::ALL
                                    .iter()
                                    .position(|&filter| filter == self.filter)
                                    .unwrap_or_default(),
                            )
                            .children(JobFilter::ALL.iter().zip(counts).map(|(filter, count)| {
                                Tab::new().label(format!("{} {count}", filter.label()))
                            }))
                            .on_click(cx.listener(|this, index: &usize, _, cx| {
                                this.set_filter(*index, cx)
                            })),
                    ),
            )
            .when(empty, |this| {
                this.child(
                    centered(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.filter.empty_text()),
                    )
                    .flex_1(),
                )
            })
            .child(
                div()
                    .id("resume-grid-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_wrap()
                            .gap_6()
                            .px(GRID_PADDING)
                            .pb(GRID_PADDING)
                            .child(
                                canvas(
                                    move |bounds, _, _| grid_width.set(bounds.size.width),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .children(cards),
                    ),
            )
    }
}

impl Focusable for ResumeGrid {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ResumeGrid {
    pub fn new(
        library: Entity<Library>,
        previews: Entity<Previews>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _subscriptions: vec![
                cx.observe(&library, |_, _, cx| cx.notify()),
                cx.observe(&previews, |_, _, cx| cx.notify()),
            ],
            library,
            previews,
            selected: None,
            filter: JobFilter::default(),
            focus_handle: cx.focus_handle(),
            grid_width: Rc::new(Cell::new(px(0.))),
        }
    }

    pub fn select(&mut self, job: JobId, cx: &mut Context<Self>) {
        self.selected = Some(job);
        cx.notify();
    }

    fn columns(&self) -> usize {
        let available = self.grid_width.get() - GRID_PADDING * 2. + CARD_GAP;
        ((available / (CARD_WIDTH + CARD_GAP)).floor() as usize).max(1)
    }

    fn set_filter(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(&filter) = JobFilter::ALL.get(index) {
            self.filter = filter;
            cx.notify();
        }
    }

    /// The jobs the filter lets through, most recently active first.
    fn visible_jobs(&self, cx: &App, now: DateTime<Utc>) -> Vec<JobId> {
        let mut jobs: Vec<&Job> = self
            .library
            .read(cx)
            .jobs
            .iter()
            .filter(|job| self.filter.includes(job, now))
            .collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.last_activity(now)));
        jobs.into_iter().map(|job| job.id).collect()
    }

    /// Moves by `offset` cards; a move past either end is ignored.
    fn move_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        let jobs = self.visible_jobs(cx, Utc::now());
        let current = self
            .selected
            .and_then(|selected| jobs.iter().position(|&job| job == selected));
        let next = match current {
            Some(index) => index
                .checked_add_signed(offset)
                .and_then(|index| jobs.get(index)),
            None => jobs.first(),
        };
        if let Some(&next) = next {
            self.selected = Some(next);
            cx.notify();
        }
    }

    fn render_card(
        &self,
        job: &Job,
        render: Option<PageRender>,
        outdated: bool,
        now: DateTime<Utc>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let id = job.id;
        let selected = self.selected == Some(id);
        let unsupported = job
            .current_resume()
            .map_or(0, |resume| resume.unsupported_claims.len());
        let page_count = match &render {
            Some(PageRender::Ready(pages)) => pages.images.len(),
            _ => 0,
        };
        let mut details = format!(
            "{} · {}",
            job.company,
            application_view::stage_label(job.stage(now), now)
        );
        let nudge = application_view::nudge_label(job, now);
        // A calendar for an interview to come; otherwise, a quiet application.
        let nudge_icon = match job.nudge(now) {
            Some(Nudge::InterviewSoon { .. }) => IconName::Calendar,
            _ => IconName::Info,
        };
        if page_count > 1 {
            details.push_str(&format!(" · {page_count} pages"));
        }

        let thumbnail = match (job.current_resume(), render) {
            (None, _) => centered(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Not generated"),
            )
            .bg(theme.muted)
            .into_any_element(),
            (Some(_), Some(PageRender::Ready(pages))) => match pages.images.first() {
                Some(image) => img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                None => centered(div().text_xs().child("Empty page"))
                    .bg(PAPER)
                    .into_any_element(),
            },
            (Some(_), None) => centered(Spinner::new().small())
                .bg(PAPER)
                .into_any_element(),
            (Some(_), Some(PageRender::Failed(reason))) => centered(
                v_flex()
                    .p_3()
                    .gap_1()
                    .text_xs()
                    .child(div().text_color(theme.danger).child("Couldn't render"))
                    .child(div().text_color(theme.muted_foreground).child(reason)),
            )
            .bg(theme.muted)
            .into_any_element(),
        };

        v_flex()
            .id(job_card_id(id))
            .w(CARD_WIDTH)
            .gap_2()
            // Select on press, like Finder; a second press opens.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.selected = Some(id);
                    this.focus_handle.focus(window, cx);
                    if event.click_count >= 2 {
                        cx.emit(ResumeGridEvent::Open(id));
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .p_1()
                    .rounded(theme.radius_lg)
                    .when(selected, |this| this.bg(theme.list_active))
                    .child(
                        div()
                            .w(THUMBNAIL_WIDTH)
                            .h(THUMBNAIL_HEIGHT)
                            .overflow_hidden()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(if selected {
                                theme.list_active_border
                            } else {
                                theme.border
                            })
                            .shadow_sm()
                            .child(thumbnail),
                    ),
            )
            .child(
                v_flex()
                    .px_1()
                    .gap_0p5()
                    .child(
                        div()
                            .px_1()
                            .rounded(theme.radius)
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            // Like Finder: the selected title sits on the accent colour,
                            // darkened enough for white text to read in the light theme.
                            .when(selected, |this| {
                                let accent = theme.list_active_border;
                                this.bg(Hsla {
                                    l: accent.l.min(TITLE_ACCENT_MAX_LIGHTNESS),
                                    ..accent
                                })
                                .text_color(white())
                            })
                            .child(job.title.clone()),
                    )
                    .child(
                        div()
                            .px_1()
                            .text_xs()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(details),
                    )
                    .when(unsupported > 0 || outdated || nudge.is_some(), |this| {
                        this.child(
                            h_flex()
                                .px_1()
                                .pt_0p5()
                                .gap_1()
                                .flex_wrap()
                                .when_some(nudge, |this, nudge| {
                                    this.child(
                                        Tag::secondary()
                                            .outline()
                                            .xsmall()
                                            .gap_1()
                                            .child(
                                                Icon::new(nudge_icon.clone())
                                                    .xsmall()
                                                    .text_color(theme.muted_foreground),
                                            )
                                            .child(nudge),
                                    )
                                })
                                .when(unsupported > 0, |this| {
                                    this.child(
                                        Tag::danger()
                                            .outline()
                                            .xsmall()
                                            .child(unsupported_label(unsupported)),
                                    )
                                })
                                .when(outdated, |this| {
                                    // Neutral, with a yellow icon: yellow text is hard
                                    // to read on a light background.
                                    this.child(
                                        Tag::secondary()
                                            .outline()
                                            .xsmall()
                                            .gap_1()
                                            .child(
                                                Icon::new(IconName::TriangleAlert)
                                                    .xsmall()
                                                    .text_color(theme.warning),
                                            )
                                            .child("Facts changed"),
                                    )
                                }),
                        )
                    }),
            )
    }
}

/// Which applications the grid shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum JobFilter {
    #[default]
    All,
    /// Not closed, including not yet applied.
    Active,
    Interviewing,
    Offers,
    Closed,
}

impl JobFilter {
    const ALL: [Self; 5] = [
        Self::All,
        Self::Active,
        Self::Interviewing,
        Self::Offers,
        Self::Closed,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Active => "Active",
            Self::Interviewing => "Interviewing",
            Self::Offers => "Offers",
            Self::Closed => "Closed",
        }
    }

    fn includes(self, job: &Job, now: DateTime<Utc>) -> bool {
        let stage = job.stage(now);
        match self {
            Self::All => true,
            Self::Active => !stage.is_closed(),
            Self::Interviewing => stage.is_interviewing(),
            Self::Offers => stage == Stage::Offer,
            Self::Closed => stage.is_closed(),
        }
    }

    fn empty_text(self) -> &'static str {
        match self {
            Self::All => "No jobs yet.",
            Self::Active => "No active applications.",
            Self::Interviewing => "No interviews right now.",
            Self::Offers => "No offers yet.",
            Self::Closed => "No closed applications.",
        }
    }
}

/// "9 jobs", or with a filter, "3 of 9 jobs".
fn job_count_label(shown: usize, total: usize, filter: JobFilter) -> String {
    let jobs = if total == 1 { "job" } else { "jobs" };
    match filter {
        JobFilter::All => format!("{total} {jobs}"),
        _ => format!("{shown} of {total} {jobs}"),
    }
}

fn job_card_id(job: JobId) -> ElementId {
    ElementId::Name(format!("job-card-{}", job.0).into())
}

fn unsupported_label(count: usize) -> String {
    match count {
        1 => "1 unsupported claim".into(),
        count => format!("{count} unsupported claims"),
    }
}

fn centered(child: impl IntoElement) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(child)
}

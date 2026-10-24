use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{DateTime, Utc};

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
use rusqlite::Connection;

use super::application_view;
use super::previews::{self, Previews, Render as PageRender};
use crate::database::schema::{Events, FactVersions, Facts, Generations, Jobs, UnsupportedClaims};
use crate::database::{Changed, Database, Error};
use crate::model::{Application, JobId, Nudge, ResumeId, Stage};

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
    database: Entity<Database>,
    previews: Entity<Previews>,
    /// Every job's application, and each one's current résumé: read when
    /// first shown, and again after the database changes.
    applications: Option<Vec<Application>>,
    current_resumes: Option<HashMap<JobId, CurrentResume>>,
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
        self.read(cx);
        let current_resumes = self.current_resumes.clone().unwrap_or_default();
        let pixels_per_point = previews::pixels_per_point(THUMBNAIL_WIDTH, window);
        let now = Utc::now();
        let jobs = self.visible_jobs(now);
        let renders: Vec<Option<PageRender>> = jobs
            .iter()
            .map(|job| {
                let resume = current_resumes.get(job)?.id;
                self.previews
                    .update(cx, |previews, cx| previews.get(resume, pixels_per_point, cx))
            })
            .collect();

        let applications = self.applications.as_deref().unwrap_or_default();
        let job_count = applications.len();
        let counts = JobFilter::ALL.map(|filter| {
            applications
                .iter()
                .filter(|job| filter.includes(job, now))
                .count()
        });
        let empty = jobs.is_empty();
        let cards: Vec<AnyElement> = jobs
            .iter()
            .filter_map(|&id| applications.iter().find(|job| job.job == id))
            .zip(renders)
            .map(|(job, render)| {
                self.render_card(job, current_resumes.get(&job.job), render, now, cx)
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
        database: Entity<Database>,
        previews: Entity<Previews>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _subscriptions: vec![
                cx.observe(&previews, |_, _, cx| cx.notify()),
                cx.subscribe(&database, |this, _, changed: &Changed, cx| {
                    if changed.touches(Jobs) || changed.touches(Events) {
                        this.applications = None;
                        cx.notify();
                    }
                    if changed.touches(Generations)
                        || changed.touches(UnsupportedClaims)
                        || changed.touches(FactVersions)
                        || changed.touches(Facts)
                    {
                        this.current_resumes = None;
                        cx.notify();
                    }
                }),
            ],
            database,
            previews,
            applications: None,
            current_resumes: None,
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

    /// Reads the jobs and their résumés, unless they're read already.
    fn read(&mut self, cx: &App) {
        let connection = self.database.read(cx).connection();
        if self.applications.is_none() {
            self.applications = Some(Application::read_all(connection).unwrap_or_else(|error| {
                eprintln!("résumé grid: couldn't read the jobs: {error}");
                Vec::new()
            }));
        }
        if self.current_resumes.is_none() {
            self.current_resumes = Some(current_resumes(connection).unwrap_or_else(|error| {
                eprintln!("résumé grid: couldn't read the résumés: {error}");
                HashMap::new()
            }));
        }
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
    fn visible_jobs(&self, now: DateTime<Utc>) -> Vec<JobId> {
        let mut jobs: Vec<&Application> = self
            .applications
            .iter()
            .flatten()
            .filter(|job| self.filter.includes(job, now))
            .collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.last_activity(now)));
        jobs.into_iter().map(|job| job.job).collect()
    }

    /// Moves by `offset` cards; a move past either end is ignored.
    fn move_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        let jobs = self.visible_jobs(Utc::now());
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
        job: &Application,
        resume: Option<&CurrentResume>,
        render: Option<PageRender>,
        now: DateTime<Utc>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let id = job.job;
        let selected = self.selected == Some(id);
        let unsupported = resume.map_or(0, |resume| resume.unsupported);
        let outdated = resume.is_some_and(|resume| resume.outdated);
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

        let thumbnail = match (resume, render) {
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

/// What a card shows of its job's current résumé.
#[derive(Clone)]
struct CurrentResume {
    id: ResumeId,
    unsupported: usize,
    /// A fact it was generated from changed since.
    outdated: bool,
}

/// Each job's current résumé, by job. A fact merged into another since
/// counts as the one at the end of its merges.
fn current_resumes(connection: &Connection) -> Result<HashMap<JobId, CurrentResume>, Error> {
    let mut statement = connection.prepare_cached(
        "WITH RECURSIVE
             chains(fact_id, root_id) AS (
                 SELECT id, merged_into FROM facts WHERE merged_into IS NOT NULL
                 UNION
                 SELECT chains.fact_id, facts.merged_into
                 FROM chains JOIN facts ON facts.id = chains.root_id
                 WHERE facts.merged_into IS NOT NULL
             ),
             roots(fact_id, root_id) AS (
                 SELECT chains.fact_id, chains.root_id
                 FROM chains JOIN facts ON facts.id = chains.root_id
                 WHERE facts.merged_into IS NULL
             )
         SELECT current.job_id, current.generation_id,
                (SELECT count(*) FROM unsupported_claims
                 WHERE generation_id = current.generation_id AND deleted_at IS NULL),
                EXISTS (
                    SELECT 1 FROM fact_versions
                    LEFT JOIN roots ON roots.fact_id = fact_versions.fact_id
                    JOIN facts ON facts.id = coalesce(roots.root_id, fact_versions.fact_id)
                    WHERE fact_versions.generation_id = current.generation_id
                      AND facts.merged_into IS NULL AND facts.deleted_at IS NULL
                      AND (fact_versions.hidden != facts.hidden
                           OR fact_versions.text != facts.text)
                )
         FROM current_generations AS current
         WHERE current.kind = 'resume'",
    )?;
    let current = statement
        .query_map([], |row| {
            Ok((
                JobId(row.get(0)?),
                CurrentResume {
                    id: ResumeId(row.get(1)?),
                    unsupported: usize::try_from(row.get::<_, i64>(2)?).unwrap_or_default(),
                    outdated: row.get(3)?,
                },
            ))
        })?
        .collect::<Result<_, _>>()?;
    Ok(current)
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

    fn includes(self, job: &Application, now: DateTime<Utc>) -> bool {
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::database::{TemporaryDirectory, Write};
    use crate::model::{FactId, commands, sample};
    use crate::workspace::testing;

    /// A job's company and its current résumé's unsupported claims and
    /// whether a fact it was generated from changed since; `None` for a job
    /// without one.
    type Summary = (String, Option<(usize, bool)>);

    /// Each job's, in the board's order.
    fn summary(connection: &Connection) -> Result<Vec<Summary>, Error> {
        let current = current_resumes(connection)?;
        let jobs: Vec<(JobId, String)> = connection
            .prepare("SELECT id, company FROM jobs ORDER BY board_sort_key, id")?
            .query_map([], |row| Ok((JobId(row.get(0)?), row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(jobs
            .into_iter()
            .map(|(job, company)| {
                let resume = current.get(&job).map(|resume| (resume.unsupported, resume.outdated));
                (company, resume)
            })
            .collect())
    }

    fn expected(rows: &[(&str, Option<(usize, bool)>)]) -> Vec<Summary> {
        rows.iter()
            .map(|&(company, resume)| (company.to_string(), resume))
            .collect()
    }

    /// The sample as written: some résumés have unsupported claims, and
    /// some were generated before their facts were edited or hidden.
    const SAMPLE: [(&str, Option<(usize, bool)>); 9] = [
        ("Cinder Labs", Some((3, true))),
        ("Ledgerly", Some((2, false))),
        ("Atlas Freight", Some((1, true))),
        ("Tidewater Systems", Some((0, false))),
        ("Quillpay", Some((0, false))),
        ("Northstar Metrics", Some((0, false))),
        ("Brightwave", Some((0, true))),
        ("Kestrel Labs", Some((0, false))),
        ("Halcyon Cloud", None),
    ];

    /// The sample, with these companies' résumés outdated too.
    fn outdated(companies: &[&str]) -> Vec<Summary> {
        let mut rows = expected(&SAMPLE);
        for (company, resume) in &mut rows {
            if let Some((_, outdated)) = resume
                && companies.contains(&company.as_str())
            {
                *outdated = true;
            }
        }
        rows
    }

    #[test]
    fn the_current_resumes_read_back_what_the_sample_wrote() -> Result<(), Error> {
        let writer = sample::written()?;
        assert_eq!(summary(writer.kron())?, expected(&SAMPLE));
        Ok(())
    }

    #[test]
    fn the_current_resumes_follow_changes_to_facts() -> Result<(), Error> {
        let mut writer = sample::written()?;
        let migration = sample::fact(writer.kron(), "Led the migration")?;
        let linkedin = sample::fact(writer.kron(), "Migrated 12 backend")?;
        let latency = sample::fact(writer.kron(), "Cut p99 latency")?;
        let property_tests = sample::fact(writer.kron(), "Introduced property-based tests")?;

        writer.execute(|write| commands::edit_fact(write, latency, "Cut p99 latency by 80%."))?;
        writer.execute(|write| commands::set_hidden(write, property_tests, false))?;
        assert_eq!(summary(writer.kron())?, outdated(&["Ledgerly", "Tidewater Systems"]));

        // Back as they were generated from.
        writer.execute(|write| commands::revert_fact(write, latency))?;
        writer.execute(|write| commands::set_hidden(write, property_tests, true))?;
        assert_eq!(summary(writer.kron())?, expected(&SAMPLE));

        // A merged fact is compared with the fact it was merged into, which
        // reads differently.
        writer.execute(|write| commands::merge_facts(write, linkedin, &[migration]))?;
        assert_eq!(
            summary(writer.kron())?,
            outdated(&["Ledgerly", "Tidewater Systems", "Quillpay"])
        );
        Ok(())
    }

    // The grid in a window, driven by keys, with the thumbnails typeset.

    struct Grid<'a> {
        grid: Entity<ResumeGrid>,
        previews: Entity<Previews>,
        database: Entity<Database>,
        cx: &'a mut VisualTestContext,
        opened: Rc<Cell<Option<JobId>>>,
        _directory: TemporaryDirectory,
    }

    fn open(app: &mut TestAppContext) -> Grid<'_> {
        let (database, directory) = testing::open_sample(app);
        let previews = app.new(|cx| Previews::new(database.clone(), cx));
        let opened = Rc::new(Cell::new(None));
        let (grid, cx) = app.add_window_view(|_, cx| {
            ResumeGrid::new(database.clone(), previews.clone(), cx)
        });
        cx.update(|window, cx| {
            grid.focus_handle(cx).focus(window, cx);
            let opened = opened.clone();
            cx.subscribe(&grid, move |_, event: &ResumeGridEvent, _| match event {
                ResumeGridEvent::Open(job) => opened.set(Some(*job)),
            })
            .detach();
        });
        cx.run_until_parked();
        Grid {
            grid,
            previews,
            database,
            cx,
            opened,
            _directory: directory,
        }
    }

    impl Grid<'_> {
        fn settle(&mut self) {
            testing::settle(self.cx, &self.database);
        }

        fn keys(&mut self, keystrokes: &str) {
            self.cx.simulate_keystrokes(keystrokes);
            self.settle();
        }

        fn run(&mut self, command: impl FnOnce(&mut Write) -> Result<(), Error> + Send + 'static) {
            testing::run(self.cx, &self.database, command);
        }

        fn visible(&mut self) -> Vec<JobId> {
            self.grid
                .read_with(self.cx, |grid, _| grid.visible_jobs(Utc::now()))
        }

        fn selected(&mut self) -> Option<JobId> {
            self.grid.read_with(self.cx, |grid, _| grid.selected)
        }

        fn current(&mut self, job: JobId) -> Option<CurrentResume> {
            self.grid.read_with(self.cx, |grid, _| {
                grid.current_resumes
                    .as_ref()
                    .and_then(|current| current.get(&job).cloned())
            })
        }

        fn job(&mut self, company: &str) -> JobId {
            let job = self.database.read_with(self.cx, |database, _| {
                database.connection().query_row(
                    "SELECT id FROM jobs WHERE company = ?1",
                    [company],
                    |row| row.get(0),
                )
            });
            match job {
                Ok(job) => JobId(job),
                Err(error) => panic!("{company}: {error}"),
            }
        }

        fn fact(&mut self, start: &str) -> FactId {
            let fact = self
                .database
                .read_with(self.cx, |database, _| sample::fact(database.connection(), start));
            match fact {
                Ok(fact) => fact,
                Err(error) => panic!("{start}: {error}"),
            }
        }

        fn thumbnail(&mut self, job: JobId) -> Option<PageRender> {
            let resume = self.current(job)?.id;
            let previews = self.previews.clone();
            let thumbnail = self.cx.update(|window, cx| {
                let pixels_per_point = previews::pixels_per_point(THUMBNAIL_WIDTH, window);
                previews.update(cx, |previews, cx| previews.get(resume, pixels_per_point, cx))
            });
            self.settle();
            thumbnail
        }
    }

    #[test]
    fn every_generated_resume_gets_a_thumbnail() {
        let mut app = TestAppContext::single();
        let mut grid = open(&mut app);
        let jobs = grid.visible();
        assert_eq!(jobs.len(), 9);
        for job in jobs {
            match grid.thumbnail(job) {
                Some(PageRender::Ready(pages)) => assert!(!pages.images.is_empty()),
                Some(PageRender::Failed(reason)) => panic!("couldn't render: {reason}"),
                None => assert!(grid.current(job).is_none(), "only a job without a résumé"),
            }
        }
        let halcyon = grid.job("Halcyon Cloud");
        assert!(grid.current(halcyon).is_none());
    }

    #[test]
    fn arrow_keys_move_by_card_and_by_row_and_enter_opens() {
        let mut app = TestAppContext::single();
        let mut grid = open(&mut app);
        let jobs = grid.visible();
        let columns = grid.grid.read_with(grid.cx, |grid, _| grid.columns());
        assert!(columns > 1, "the test window fits more than one card a row");
        grid.keys("right");
        assert_eq!(grid.selected(), Some(jobs[0]), "the first card first");
        grid.keys("right");
        assert_eq!(grid.selected(), Some(jobs[1]));
        grid.keys("left left");
        assert_eq!(grid.selected(), Some(jobs[0]), "stays at the first");
        grid.keys("down");
        assert_eq!(grid.selected(), jobs.get(columns).copied());
        grid.keys("up");
        assert_eq!(grid.selected(), Some(jobs[0]));
        grid.keys("enter");
        assert_eq!(grid.opened.get(), Some(jobs[0]));
    }

    #[test]
    fn filter_keys_narrow_the_grid() {
        let mut app = TestAppContext::single();
        let mut grid = open(&mut app);
        let all = grid.visible().len();
        grid.keys("ctrl-5");
        let closed = grid.visible().len();
        assert!(closed > 0 && closed < all);
        grid.keys("ctrl-1");
        assert_eq!(grid.visible().len(), all);
    }

    #[test]
    fn a_changed_fact_tags_the_cards_using_it_and_reverting_clears_it() {
        let mut app = TestAppContext::single();
        let mut grid = open(&mut app);
        let ledgerly = grid.job("Ledgerly");
        assert!(grid.current(ledgerly).is_some_and(|resume| !resume.outdated));
        let latency = grid.fact("Cut p99 latency");
        grid.run(move |write| commands::edit_fact(write, latency, "Cut p99 latency by 80%."));
        assert!(grid.current(ledgerly).is_some_and(|resume| resume.outdated));
        grid.run(move |write| commands::revert_fact(write, latency));
        assert!(grid.current(ledgerly).is_some_and(|resume| !resume.outdated));
    }

    #[test]
    fn a_template_switch_typesets_again_and_a_fact_edit_does_not() {
        let mut app = TestAppContext::single();
        let mut grid = open(&mut app);
        let ledgerly = grid.job("Ledgerly");
        let Some(PageRender::Ready(before)) = grid.thumbnail(ledgerly) else {
            panic!("no thumbnail");
        };
        let latency = grid.fact("Cut p99 latency");
        grid.run(move |write| commands::edit_fact(write, latency, "Cut p99 latency by 80%."));
        let Some(PageRender::Ready(after_edit)) = grid.thumbnail(ledgerly) else {
            panic!("no thumbnail");
        };
        assert!(Arc::ptr_eq(&before, &after_edit), "the page didn't change");

        grid.run(move |write| commands::set_template(write, ledgerly, "sidebar"));
        let Some(PageRender::Ready(after_switch)) = grid.thumbnail(ledgerly) else {
            panic!("no thumbnail");
        };
        assert!(!Arc::ptr_eq(&before, &after_switch), "typeset again");
    }
}

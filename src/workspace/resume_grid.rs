use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{DateTime, Utc};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::{
    Action, Anchor, AnyElement, App, Context, Div, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, Hsla, IntoElement, KeyBinding, MouseButton, MouseDownEvent, ObjectFit,
    Pixels, ScrollHandle, Subscription, Window, actions, canvas, div, img, prelude::*, px,
};
use rusqlite::Connection;

use super::application_view;
use super::parts::{key_hint, page_title, search_suffix, text_field};
use super::previews::{self, Previews, Render as PageRender};
use crate::database::schema::{Events, FactVersions, Facts, Generations, Jobs, UnsupportedClaims};
use crate::database::{Changed, Database, Error};
use crate::model::{Application, JobId, Nudge, ResumeId, Stage};

const KEY_CONTEXT: &str = "ResumeGrid";
const SEARCH_CONTEXT: &str = "JobSearch";
/// The keys for `JobFilter::ALL`, in order. Spelled out because gpui only
/// reads keystrokes from `'static` strings.
const FILTER_KEYS: [&str; JobFilter::ALL.len()] = [
    "secondary-1",
    "secondary-2",
    "secondary-3",
    "secondary-4",
    "secondary-5",
];
const CARD_WIDTH: Pixels = px(248.);
const THUMBNAIL_WIDTH: Pixels = px(218.);
/// US Letter proportions (8.5 × 11), so the page fills the box.
const THUMBNAIL_HEIGHT: Pixels = px(282.);
const CARD_GAP: Pixels = px(36.);
const GRID_PADDING: Pixels = px(32.);
const COMPACT_GRID_PADDING: Pixels = px(12.);
const COMPACT_WINDOW_WIDTH: Pixels = px(640.);
/// Résumé pages are paper, light in both themes.
const PAPER: Hsla = Hsla {
    h: 0.,
    s: 0.,
    l: 0.98,
    a: 1.,
};

actions!(
    resume_grid,
    [
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        OpenSelected,
        FocusSearch,
        /// In the search: clears it, then leaves it for the grid.
        ClearSearch
    ]
);

/// Shows the `index`th filter: All, Active and so on.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = resume_grid, no_json)]
pub struct ShowFilter {
    pub index: usize,
}

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = resume_grid, no_json)]
struct SortJobs {
    sort: JobSort,
}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", SelectLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-f", FocusSearch, Some(KEY_CONTEXT)),
        KeyBinding::new(
            "escape",
            ClearSearch,
            Some(&format!("{SEARCH_CONTEXT} > Input")),
        ),
    ]);
    cx.bind_keys(
        FILTER_KEYS
            .iter()
            .enumerate()
            .map(|(index, key)| KeyBinding::new(key, ShowFilter { index }, Some(KEY_CONTEXT))),
    );
}

pub enum ResumeGridEvent {
    Open(JobId),
    NewJob,
    FiltersChanged,
}

/// Home screen: one card per job, showing its résumé's first page.
pub struct ResumeGrid {
    database: Entity<Database>,
    previews: Entity<Previews>,
    /// Read on startup and whenever jobs or their application history change.
    applications: Vec<Application>,
    current_resumes: Option<HashMap<JobId, CurrentResume>>,
    selected: Option<JobId>,
    filter: JobFilter,
    sort: JobSort,
    search: Entity<InputState>,
    query: String,
    focus_handle: FocusHandle,
    /// Width available to the cards at the last layout.
    card_area_width: Rc<Cell<Pixels>>,
    scroll_handle: ScrollHandle,
    /// The thumbnails shown, held so `Previews` never evicts one on screen.
    thumbnails: Vec<PageRender>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ResumeGridEvent> for ResumeGrid {}

impl Render for ResumeGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.read(cx);
        let grid = cx.entity();
        let suffix = search_suffix(
            &self.search,
            &FocusSearch,
            KEY_CONTEXT,
            move |window, cx| {
                grid.update(cx, |grid, cx| {
                    grid.clear_search(window, cx);
                    grid.search
                        .update(cx, |search, cx| search.focus(window, cx));
                })
            },
            window,
            cx,
        );
        let current_resumes = self.current_resumes.clone().unwrap_or_default();
        let pixels_per_point = previews::pixels_per_point(THUMBNAIL_WIDTH, window);
        let now = Utc::now();
        let jobs = self.visible_jobs(now);
        let renders: Vec<Option<PageRender>> = jobs
            .iter()
            .map(|job| {
                let resume = current_resumes.get(job)?.id;
                self.previews.update(cx, |previews, cx| {
                    previews.get(resume, pixels_per_point, cx)
                })
            })
            .collect();
        self.thumbnails = renders.iter().flatten().cloned().collect();

        let applications = &self.applications;
        let job_count = applications.len();
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
        let card_area_width = self.card_area_width.clone();
        let padding = if window.viewport_size().width < COMPACT_WINDOW_WIDTH {
            COMPACT_GRID_PADDING
        } else {
            GRID_PADDING
        };

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
            .on_action(
                cx.listener(|this, action: &ShowFilter, _, cx| this.set_filter(action.index, cx)),
            )
            .on_action(cx.listener(|this, action: &SortJobs, _, cx| {
                this.sort = action.sort;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search
                    .update(cx, |search, cx| search.focus(window, cx))
            }))
            .on_action(cx.listener(|this, _: &ClearSearch, window, cx| {
                if this.search.read(cx).value().is_empty() {
                    this.focus_handle.focus(window, cx);
                } else {
                    this.clear_search(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, window, cx| {
                if this.search.focus_handle(cx).is_focused(window) {
                    return;
                }
                if let Some(job) = this
                    .selected
                    .filter(|job| this.visible_jobs(Utc::now()).contains(job))
                {
                    cx.emit(ResumeGridEvent::Open(job));
                }
            }))
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .flex_wrap()
                    .justify_between()
                    .gap_3()
                    .px(padding)
                    .py_5()
                    .child(page_title("Résumés"))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                text_field(
                                    Input::new(&self.search)
                                        .small()
                                        .prefix(Icon::new(IconName::Search).small())
                                        .when_some(suffix, |this, suffix| this.suffix(suffix)),
                                    &self.search,
                                    cx,
                                )
                                .key_context(SEARCH_CONTEXT)
                                .w(px(176.)),
                            )
                            .child(sort_menu(self.sort))
                            .child(
                                Button::new("new-job")
                                    .label("New job")
                                    .icon(IconName::Plus)
                                    .small()
                                    .ghost()
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(ResumeGridEvent::NewJob)),
                                    ),
                            ),
                    ),
            )
            .when_else(
                empty,
                |this| {
                    this.child(
                        centered(
                            v_flex()
                                .items_center()
                                .gap_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .when_else(
                                    job_count == 0,
                                    |this| {
                                        this.child(Icon::new(IconName::FileText))
                                            .child(
                                                div()
                                                    .text_lg()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(cx.theme().foreground)
                                                    .child("Add your first job"),
                                            )
                                            .child(
                                                "Each job has its own résumé and application \
                                                 history.",
                                            )
                                            .child(
                                                Button::new("first-job")
                                                    .label("New job")
                                                    .icon(IconName::Plus)
                                                    .primary()
                                                    .on_click(cx.listener(|_, _, _, cx| {
                                                        cx.emit(ResumeGridEvent::NewJob)
                                                    })),
                                            )
                                    },
                                    |this| {
                                        this.child(if self.query.is_empty() {
                                            self.filter.empty_text()
                                        } else {
                                            "No jobs match your search."
                                        })
                                    },
                                ),
                        )
                        .flex_1(),
                    )
                },
                |this| {
                    this.child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .id("resume-grid-scroll")
                                    .relative()
                                    .size_full()
                                    .flex()
                                    .flex_wrap()
                                    .items_start()
                                    .content_start()
                                    .gap(CARD_GAP)
                                    .px(padding)
                                    .pt_5()
                                    .pb_8()
                                    .track_scroll(&self.scroll_handle)
                                    .overflow_y_scroll()
                                    .children(cards)
                                    .child(
                                        canvas(
                                            move |bounds, _, _| {
                                                card_area_width
                                                    .set(bounds.size.width - padding * 2.);
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    ),
                            )
                            .vertical_scrollbar(&self.scroll_handle),
                    )
                },
            )
            .child(
                h_flex()
                    .flex_none()
                    .justify_between()
                    .gap_2()
                    .px(padding)
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(job_count_label(jobs.len(), job_count))
                    .when(padding == GRID_PADDING, |this| {
                        this.child(
                            h_flex()
                                .gap_3()
                                .child(key_hint(&["↑", "↓", "←", "→"], "select", cx))
                                .child(key_hint(&["enter"], "opens", cx)),
                        )
                    }),
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search jobs"));
        let applications = read_applications(&database, cx);
        Self {
            _subscriptions: vec![
                cx.observe(&previews, |_, _, cx| cx.notify()),
                cx.subscribe(&database, |this, _, changed: &Changed, cx| {
                    if changed.touches(Jobs) || changed.touches(Events) {
                        this.applications = read_applications(&this.database, cx);
                        cx.emit(ResumeGridEvent::FiltersChanged);
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
                cx.subscribe(&search, |this, input, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.query = input.read(cx).value().trim().to_lowercase();
                        cx.notify();
                    }
                }),
            ],
            database,
            previews,
            applications,
            current_resumes: None,
            selected: None,
            filter: JobFilter::default(),
            sort: JobSort::default(),
            search,
            query: String::new(),
            focus_handle: cx.focus_handle(),
            card_area_width: Rc::new(Cell::new(px(0.))),
            scroll_handle: ScrollHandle::new(),
            thumbnails: Vec::new(),
        }
    }

    pub fn select(&mut self, job: JobId, cx: &mut Context<Self>) {
        self.selected = Some(job);
        cx.notify();
    }

    /// Reads the current résumés after a change, when their previews are needed.
    fn read(&mut self, cx: &App) {
        if self.current_resumes.is_none() {
            let connection = self.database.read(cx).connection();
            self.current_resumes = Some(current_resumes(connection).unwrap_or_else(|error| {
                eprintln!("résumé grid: couldn't read the résumés: {error}");
                HashMap::new()
            }));
        }
    }

    fn columns(&self) -> usize {
        let available = self.card_area_width.get() + CARD_GAP;
        ((available / (CARD_WIDTH + CARD_GAP)).floor() as usize).max(1)
    }

    pub(super) fn set_filter(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(&filter) = JobFilter::ALL.get(index) {
            self.filter = filter;
            cx.emit(ResumeGridEvent::FiltersChanged);
            cx.notify();
        }
    }

    pub(super) fn filters(&self) -> impl Iterator<Item = (&'static str, usize, bool)> + '_ {
        let now = Utc::now();
        JobFilter::ALL.into_iter().map(move |filter| {
            let count = self
                .applications
                .iter()
                .filter(|job| filter.includes(job, now))
                .count();
            (filter.label(), count, self.filter == filter)
        })
    }

    /// Empties the search, and the grid shows every job in the filter again.
    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        self.query.clear();
        cx.notify();
    }

    /// Jobs matching both the application filter and the search.
    fn visible_jobs(&self, now: DateTime<Utc>) -> Vec<JobId> {
        let mut jobs: Vec<&Application> = self
            .applications
            .iter()
            .filter(|job| self.filter.includes(job, now))
            .filter(|job| {
                self.query.is_empty()
                    || job.title.to_lowercase().contains(&self.query)
                    || job.company.to_lowercase().contains(&self.query)
            })
            .collect();
        match self.sort {
            JobSort::LastActivity => {
                jobs.sort_by_key(|job| std::cmp::Reverse(job.last_activity(now)))
            }
            JobSort::Company => jobs
                .sort_by_cached_key(|job| (job.company.to_lowercase(), job.title.to_lowercase())),
        }
        jobs.into_iter().map(|job| job.job).collect()
    }

    /// Moves by `offset` cards; a move past either end is ignored.
    fn move_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        let jobs = self.visible_jobs(Utc::now());
        let current = self
            .selected
            .and_then(|selected| jobs.iter().position(|&job| job == selected));
        let next = current.map_or(Some(0), |index| index.checked_add_signed(offset));
        if let Some((index, &job)) = next.and_then(|index| jobs.get(index).map(|job| (index, job)))
        {
            self.selected = Some(job);
            self.scroll_handle.scroll_to_item(index);
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
        let stage = job.stage(now);
        let (status, nudge) = card_status(job, now);
        let stage_color = match stage {
            Stage::Interview { .. } => theme.warning,
            Stage::Offer => theme.primary,
            _ => theme.muted_foreground,
        };

        let thumbnail = match (resume, render) {
            (None, _) => centered(
                v_flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::FileText).small())
                    .child("No résumé yet"),
            )
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
            .max_w_full()
            .flex_none()
            .gap_2()
            .rounded(theme.radius_lg)
            .p_1()
            .when(selected, |this| this.bg(theme.list_active))
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
                h_flex().w_full().justify_center().child(
                    div()
                        .relative()
                        .w(THUMBNAIL_WIDTH)
                        .max_w_full()
                        .aspect_ratio(THUMBNAIL_WIDTH / THUMBNAIL_HEIGHT)
                        .when_else(
                            resume.is_some(),
                            |this| this.bg(PAPER).shadow_md(),
                            |this| this.border_1().border_dashed().border_color(theme.border),
                        )
                        .child(thumbnail)
                        .when(page_count > 1, |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .right_2()
                                    .bottom_2()
                                    .px_1()
                                    .rounded_sm()
                                    .bg(theme.popover)
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{page_count} pages")),
                            )
                        }),
                ),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .w_full()
                    .px_1()
                    .pt_2()
                    .pb_1()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(job.title.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(job.company.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(
                                div()
                                    .size(px(5.))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(stage_color),
                            )
                            .child(div().truncate().child(status)),
                    )
                    .when(unsupported > 0 || outdated || nudge.is_some(), |this| {
                        this.child(
                            h_flex()
                                .gap_x_2()
                                .gap_y_1()
                                .pt_0p5()
                                .flex_wrap()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .when(unsupported > 0, |this| {
                                    this.child(
                                        h_flex()
                                            .gap_1()
                                            .child(Icon::new(IconName::Info).xsmall())
                                            .child(left_out_label(unsupported)),
                                    )
                                })
                                .when(outdated, |this| {
                                    this.child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Icon::new(IconName::TriangleAlert)
                                                    .xsmall()
                                                    .text_color(theme.warning),
                                            )
                                            .child("Facts changed"),
                                    )
                                })
                                .when_some(nudge, |this, nudge| {
                                    this.child(
                                        h_flex()
                                            .gap_1()
                                            .child(Icon::new(IconName::Info).xsmall())
                                            .child(nudge),
                                    )
                                }),
                        )
                    }),
            )
    }
}

fn read_applications(database: &Entity<Database>, cx: &App) -> Vec<Application> {
    Application::read_all(database.read(cx).connection()).unwrap_or_else(|error| {
        eprintln!("résumé grid: couldn't read the jobs: {error}");
        Vec::new()
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum JobSort {
    #[default]
    LastActivity,
    Company,
}

impl JobSort {
    const ALL: [Self; 2] = [Self::LastActivity, Self::Company];

    fn label(self) -> &'static str {
        match self {
            Self::LastActivity => "Last activity",
            Self::Company => "Company",
        }
    }
}

fn sort_menu(current: JobSort) -> impl IntoElement {
    Button::new("sort-jobs")
        .label(current.label())
        .ghost()
        .small()
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
            for sort in JobSort::ALL {
                menu = menu.menu_with_check(
                    sort.label(),
                    sort == current,
                    Box::new(SortJobs { sort }),
                );
            }
            menu
        })
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
            Self::All => "All applications",
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

/// The card's stage line, and the nudge under it. An interview coming up is
/// both, so it's said once, on the stage line.
fn card_status(job: &Application, now: DateTime<Utc>) -> (String, Option<String>) {
    let nudge = application_view::nudge_label(job, now);
    match job.nudge(now) {
        Some(Nudge::InterviewSoon { .. }) => (nudge.unwrap_or_default(), None),
        _ => (application_view::stage_label(job.stage(now), now), nudge),
    }
}

/// "9 jobs", or with a filter, "3 of 9 jobs".
fn job_count_label(shown: usize, total: usize) -> String {
    let jobs = if total == 1 { "job" } else { "jobs" };
    if shown == total {
        format!("{total} {jobs}")
    } else {
        format!("{shown} of {total} {jobs}")
    }
}

fn job_card_id(job: JobId) -> ElementId {
    ElementId::Name(format!("job-card-{}", job.0).into())
}

fn left_out_label(count: usize) -> String {
    match count {
        1 => "1 claim left out".into(),
        count => format!("{count} claims left out"),
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
                let resume = current
                    .get(&job)
                    .map(|resume| (resume.unsupported, resume.outdated));
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
        assert_eq!(
            summary(writer.kron())?,
            outdated(&["Ledgerly", "Tidewater Systems"])
        );

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
        let (grid, cx) = app.add_window_view(|window, cx| {
            ResumeGrid::new(database.clone(), previews.clone(), window, cx)
        });
        cx.update(|window, cx| {
            grid.focus_handle(cx).focus(window, cx);
            let opened = opened.clone();
            cx.subscribe(&grid, move |_, event: &ResumeGridEvent, _| {
                if let ResumeGridEvent::Open(job) = event {
                    opened.set(Some(*job));
                }
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
            let fact = self.database.read_with(self.cx, |database, _| {
                sample::fact(database.connection(), start)
            });
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
                previews.update(cx, |previews, cx| {
                    previews.get(resume, pixels_per_point, cx)
                })
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
    fn search_combines_with_application_filters_and_enter_opens_only_visible_jobs() {
        let mut app = TestAppContext::single();
        let mut grid = open(&mut app);
        let ledgerly = grid.job("Ledgerly");
        grid.cx.update(|window, cx| {
            let search = grid.grid.read(cx).search.clone();
            grid.grid.update(cx, |grid, cx| grid.select(ledgerly, cx));
            search.update(cx, |search, cx| search.focus(window, cx));
        });
        grid.cx.simulate_input("lEdGeR");
        assert_eq!(grid.visible(), vec![ledgerly]);
        grid.keys("enter");
        assert_eq!(
            grid.opened.get(),
            None,
            "Enter in search stays in the library"
        );
        grid.cx
            .update(|window, cx| grid.grid.focus_handle(cx).focus(window, cx));
        grid.keys("ctrl-5 enter");
        assert!(grid.visible().is_empty());
        assert_eq!(
            grid.opened.get(),
            None,
            "a filtered-out selection cannot open"
        );
        grid.keys("ctrl-1 enter");
        assert_eq!(grid.opened.get(), Some(ledgerly));
    }

    #[test]
    fn an_interview_coming_up_is_one_line_on_its_card() {
        let mut app = TestAppContext::single();
        let mut grid = open(&mut app);
        let ledgerly = grid.job("Ledgerly");
        let status = grid.cx.update(|_, cx| {
            let grid = grid.grid.read(cx);
            let job = grid.applications.iter().find(|job| job.job == ledgerly);
            job.map(|job| card_status(job, Utc::now()))
        });
        let Some((status, nudge)) = status else {
            panic!("no Ledgerly application");
        };
        assert_eq!(status, "Interview in 3 days");
        assert_eq!(nudge, None, "not said again under it");
    }

    #[test]
    fn ctrl_f_searches_and_escape_clears_then_returns_to_the_grid() {
        let mut app = TestAppContext::single();
        let mut grid = open(&mut app);
        let ledgerly = grid.job("Ledgerly");
        let all = grid.visible().len();
        grid.keys("ctrl-f");
        grid.cx.simulate_input("ledger");
        assert_eq!(grid.visible(), vec![ledgerly]);
        grid.keys("escape");
        assert_eq!(grid.visible().len(), all, "the first Esc clears the search");
        let search = grid.cx.update(|_, cx| grid.grid.read(cx).search.clone());
        grid.cx.update(|window, cx| {
            assert!(
                search.read(cx).focus_handle(cx).is_focused(window),
                "and stays in it"
            )
        });
        grid.keys("escape");
        grid.cx
            .update(|window, cx| assert!(grid.grid.focus_handle(cx).is_focused(window)));
    }

    #[test]
    fn a_changed_fact_tags_the_cards_using_it_and_reverting_clears_it() {
        let mut app = TestAppContext::single();
        let mut grid = open(&mut app);
        let ledgerly = grid.job("Ledgerly");
        assert!(
            grid.current(ledgerly)
                .is_some_and(|resume| !resume.outdated)
        );
        let latency = grid.fact("Cut p99 latency");
        grid.run(move |write| commands::edit_fact(write, latency, "Cut p99 latency by 80%."));
        assert!(grid.current(ledgerly).is_some_and(|resume| resume.outdated));
        grid.run(move |write| commands::revert_fact(write, latency));
        assert!(
            grid.current(ledgerly)
                .is_some_and(|resume| !resume.outdated)
        );
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

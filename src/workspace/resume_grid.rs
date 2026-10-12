use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Div, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Hsla, IntoElement, KeyBinding, MouseButton, MouseDownEvent, ObjectFit, Pixels,
    Subscription, Window, actions, canvas, div, img, prelude::*, px, white,
};

use super::job_view;
use super::previews::{self, Previews, Render as PageRender};
use crate::model::{Job, JobId, Library};

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

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", SelectLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(KEY_CONTEXT)),
    ]);
}

pub enum ResumeGridEvent {
    Open(JobId),
}

/// Home screen: one card per job, showing its résumé's first page.
pub struct ResumeGrid {
    library: Entity<Library>,
    previews: Entity<Previews>,
    selected: Option<JobId>,
    focus_handle: FocusHandle,
    /// Width available to the cards at the last layout.
    grid_width: Rc<Cell<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ResumeGridEvent> for ResumeGrid {}

impl Render for ResumeGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pixels_per_point = previews::pixels_per_point(THUMBNAIL_WIDTH, window);
        let jobs: Vec<JobId> = self
            .library
            .read(cx)
            .jobs
            .iter()
            .map(|job| job.id)
            .collect();
        let renders: Vec<Option<PageRender>> = jobs
            .iter()
            .map(|&job| {
                self.previews
                    .update(cx, |previews, cx| previews.get(job, pixels_per_point, cx))
            })
            .collect();

        let library = self.library.read(cx);
        let job_count = library.jobs.len();
        let cards: Vec<AnyElement> = library
            .jobs
            .iter()
            .zip(renders)
            .map(|(job, render)| {
                let outdated = job
                    .current_resume()
                    .is_some_and(|resume| !library.changed_facts(resume).is_empty());
                self.render_card(job, render, outdated, cx)
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
                            .child(match job_count {
                                1 => "1 job".to_string(),
                                count => format!("{count} jobs"),
                            }),
                    ),
            )
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

    /// Moves by `offset` cards; a move past either end is ignored.
    fn move_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        let jobs = &self.library.read(cx).jobs;
        let current = self
            .selected
            .and_then(|selected| jobs.iter().position(|job| job.id == selected));
        let next = match current {
            Some(index) => index
                .checked_add_signed(offset)
                .and_then(|index| jobs.get(index)),
            None => jobs.first(),
        };
        if let Some(next) = next {
            self.selected = Some(next.id);
            cx.notify();
        }
    }

    fn render_card(
        &self,
        job: &Job,
        render: Option<PageRender>,
        outdated: bool,
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
        let mut details = format!("{} · {}", job.company, job_view::status_label(job));
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
                    .when(unsupported > 0 || outdated, |this| {
                        this.child(
                            h_flex()
                                .px_1()
                                .pt_0p5()
                                .gap_1()
                                .flex_wrap()
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

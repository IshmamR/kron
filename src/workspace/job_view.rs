use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use chrono::Local;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::resizable::{ResizableState, h_resizable, resizable_panel};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Bounds, Context, Div, ElementId, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Hsla, Image, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    Point, ScrollHandle, SharedString, Stateful, Subscription, Window, actions, canvas, div, img, point,
    prelude::*, px, rgb,
};

use super::inspector::Inspector;
use super::previews::{self, Previews, Render as PageRender, RenderedPages};
use crate::model::{Job, JobEventKind, JobId, Library, LineId, TailoredResume};
use crate::typeset::{LineBox, PageSize};

const KEY_CONTEXT: &str = "JobView";
/// Pages are shown this wide: a bit over US Letter at 96 dpi.
const PAGE_WIDTH: Pixels = px(640.);
/// Between pages, and above the first and below the last.
const PAGE_GAP: f32 = 24.;
const INSPECTOR_WIDTH: Pixels = px(340.);
const INSPECTOR_MIN_WIDTH: Pixels = px(280.);
const INSPECTOR_MAX_WIDTH: Pixels = px(560.);
/// Keep this much room around a line scrolled into view.
const SCROLL_MARGIN: f32 = 48.;

// Drawn on the page, which is paper and stays light in both themes.
const PAPER: u32 = 0xfafafa;
const SELECTION: u32 = 0x2f6fde;
const EDITED_MARKER: u32 = 0xbf8700;

actions!(job_view, [SelectNextLine, SelectPreviousLine, Back]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", SelectNextLine, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectPreviousLine, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Back, Some(KEY_CONTEXT)),
    ]);
}

pub enum JobViewEvent {
    Close,
}

/// One job: its tailored résumé as typeset pages, next to the inspector.
pub struct JobView {
    library: Entity<Library>,
    previews: Entity<Previews>,
    job: Option<JobId>,
    selected: Option<LineId>,
    hovered: Option<LineId>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    /// The inspector's width.
    layout: Entity<ResizableState>,
    /// The pages as last drawn, to map clicks to lines and scroll to them.
    pages: Option<Arc<RenderedPages>>,
    /// Where each page was drawn in the window, written during paint.
    page_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    /// The selection was made from elsewhere; scroll to it once the pages
    /// are ready.
    scroll_pending: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<JobViewEvent> for JobView {}

impl Render for JobView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pixels_per_point = previews::pixels_per_point(PAGE_WIDTH, window);
        let render = self.job.and_then(|job| {
            self.previews
                .update(cx, |previews, cx| previews.get(job, pixels_per_point, cx))
        });
        self.pages = match &render {
            Some(PageRender::Ready(pages)) => Some(pages.clone()),
            _ => None,
        };
        if self.scroll_pending && self.pages.is_some() {
            self.scroll_pending = false;
            self.scroll_to_selected();
        }

        let library = self.library.read(cx);
        let content = match self.job.and_then(|job| library.job(job)) {
            Some(job) => h_resizable("job-view")
                .with_state(&self.layout)
                .child(resizable_panel().child(
                    v_flex()
                        .size_full()
                        .child(job_header(job, cx))
                        .when_some(outdated_notice(job, library, cx), |this, notice| {
                            this.child(notice)
                        })
                        .child(self.render_resume(job, render, cx)),
                ))
                .child(
                    resizable_panel()
                        .size(INSPECTOR_WIDTH)
                        .size_range(INSPECTOR_MIN_WIDTH..INSPECTOR_MAX_WIDTH)
                        .child(Inspector::new(self.library.clone(), job.id, self.selected)),
                )
                .into_any_element(),
            None => centered_message("No job selected", cx).into_any_element(),
        };

        div()
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &SelectNextLine, _, cx| this.move_selection(true, cx)))
            .on_action(
                cx.listener(|this, _: &SelectPreviousLine, _, cx| this.move_selection(false, cx)),
            )
            .on_action(cx.listener(Self::back))
            .size_full()
            .child(content)
    }
}

impl Focusable for JobView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl JobView {
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
            job: None,
            selected: None,
            hovered: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            layout: cx.new(|_| ResizableState::default()),
            pages: None,
            page_bounds: Rc::default(),
            scroll_pending: false,
        }
    }

    pub fn job(&self) -> Option<JobId> {
        self.job
    }

    pub fn set_job(&mut self, job: JobId, cx: &mut Context<Self>) {
        if self.job != Some(job) {
            self.job = Some(job);
            self.selected = None;
            self.hovered = None;
            self.pages = None;
            self.scroll_handle.set_offset(point(px(0.), px(0.)));
            cx.notify();
        }
    }

    pub fn select_line(&mut self, line: LineId, cx: &mut Context<Self>) {
        self.selected = Some(line);
        self.scroll_pending = true;
        cx.notify();
    }

    /// Esc first clears the line selection, then leaves the job.
    fn back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.take().is_some() {
            cx.notify();
        } else {
            cx.emit(JobViewEvent::Close);
        }
    }

    fn move_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        let library = self.library.read(cx);
        let Some(resume) = self.current_resume(library) else {
            return;
        };
        let lines: Vec<LineId> = resume.lines().map(|line| line.id).collect();
        let current = self
            .selected
            .and_then(|selected| lines.iter().position(|&line| line == selected));
        let next = match current {
            Some(index) if forward => lines.get(index + 1),
            Some(index) => index.checked_sub(1).and_then(|index| lines.get(index)),
            None if forward => lines.first(),
            None => lines.last(),
        };
        // At either end the selection stays put rather than wrapping.
        if let Some(&next) = next {
            self.selected = Some(next);
            self.scroll_to_selected();
            cx.notify();
        }
    }

    /// Scrolls just enough to show the selected line's first box.
    fn scroll_to_selected(&self) {
        let Some((pages, line)) = self.pages.as_ref().zip(self.selected) else {
            return;
        };
        let Some(first) = pages.layout.boxes_of(line).next() else {
            return;
        };
        let pages_above: f32 = pages
            .layout
            .pages
            .iter()
            .take(first.page)
            .map(|page| page.height * scale(*page) + PAGE_GAP)
            .sum();
        let page_scale = pages
            .layout
            .pages
            .get(first.page)
            .map_or(1., |page| scale(*page));
        let page_top = PAGE_GAP + pages_above;
        let top = page_top + first.top * page_scale - SCROLL_MARGIN;
        let bottom = page_top + first.bottom * page_scale + SCROLL_MARGIN;

        let offset = self.scroll_handle.offset();
        let viewport = f32::from(self.scroll_handle.bounds().size.height);
        let visible_top = -f32::from(offset.y);
        if top < visible_top {
            self.scroll_handle
                .set_offset(point(offset.x, px(-top.max(0.))));
        } else if bottom > visible_top + viewport {
            self.scroll_handle
                .set_offset(point(offset.x, px(viewport - bottom)));
        }
    }

    /// The line under `position` on page `page`, if any.
    fn line_at(&self, page: usize, position: Point<Pixels>) -> Option<LineId> {
        let pages = self.pages.as_ref()?;
        let size = *pages.layout.pages.get(page)?;
        let bounds = *self.page_bounds.borrow().get(page)?;
        let local = position - bounds.origin;
        let scale = scale(size);
        pages
            .layout
            .line_at(page, f32::from(local.x) / scale, f32::from(local.y) / scale)
    }

    fn current_resume<'a>(&self, library: &'a Library) -> Option<&'a TailoredResume> {
        self.job
            .and_then(|job| library.job(job))
            .and_then(Job::current_resume)
    }

    fn render_resume(
        &self,
        job: &Job,
        render: Option<PageRender>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(resume) = job.current_resume() else {
            return centered_message("No résumé for this job yet.", cx).into_any_element();
        };
        let pages = match render {
            Some(PageRender::Ready(pages)) => pages,
            Some(PageRender::Failed(reason)) => {
                return v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .text_sm()
                    .child(
                        div()
                            .text_color(cx.theme().danger)
                            .child("Couldn't typeset the résumé"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(reason),
                    )
                    .into_any_element();
            }
            None => {
                return div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Spinner::new())
                    .into_any_element();
            }
        };

        let library = self.library.read(cx);
        let changed = library.changed_facts(resume);
        let markers = Markers {
            edited: resume
                .lines()
                .filter(|line| line.is_edited())
                .map(|line| line.id)
                .collect(),
            outdated: resume
                .lines()
                .filter(|line| line.cites.iter().any(|fact| changed.contains(fact)))
                .map(|line| line.id)
                .collect(),
        };
        let page_elements = pages
            .images
            .iter()
            .zip(&pages.layout.pages)
            .enumerate()
            .map(|(index, (image, size))| {
                let boxes: Vec<LineBox> = pages
                    .layout
                    .boxes
                    .iter()
                    .filter(|line_box| line_box.page == index)
                    .copied()
                    .collect();
                self.render_page(index, image.clone(), *size, &boxes, &markers, cx)
            });

        // Our own scroll handle rather than `overflow_y_scrollbar`, which
        // brings its own, so the selected line can be scrolled into view.
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("resume-scroll")
                    .size_full()
                    .track_scroll(&self.scroll_handle)
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .items_center()
                            .gap(px(PAGE_GAP))
                            .py(px(PAGE_GAP))
                            .children(page_elements),
                    ),
            )
            .vertical_scrollbar(&self.scroll_handle)
            .into_any_element()
    }

    fn render_page(
        &self,
        index: usize,
        image: Arc<Image>,
        size: PageSize,
        boxes: &[LineBox],
        markers: &Markers,
        cx: &Context<Self>,
    ) -> AnyElement {
        let scale = scale(size);
        let page_bounds = self.page_bounds.clone();
        let highlight = |line_box: &LineBox, color: Hsla| {
            div()
                .absolute()
                .left(px(line_box.left * scale - 4.))
                .top(px(line_box.top * scale - 2.))
                .w(px((line_box.right - line_box.left) * scale + 8.))
                .h(px((line_box.bottom - line_box.top) * scale + 4.))
                .rounded_sm()
                .bg(color)
        };
        let hovered = self.hovered.filter(|&line| Some(line) != self.selected);

        div()
            .id(("page", index))
            .relative()
            .flex_none()
            .w(PAGE_WIDTH)
            .h(px(size.height * scale))
            .bg(rgb(PAPER))
            .shadow_md()
            .child(img(image).absolute().size_full())
            .children(
                boxes
                    .iter()
                    .filter(|line_box| Some(line_box.line) == hovered)
                    .map(|line_box| highlight(line_box, Hsla::black().opacity(0.05))),
            )
            .children(
                boxes
                    .iter()
                    .filter(|line_box| Some(line_box.line) == self.selected)
                    .map(|line_box| highlight(line_box, Hsla::from(rgb(SELECTION)).opacity(0.16))),
            )
            // Not part of the printed page: marks in the margin beside the
            // lines the user edited, and the lines that use changed facts.
            .children(boxes.iter().enumerate().flat_map(|(box_index, line_box)| {
                let middle = (line_box.top + line_box.bottom) / 2. * scale;
                let left = line_box.left * scale;
                let edited = markers.edited.contains(&line_box.line);
                let outdated = markers.outdated.contains(&line_box.line);
                let edited_marker = edited.then(|| {
                    margin_marker(
                        ("edited-marker", index * 1000 + box_index),
                        "You edited this line",
                    )
                    .left(px(left - 16.))
                    .top(px(middle - 3.))
                    .size(px(6.))
                    .rounded_full()
                    .bg(rgb(EDITED_MARKER))
                    .into_any_element()
                });
                let outdated_marker = outdated.then(|| {
                    margin_marker(
                        ("outdated-marker", index * 1000 + box_index),
                        "A fact this line uses changed after it was generated",
                    )
                    .left(px(left - if edited { 34. } else { 22. }))
                    .top(px(middle - 6.))
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .xsmall()
                            .text_color(rgb(EDITED_MARKER)),
                    )
                    .into_any_element()
                });
                edited_marker.into_iter().chain(outdated_marker)
            }))
            .child(
                canvas(
                    move |bounds, _, _| {
                        let mut all = page_bounds.borrow_mut();
                        if all.len() <= index {
                            all.resize(index + 1, Bounds::default());
                        }
                        if let Some(slot) = all.get_mut(index) {
                            *slot = bounds;
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            // Selects on press, like native lists; clicking between lines
            // clears the selection.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.selected = this.line_at(index, event.position);
                    this.focus_handle.focus(window, cx);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let hovered = this.line_at(index, event.position);
                if hovered != this.hovered {
                    this.hovered = hovered;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                if !hovering && this.hovered.take().is_some() {
                    cx.notify();
                }
            }))
            .into_any_element()
    }
}

/// Lines to mark in the page margin.
struct Markers {
    edited: Vec<LineId>,
    /// Lines citing facts that changed since the résumé was generated.
    outdated: Vec<LineId>,
}

fn margin_marker(id: impl Into<ElementId>, tooltip: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
}

/// A short label for where an application stands, also shown on the grid.
pub fn status_label(job: &Job) -> SharedString {
    match job.status() {
        None => "Not applied".into(),
        Some(JobEventKind::Applied) => "Applied".into(),
        Some(JobEventKind::RecruiterReplied) => "Recruiter replied".into(),
        Some(JobEventKind::InterviewScheduled { at }) => {
            format!("Interview {}", at.with_timezone(&Local).format("%b %-d")).into()
        }
        Some(JobEventKind::Rejected) => "Rejected".into(),
        Some(JobEventKind::Offer) => "Offer".into(),
        Some(JobEventKind::Withdrawn) => "Withdrawn".into(),
        Some(JobEventKind::Note(_)) => "Note".into(),
    }
}

/// Pixels on screen per typographic point.
fn scale(page: PageSize) -> f32 {
    f32::from(PAGE_WIDTH) / page.width
}

fn job_header(job: &Job, cx: &Context<JobView>) -> impl IntoElement {
    let mut details = vec![job.company.clone(), status_label(job).to_string()];
    if let Some(resume) = job.current_resume() {
        details.push(format!(
            "Generated {}",
            resume
                .generated_at
                .with_timezone(&Local)
                .format("%b %-d, %H:%M")
        ));
    }

    h_flex()
        .flex_none()
        .gap_2()
        .px_2()
        .py_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            Button::new("back")
                .ghost()
                .small()
                .icon(IconName::ChevronLeft)
                .tooltip("Back to résumés (Esc)")
                .on_click(cx.listener(|_, _, _, cx| cx.emit(JobViewEvent::Close))),
        )
        .child(
            v_flex()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(job.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .child(details.join(" · ")),
                ),
        )
}

/// Shown when facts the résumé was generated from changed since.
fn outdated_notice(job: &Job, library: &Library, cx: &App) -> Option<impl IntoElement> {
    let resume = job.current_resume()?;
    let changed = library.changed_facts(resume);
    if changed.is_empty() {
        return None;
    }
    // A fact it left out, like a hidden one shown again, is on no line.
    let marked = resume
        .lines()
        .any(|line| line.cites.iter().any(|fact| changed.contains(fact)));
    let mut text = if changed.len() == 1 {
        "A fact this résumé was generated from has changed since.".to_string()
    } else {
        format!(
            "{} facts this résumé was generated from have changed since.",
            changed.len()
        )
    };
    if marked {
        text.push_str(" Lines that use them are marked in the margin.");
    }
    let theme = cx.theme();
    Some(
        h_flex()
            .flex_none()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.warning.opacity(0.08))
            .text_sm()
            .child(Icon::new(IconName::TriangleAlert).small().text_color(theme.warning))
            .child(div().flex_1().child(text))
            .child(
                Button::new("regenerate")
                    .label("Regenerate")
                    .outline()
                    .small()
                    .disabled(true)
                    .tooltip("Regenerating isn't available yet"),
            ),
    )
}

fn centered_message(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

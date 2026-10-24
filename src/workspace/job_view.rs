use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::resizable::{ResizableState, h_resizable, resizable_panel};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab as KitTab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::{
    Action, Anchor, AnyElement, App, Bounds, Context, Div, ElementId, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Hsla, Image, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    Point, ScrollHandle, Stateful, Subscription, Window, actions, canvas, div, img, point,
    prelude::*, px, rgb,
};
use rusqlite::{Connection, OptionalExtension};
use super::application_view::{self, ApplicationView, ApplicationViewEvent};
use super::inspector::{Inspection, Inspector};
use super::previews::{self, Previews, Render as PageRender, RenderedPages};
use crate::database::schema::{
    Blocks, DocumentTexts, Documents, Entries, Events, FactSources, FactVersions, Facts,
    Generations, Jobs, LineCitations, Lines, Sections, UnsupportedClaims,
};
use crate::database::{Changed, Database, Error};
use crate::model::{self, Application, FactChange, FactId, JobId, LineId, ResumeId, commands};
use crate::typeset::{self, LineBox, PageSize, Template};

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
const EDITED_MARKER: u32 = 0x2f6fde;
const OUTDATED_MARKER: u32 = 0xbf8700;

actions!(
    job_view,
    [SelectNextLine, SelectPreviousLine, Back, ShowResumeTab, ShowApplicationTab]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", SelectNextLine, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectPreviousLine, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-1", ShowResumeTab, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-2", ShowApplicationTab, Some(KEY_CONTEXT)),
    ]);
    application_view::init(cx);
}

/// Lays the current résumé out with one of `typeset::TEMPLATES`.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = job_view, no_json)]
struct UseTemplate {
    key: &'static str,
}

pub enum JobViewEvent {
    Close,
}

/// One job: its tailored résumé as typeset pages, next to the inspector.
pub struct JobView {
    database: Entity<Database>,
    previews: Entity<Previews>,
    job: Option<JobId>,
    tab: Tab,
    /// The older résumé that was sent, when showing it instead of the
    /// current one.
    showing_sent: Option<ResumeId>,
    /// The job's title, company and events for the header, its versions,
    /// the one shown, and what the inspector shows of it: read when first
    /// shown, and again after the database changes.
    summary: Option<Application>,
    versions: Option<Versions>,
    shown: Option<Rc<ShownResume>>,
    inspection: Option<Rc<Inspection>>,
    application: Entity<ApplicationView>,
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
        self.read(cx);
        let pixels_per_point = previews::pixels_per_point(PAGE_WIDTH, window);
        let shown = self.shown_id();
        let render = shown.and_then(|resume| {
            self.previews
                .update(cx, |previews, cx| previews.get(resume, pixels_per_point, cx))
        });
        self.pages = match &render {
            Some(PageRender::Ready(pages)) => Some(pages.clone()),
            _ => None,
        };
        if self.scroll_pending && self.pages.is_some() {
            self.scroll_pending = false;
            self.scroll_to_selected();
        }

        let content = match &self.summary {
            Some(job) => v_flex()
                .size_full()
                .child(self.render_header(job, cx))
                .child(div().flex_1().min_h_0().child(match self.tab {
                    Tab::Resume => self.render_resume_tab(render, cx),
                    Tab::Application => self.application.clone().into_any_element(),
                }))
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
            .on_action(cx.listener(|this, _: &ShowResumeTab, window, cx| {
                this.set_tab(Tab::Resume, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowApplicationTab, window, cx| {
                this.set_tab(Tab::Application, window, cx)
            }))
            .on_action(cx.listener(|this, action: &UseTemplate, _, cx| {
                if let Some(job) = this.job {
                    let key = action.key;
                    this.database
                        .read(cx)
                        .queue(move |write| commands::set_template(write, job, key));
                }
            }))
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
        database: Entity<Database>,
        previews: Entity<Previews>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let application = cx.new(|cx| {
            ApplicationView::new(database.clone(), window, cx)
        });
        Self {
            _subscriptions: vec![
                cx.subscribe(&database, |this, _, changed: &Changed, cx| {
                    if shows(changed) {
                        this.summary = None;
                        this.versions = None;
                        this.shown = None;
                        this.inspection = None;
                        cx.notify();
                    }
                }),
                cx.observe(&previews, |_, _, cx| cx.notify()),
                cx.observe(&application, |_, _, cx| cx.notify()),
                cx.subscribe_in(&application, window, |this, _, event, window, cx| {
                    match event {
                        ApplicationViewEvent::ShowResume(resume) => {
                            this.show_version(*resume, cx);
                            this.set_tab(Tab::Resume, window, cx);
                        }
                    }
                }),
            ],
            database,
            previews,
            job: None,
            tab: Tab::Resume,
            showing_sent: None,
            summary: None,
            versions: None,
            shown: None,
            inspection: None,
            application,
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
            self.tab = Tab::Resume;
            self.showing_sent = None;
            self.application
                .update(cx, |application, cx| application.set_job(job, cx));
            self.selected = None;
            self.hovered = None;
            self.pages = None;
            self.scroll_handle.set_offset(point(px(0.), px(0.)));
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn selected_line(&self) -> Option<LineId> {
        self.selected
    }

    pub fn select_line(&mut self, line: LineId, cx: &mut Context<Self>) {
        self.tab = Tab::Resume;
        self.showing_sent = None;
        self.selected = Some(line);
        self.scroll_pending = true;
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        // The tab's controls may have had focus; keys still need somewhere
        // to go.
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    /// Shows a version of the résumé: the current one, or an older one sent.
    fn show_version(&mut self, resume: ResumeId, cx: &mut Context<Self>) {
        self.read(cx);
        let current = self
            .versions
            .as_ref()
            .and_then(|versions| versions.current.as_ref())
            .map(|version| version.id);
        self.showing_sent = (Some(resume) != current).then_some(resume);
        self.selected = None;
        self.hovered = None;
        self.pages = None;
        self.scroll_handle.set_offset(point(px(0.), px(0.)));
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
        if self.tab != Tab::Resume {
            return;
        }
        self.read(cx);
        let Some(resume) = self.shown.clone() else {
            return;
        };
        // As the template placed them, so ↓ goes where the eye goes; lines
        // the page doesn't show come last.
        let mut lines = self
            .pages
            .as_ref()
            .map(|pages| pages.layout.reading_order())
            .unwrap_or_default();
        for line in &resume.lines {
            if !lines.contains(&line.id) {
                lines.push(line.id);
            }
        }
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

    /// The résumé on screen: the current one, or the older one sent.
    fn shown_id(&self) -> Option<ResumeId> {
        let versions = self.versions.as_ref()?;
        match self.showing_sent {
            Some(resume) => versions
                .sent
                .as_ref()
                .filter(|sent| sent.id == resume)
                .map(|sent| sent.id),
            None => versions.current.as_ref().map(|current| current.id),
        }
    }

    fn shown_version(&self) -> Option<&Version> {
        let versions = self.versions.as_ref()?;
        let shown = self.shown_id()?;
        [versions.current.as_ref(), versions.sent.as_ref()]
            .into_iter()
            .flatten()
            .find(|version| version.id == shown)
    }

    /// Reads what's shown that isn't read yet: the job's versions, the
    /// résumé on screen, and what the inspector shows of it.
    fn read(&mut self, cx: &App) {
        let Some(job) = self.job else {
            return;
        };
        let connection = self.database.read(cx).connection();
        if self.summary.as_ref().is_none_or(|summary| summary.job != job) {
            self.summary = Application::read(connection, job).unwrap_or_else(|error| {
                eprintln!("job view: couldn't read the job: {error}");
                None
            });
        }
        if self.versions.as_ref().is_none_or(|versions| versions.job != job) {
            self.versions = Some(Versions::read(connection, job).unwrap_or_else(|error| {
                eprintln!("job view: couldn't read the job's résumés: {error}");
                Versions {
                    job,
                    current: None,
                    sent: None,
                }
            }));
        }
        let Some(shown) = self.shown_id() else {
            self.shown = None;
            self.inspection = None;
            return;
        };
        if self.shown.as_ref().is_none_or(|resume| resume.id != shown) {
            let resume = ShownResume::read(connection, shown).unwrap_or_else(|error| {
                eprintln!("job view: couldn't read the résumé: {error}");
                ShownResume {
                    id: shown,
                    lines: Vec::new(),
                    changes: Vec::new(),
                }
            });
            self.shown = Some(Rc::new(resume));
        }
        let read = self
            .inspection
            .as_ref()
            .is_some_and(|inspection| inspection.is_of(shown, self.selected));
        if !read && let Some(resume) = &self.shown {
            let inspection = Inspection::read(connection, resume, self.selected)
                .unwrap_or_else(|error| {
                    eprintln!("job view: couldn't read the inspector's facts: {error}");
                    Inspection::empty(shown, self.selected)
                });
            self.inspection = Some(Rc::new(inspection));
        }
    }

    fn render_header(&self, job: &Application, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_none()
            .justify_between()
            .gap_4()
            .pr_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(job_header(
                job,
                self.shown_version().map(|version| version.generated_at),
                cx,
            ))
            .child(
                TabBar::new("job-tabs")
                    .segmented()
                    .small()
                    .selected_index(match self.tab {
                        Tab::Resume => 0,
                        Tab::Application => 1,
                    })
                    .child(KitTab::new().label("Résumé"))
                    .child(KitTab::new().label("Application"))
                    .on_click(cx.listener(|this, index: &usize, window, cx| {
                        let tab = if *index == 0 {
                            Tab::Resume
                        } else {
                            Tab::Application
                        };
                        this.set_tab(tab, window, cx)
                    })),
            )
    }

    fn render_resume_tab(&self, render: Option<PageRender>, cx: &Context<Self>) -> AnyElement {
        let shown = self.shown.as_ref().filter(|resume| Some(resume.id) == self.shown_id());
        let version = self.shown_version();
        let showing_current = self.showing_sent.is_none();
        let sent = self.versions.as_ref().and_then(|versions| versions.sent.as_ref());
        h_resizable("job-view")
            .with_state(&self.layout)
            .child(
                resizable_panel().child(
                    v_flex()
                        .size_full()
                        .children(version.map(|version| {
                            template_bar(version.template, !showing_current, cx)
                        }))
                        .when_some(sent_notice(sent, self.showing_sent, cx), |this, notice| {
                            this.child(notice)
                        })
                        .when(showing_current, |this| {
                            this.when_some(shown.and_then(|resume| outdated_notice(resume, cx)), |this, notice| {
                                this.child(notice)
                            })
                        })
                        .child(self.render_resume(shown.map(Rc::as_ref), render, cx)),
                ),
            )
            .child(
                resizable_panel()
                    .size(INSPECTOR_WIDTH)
                    .size_range(INSPECTOR_MIN_WIDTH..INSPECTOR_MAX_WIDTH)
                    .children(shown.zip(self.inspection.as_ref()).map(|(resume, inspection)| {
                        Inspector::new(
                            self.database.clone(),
                            resume.clone(),
                            inspection.clone(),
                            showing_current,
                        )
                    })),
            )
            .into_any_element()
    }

    fn render_resume(
        &self,
        resume: Option<&ShownResume>,
        render: Option<PageRender>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(resume) = resume else {
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

        let markers = Markers {
            edited: resume
                .lines
                .iter()
                .filter(|line| line.is_edited())
                .map(|line| line.id)
                .collect(),
            outdated: resume
                .lines
                .iter()
                .filter(|line| resume.is_outdated(line))
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
                let middle = line_box.mark_middle * scale;
                let left = line_box.mark_left * scale;
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
                            .text_color(rgb(OUTDATED_MARKER)),
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

/// Pixels on screen per typographic point.
fn scale(page: PageSize) -> f32 {
    f32::from(PAGE_WIDTH) / page.width
}

/// Above the page, as it's about the page: which template lays the résumé
/// out, and a menu to pick another. Its content stays as it is, so switching
/// is only a new layout. A version that was sent keeps its template, so for
/// one it's only named.
fn template_bar(current: &'static Template, read_only: bool, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let picker = if read_only {
        div()
            .text_sm()
            .child(current.name)
            .into_any_element()
    } else {
        template_menu(current).into_any_element()
    };
    h_flex()
        .flex_none()
        .gap_2()
        .px_4()
        .py_1()
        .border_b_1()
        .border_color(theme.border)
        .child(
            Icon::new(IconName::LayoutDashboard)
                .small()
                .text_color(theme.muted_foreground),
        )
        .child(div().text_sm().text_color(theme.muted_foreground).child("Template"))
        .child(picker)
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(if read_only {
                    "Kept as it was sent"
                } else {
                    "Changes the look, not what it says"
                }),
        )
}

fn template_menu(current: &'static typeset::Template) -> impl IntoElement {
    let current = current.key;
    Button::new("template")
        .label(typeset::template(current).name)
        .dropdown_caret(true)
        .outline()
        .small()
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
            for template in &typeset::TEMPLATES {
                menu = menu.menu_with_check(
                    template.name,
                    template.key == current,
                    Box::new(UseTemplate { key: template.key }),
                );
            }
            menu
        })
        .into_any_element()
}

/// `generated_at` is the résumé version's on screen.
fn job_header(
    job: &Application,
    generated_at: Option<DateTime<Utc>>,
    cx: &Context<JobView>,
) -> impl IntoElement {
    let mut details = vec![
        job.company.clone(),
        application_view::status_label(job, Utc::now()),
    ];
    if let Some(generated_at) = generated_at {
        details.push(format!(
            "Generated {}",
            generated_at.with_timezone(&Local).format("%b %-d, %H:%M")
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

/// Says when the résumé sent is older than the current one, and switches
/// between them.
fn sent_notice(
    sent: Option<&Version>,
    showing_sent: Option<ResumeId>,
    cx: &Context<JobView>,
) -> Option<impl IntoElement> {
    let sent = sent?;
    let sent_id = sent.id;
    let theme = cx.theme();
    let generated = sent.generated_at.with_timezone(&Local).format("%b %-d");
    let (text, label) = match showing_sent {
        Some(_) => (
            format!(
                "Showing the version you sent, generated {generated}, in the {} template. \
                 It's read-only.",
                sent.template.name
            ),
            "Show current",
        ),
        None => (
            format!(
                "You sent an older version of this résumé, generated {generated}, before \
                 regenerating it."
            ),
            "Show the version sent",
        ),
    };
    Some(
        h_flex()
            .flex_none()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .text_sm()
            .child(Icon::new(IconName::Info).small().text_color(theme.muted_foreground))
            .child(div().flex_1().child(text))
            .child(
                Button::new("toggle-sent")
                    .label(label)
                    .outline()
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let current = this
                            .versions
                            .as_ref()
                            .and_then(|versions| versions.current.as_ref())
                            .map(|version| version.id);
                        match (this.showing_sent, current) {
                            (Some(_), Some(current)) => this.show_version(current, cx),
                            _ => this.show_version(sent_id, cx),
                        }
                    })),
            ),
    )
}

/// Shown when facts the résumé was generated from changed since.
fn outdated_notice(resume: &ShownResume, cx: &App) -> Option<impl IntoElement> {
    let changed = resume.changes.len();
    if changed == 0 {
        return None;
    }
    // A fact it left out, like a hidden one shown again, is on no line.
    let marked = resume.lines.iter().any(|line| resume.is_outdated(line));
    let mut text = if changed == 1 {
        "A fact this résumé was generated from has changed since.".to_string()
    } else {
        format!("{changed} facts this résumé was generated from have changed since.")
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

/// The job's résumé versions the view switches between.
struct Versions {
    job: JobId,
    current: Option<Version>,
    /// The version sent with the latest application, when it's no longer
    /// the current one.
    sent: Option<Version>,
}

struct Version {
    id: ResumeId,
    generated_at: DateTime<Utc>,
    template: &'static Template,
}

impl Versions {
    fn read(connection: &Connection, job: JobId) -> Result<Self, Error> {
        let version = |row: &rusqlite::Row| -> rusqlite::Result<Version> {
            Ok(Version {
                id: ResumeId(row.get(0)?),
                generated_at: DateTime::from_timestamp_millis(row.get(1)?).unwrap_or_default(),
                template: typeset::template_of_version(row.get(2)?),
            })
        };
        let current = connection
            .prepare_cached(
                "SELECT id, generated_at, template_version_id FROM generations
                 WHERE job_id = ?1 AND kind = 'resume' AND deleted_at IS NULL
                 ORDER BY generated_at DESC, id DESC LIMIT 1",
            )?
            .query_row([job.0], version)
            .optional()?;
        let sent = connection
            .prepare_cached(
                "SELECT id, generated_at, template_version_id FROM generations
                 WHERE id = (
                     SELECT resume_id FROM events
                     WHERE job_id = ?1 AND kind = 'applied' AND resume_id IS NOT NULL
                       AND deleted_at IS NULL
                     ORDER BY at DESC, id DESC LIMIT 1
                 )
                   AND job_id = ?1 AND kind = 'resume' AND deleted_at IS NULL",
            )?
            .query_row([job.0], version)
            .optional()?;
        Ok(Self {
            job,
            sent: sent.filter(|sent| Some(sent.id) != current.as_ref().map(|current| current.id)),
            current,
        })
    }
}

/// A version of the résumé as the view and the inspector show it.
pub struct ShownResume {
    pub id: ResumeId,
    /// In the order they're laid out.
    pub lines: Vec<ShownLine>,
    /// The facts it was generated from that changed since, and how.
    pub changes: Vec<(FactId, FactChange)>,
}

pub struct ShownLine {
    pub id: LineId,
    pub text: String,
    pub generated: String,
    /// The facts it cites, in order: for one merged into another since, that
    /// one.
    pub cites: Vec<FactId>,
}

impl ShownLine {
    pub fn is_edited(&self) -> bool {
        self.text != self.generated
    }
}

impl ShownResume {
    fn read(connection: &Connection, resume: ResumeId) -> Result<Self, Error> {
        let mut lines: Vec<ShownLine> = connection
            .prepare_cached(
                "SELECT lines.id, lines.text, lines.generated
                 FROM sections
                 JOIN blocks ON blocks.section_id = sections.id AND blocks.deleted_at IS NULL
                 JOIN lines ON lines.block_id = blocks.id AND lines.deleted_at IS NULL
                 WHERE sections.generation_id = ?1 AND sections.deleted_at IS NULL
                 ORDER BY sections.sort_key, blocks.sort_key, lines.sort_key",
            )?
            .query_map([resume.0], |row| {
                Ok(ShownLine {
                    id: LineId(row.get(0)?),
                    text: row.get(1)?,
                    generated: row.get(2)?,
                    cites: Vec::new(),
                })
            })?
            .collect::<Result<_, _>>()?;
        let citations = model::citations(connection, resume)?;
        for (line, fact) in citations {
            if let Some(line) = lines.iter_mut().find(|each| each.id == line) {
                line.cites.push(fact);
            }
        }
        Ok(Self {
            id: resume,
            lines,
            changes: changes(connection, resume)?,
        })
    }

    pub fn change(&self, fact: FactId) -> Option<FactChange> {
        self.changes
            .iter()
            .find(|(changed, _)| *changed == fact)
            .map(|(_, change)| *change)
    }

    /// Whether the line cites a fact that changed since it was generated.
    pub fn is_outdated(&self, line: &ShownLine) -> bool {
        line.cites.iter().any(|&fact| self.change(fact).is_some())
    }
}

/// How each fact the résumé was generated from differs from the version it
/// was generated from, if it does. A merged fact counts as the one it was
/// merged into.
fn changes(connection: &Connection, resume: ResumeId) -> Result<Vec<(FactId, FactChange)>, Error> {
    let versions: Vec<(FactId, bool, bool, bool)> = connection
        .prepare_cached(
            "WITH RECURSIVE chains(fact_id, root_id) AS (
                 SELECT fact_id, fact_id FROM fact_versions WHERE generation_id = ?1
                 UNION
                 SELECT chains.fact_id, facts.merged_into
                 FROM chains JOIN facts ON facts.id = chains.root_id
                 WHERE facts.merged_into IS NOT NULL
             )
             SELECT roots.id, fact_versions.hidden, roots.hidden,
                    fact_versions.text != roots.text
             FROM fact_versions
             JOIN chains ON chains.fact_id = fact_versions.fact_id
             JOIN facts AS roots ON roots.id = chains.root_id
                                AND roots.merged_into IS NULL AND roots.deleted_at IS NULL
             WHERE fact_versions.generation_id = ?1
             ORDER BY fact_versions.fact_id",
        )?
        .query_map([resume.0], |row| {
            Ok((FactId(row.get(0)?), row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut changes: Vec<(FactId, FactChange)> = Vec::new();
    for (fact, was_hidden, hidden, reworded) in versions {
        let change = match (was_hidden, hidden) {
            (false, true) => FactChange::Hidden,
            (true, false) => FactChange::Shown,
            _ if reworded => FactChange::Reworded,
            _ => continue,
        };
        if !changes.iter().any(|(changed, _)| *changed == fact) {
            changes.push((fact, change));
        }
    }
    Ok(changes)
}

/// Whether the change is to anything the view or its inspector shows.
fn shows(changed: &Changed) -> bool {
    changed.touches(Jobs)
        || changed.touches(Generations)
        || changed.touches(Sections)
        || changed.touches(Blocks)
        || changed.touches(Lines)
        || changed.touches(LineCitations)
        || changed.touches(FactVersions)
        || changed.touches(UnsupportedClaims)
        || changed.touches(Events)
        || changed.touches(Facts)
        || changed.touches(FactSources)
        || changed.touches(Documents)
        || changed.touches(DocumentTexts)
        || changed.touches(Entries)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Resume,
    Application,
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::database::{TemporaryDirectory, Write, Writer};
    use crate::model::{ClaimId, FactId, Library, TailoredResume, UnsupportedReason};
    use crate::workspace::parts::Source;
    use crate::workspace::testing::{self, write_sample};

    /// The queries read each job's versions, each version's lines and
    /// changes, and what the inspector shows of each line, as
    /// `Library::load` does.
    fn assert_queries_match_the_library(connection: &Connection) -> Result<(), Error> {
        let library = Library::load(connection)?;
        for job in &library.jobs {
            let versions = Versions::read(connection, job.id)?;
            assert_eq!(
                versions.current.as_ref().map(|version| version.id),
                job.current_resume().map(|resume| resume.id),
                "{}",
                job.company
            );
            assert_eq!(
                versions.sent.as_ref().map(|version| version.id),
                job.sent_older_resume().map(|resume| resume.id),
                "{}",
                job.company
            );
            for version in versions.current.iter().chain(&versions.sent) {
                let Some(resume) = job.resume(version.id) else {
                    panic!("{}: no such résumé", job.company);
                };
                assert_eq!(version.generated_at, resume.generated_at);
                assert_eq!(version.template.key, resume.template);
            }
            for resume in &job.resumes {
                assert_resume_matches(connection, &library, resume)?;
            }
        }
        Ok(())
    }

    fn assert_resume_matches(
        connection: &Connection,
        library: &Library,
        resume: &TailoredResume,
    ) -> Result<(), Error> {
        let shown = ShownResume::read(connection, resume.id)?;
        // As sets: the library reads a block's lines by role, the query by
        // their order on the page.
        let lines: BTreeSet<(LineId, &str, &str, &[FactId])> = shown
            .lines
            .iter()
            .map(|line| {
                (line.id, line.text.as_str(), line.generated.as_str(), line.cites.as_slice())
            })
            .collect();
        let expected: BTreeSet<(LineId, &str, &str, &[FactId])> = resume
            .lines()
            .map(|line| {
                (line.id, line.text.as_str(), line.generated.as_str(), line.cites.as_slice())
            })
            .collect();
        assert_eq!(lines, expected);
        let mut changes = shown.changes.clone();
        changes.sort_by_key(|(fact, _)| *fact);
        let expected: Vec<(FactId, FactChange)> = library
            .changed_facts(resume)
            .into_iter()
            .filter_map(|fact| Some((fact, library.fact_change(resume, fact)?)))
            .collect();
        assert_eq!(changes, expected);

        for line in resume.lines() {
            let inspection = Inspection::read(connection, &shown, Some(line.id))?;
            assert_eq!(
                inspection.facts.iter().map(|cited| cited.id).collect::<Vec<_>>(),
                line.cites
            );
            for cited in &inspection.facts {
                let expected = library.fact(cited.id);
                assert_eq!(cited.fact.is_some(), expected.is_some());
                let (Some(fact), Some(expected)) = (&cited.fact, expected) else {
                    continue;
                };
                assert_eq!(fact.text, expected.text);
                assert_eq!(fact.kind, expected.kind);
                assert_eq!(fact.hidden, expected.hidden);
                assert_eq!(fact.review, expected.review);
                assert_eq!(
                    fact.entry,
                    expected.entry.and_then(|entry| library.entry_label(entry))
                );
                let labels: Vec<&str> = fact.sources.iter().map(|source| source.label.as_str()).collect();
                let expected_labels: Vec<String> = expected
                    .sources
                    .iter()
                    .map(|source| Source::from_library(library, source).label)
                    .collect();
                assert_eq!(labels, expected_labels);
            }
        }

        let inspection = Inspection::read(connection, &shown, None)?;
        assert_eq!(inspection.claims.len(), resume.unsupported_claims.len());
        for (claim, expected) in inspection.claims.iter().zip(&resume.unsupported_claims) {
            assert_eq!(claim.id, expected.id);
            assert_eq!(claim.text, expected.text);
            assert_eq!(claim.reason, expected.reason);
            assert_eq!(claim.added_as, expected.added_as);
            let shown_again = match expected.reason {
                UnsupportedReason::UsesHiddenFact(fact) => {
                    library.fact(fact).is_some_and(|fact| !fact.hidden)
                }
                _ => false,
            };
            assert_eq!(claim.hidden_fact_shown, shown_again);
        }
        Ok(())
    }

    fn sample_writer() -> Result<Writer, Error> {
        let mut writer = Writer::open_in_memory()?;
        writer.execute(write_sample)?;
        Ok(writer)
    }

    fn fact(connection: &Connection, start: &str) -> Result<FactId, Error> {
        Ok(FactId(connection.query_row(
            "SELECT id FROM facts WHERE text LIKE ?1 || '%' ORDER BY id LIMIT 1",
            [start],
            |row| row.get(0),
        )?))
    }

    #[test]
    fn the_queries_read_the_sample_as_the_library_does() -> Result<(), Error> {
        assert_queries_match_the_library(sample_writer()?.kron())
    }

    #[test]
    fn the_queries_follow_changes_to_facts_and_claims_as_the_library_does() -> Result<(), Error> {
        let mut writer = sample_writer()?;
        let migration = fact(writer.kron(), "Led the migration")?;
        let linkedin = fact(writer.kron(), "Migrated 12 backend")?;
        let latency = fact(writer.kron(), "Cut p99 latency")?;
        let property_tests = fact(writer.kron(), "Introduced property-based tests")?;
        let claim: ClaimId = ClaimId(writer.kron().query_row(
            "SELECT id FROM unsupported_claims WHERE added_as IS NULL
             AND reason != 'uses_hidden_fact' ORDER BY id LIMIT 1",
            [],
            |row| row.get(0),
        )?);
        writer.execute(|write| commands::merge_facts(write, linkedin, &[migration]))?;
        writer.execute(|write| commands::edit_fact(write, latency, "Cut p99 latency by 80%."))?;
        writer.execute(|write| commands::set_hidden(write, property_tests, false))?;
        writer.execute(|write| commands::add_claim_as_fact(write, claim))?;
        writer.execute(|write| commands::set_hidden(write, latency, true))?;
        assert_queries_match_the_library(writer.kron())
    }

    #[test]
    fn a_job_without_a_resume_has_no_versions() -> Result<(), Error> {
        let writer = sample_writer()?;
        let job = JobId(writer.kron().query_row(
            "SELECT id FROM jobs WHERE company = 'Halcyon Cloud'",
            [],
            |row| row.get(0),
        )?);
        let versions = Versions::read(writer.kron(), job)?;
        assert!(versions.current.is_none() && versions.sent.is_none());
        Ok(())
    }

    // The view in a window, driven by keys, with the pages typeset.

    struct View<'a> {
        view: Entity<JobView>,
        database: Entity<Database>,
        cx: &'a mut VisualTestContext,
        closed: Rc<RefCell<usize>>,
        _directory: TemporaryDirectory,
    }

    fn open<'a>(app: &'a mut TestAppContext, company: &str) -> View<'a> {
        let (database, directory) = testing::open_sample(app);
        let library = database.read_with(app, |database, _| Library::load(database.connection()));
        let library = match library {
            Ok(library) => library,
            Err(error) => panic!("{error}"),
        };
        let job = library
            .jobs
            .iter()
            .find(|job| job.company == company)
            .map(|job| job.id);
        let Some(job) = job else {
            panic!("no job at {company}");
        };
        let previews = app.new(|cx| Previews::new(database.clone(), cx));
        let closed = Rc::new(RefCell::new(0));
        let (view, cx) = app.add_window_view(|window, cx| {
            let mut view = JobView::new(database.clone(), previews, window, cx);
            view.set_job(job, cx);
            view
        });
        cx.update(|window, cx| {
            view.focus_handle(cx).focus(window, cx);
            let closed = closed.clone();
            cx.subscribe(&view, move |_, _: &JobViewEvent, _| *closed.borrow_mut() += 1)
                .detach();
        });
        cx.run_until_parked();
        View {
            view,
            database,
            cx,
            closed,
            _directory: directory,
        }
    }

    impl View<'_> {
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

        fn read<R>(&mut self, read: impl FnOnce(&JobView) -> R) -> R {
            self.view.read_with(self.cx, |view, _| read(view))
        }

        fn update(&mut self, update: impl FnOnce(&mut JobView, &mut Context<JobView>)) {
            self.view.update(self.cx, update);
            self.settle();
        }

        fn selected(&mut self) -> Option<LineId> {
            self.read(|view| view.selected)
        }

        fn shown(&mut self) -> Rc<ShownResume> {
            match self.read(|view| view.shown.clone()) {
                Some(shown) => shown,
                None => panic!("no résumé shown"),
            }
        }

        fn inspection(&mut self) -> Rc<Inspection> {
            match self.read(|view| view.inspection.clone()) {
                Some(inspection) => inspection,
                None => panic!("nothing inspected"),
            }
        }

        fn current(&mut self) -> Option<ResumeId> {
            self.read(|view| {
                view.versions
                    .as_ref()
                    .and_then(|versions| versions.current.as_ref())
                    .map(|version| version.id)
            })
        }

        fn connection<R>(&mut self, read: impl FnOnce(&Connection) -> rusqlite::Result<R>) -> R {
            match self
                .database
                .read_with(self.cx, |database, _| read(database.connection()))
            {
                Ok(value) => value,
                Err(error) => panic!("{error}"),
            }
        }

        fn fact(&mut self, start: &str) -> FactId {
            FactId(self.connection(|connection| {
                connection.query_row(
                    "SELECT id FROM facts WHERE text LIKE ?1 || '%' ORDER BY id LIMIT 1",
                    [start],
                    |row| row.get(0),
                )
            }))
        }
    }

    #[test]
    fn the_view_reads_the_current_resume_and_its_pages() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        let shown = view.shown();
        assert_eq!(Some(shown.id), view.current());
        assert!(!shown.lines.is_empty());
        assert!(view.read(|view| view.pages.is_some()), "typeset");
        assert!(view.inspection().claims.len() == 2, "Ledgerly's two unsupported claims");
    }

    #[test]
    fn arrow_keys_move_through_the_lines_as_laid_out_and_stop_at_the_ends() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        let order = view.read(|view| {
            view.pages
                .as_ref()
                .map(|pages| pages.layout.reading_order())
                .unwrap_or_default()
        });
        assert!(order.len() > 3);
        view.keys("down");
        assert_eq!(view.selected(), Some(order[0]));
        view.keys("down down");
        assert_eq!(view.selected(), Some(order[2]));
        view.keys("up");
        assert_eq!(view.selected(), Some(order[1]));
        view.keys("up up up");
        assert_eq!(view.selected(), Some(order[0]), "stays at the first");
        let inspection = view.inspection();
        assert_eq!(inspection.line, Some(order[0]));
        assert!(!inspection.facts.is_empty(), "the inspector shows its facts");
    }

    #[test]
    fn escape_clears_the_selection_then_leaves() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        view.keys("down");
        view.keys("escape");
        assert_eq!(view.selected(), None);
        assert_eq!(*view.closed.borrow(), 0);
        view.keys("escape");
        assert_eq!(*view.closed.borrow(), 1);
    }

    #[test]
    fn a_fact_changed_elsewhere_marks_its_lines_and_shows_in_the_inspector() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        let latency = view.fact("Cut p99 latency");
        let line = view
            .shown()
            .lines
            .iter()
            .find(|line| line.cites.contains(&latency))
            .map(|line| line.id);
        let Some(line) = line else {
            panic!("no Ledgerly line cites the latency fact");
        };
        assert!(view.shown().changes.is_empty());
        view.update(|view, cx| view.select_line(line, cx));

        view.run(move |write| commands::set_hidden(write, latency, true));
        let shown = view.shown();
        assert_eq!(shown.change(latency), Some(FactChange::Hidden));
        assert!(shown.lines.iter().any(|each| each.id == line && shown.is_outdated(each)));
        let inspection = view.inspection();
        let hidden = inspection
            .facts
            .iter()
            .find(|cited| cited.id == latency)
            .and_then(|cited| cited.fact.as_ref())
            .map(|fact| fact.hidden);
        assert_eq!(hidden, Some(true));

        view.run(move |write| commands::set_hidden(write, latency, false));
        assert!(view.shown().changes.is_empty(), "showing it again undoes it");
    }

    #[test]
    fn merging_a_cited_fact_cites_the_one_kept() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        let migration = view.fact("Led the migration");
        let linkedin = view.fact("Migrated 12 backend");
        let citing: Vec<LineId> = view
            .shown()
            .lines
            .iter()
            .filter(|line| line.cites.contains(&migration))
            .map(|line| line.id)
            .collect();
        assert!(!citing.is_empty());
        view.run(move |write| commands::merge_facts(write, linkedin, &[migration]).map(drop));
        let shown = view.shown();
        for line in shown.lines.iter().filter(|line| citing.contains(&line.id)) {
            assert!(line.cites.contains(&linkedin));
            assert!(!line.cites.contains(&migration));
        }
    }

    #[test]
    fn a_template_switch_shows_in_the_template_bar() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        view.cx.dispatch_action(UseTemplate { key: "sidebar" });
        view.settle();
        let template = view.read(|view| view.shown_version().map(|version| version.template.key));
        assert_eq!(template, Some("sidebar"));
    }

    #[test]
    fn a_claim_added_as_a_fact_says_so() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        let claim = view
            .inspection()
            .claims
            .iter()
            .find(|claim| claim.added_as.is_none() && !matches!(claim.reason, UnsupportedReason::UsesHiddenFact(_)))
            .map(|claim| claim.id);
        let Some(claim) = claim else {
            panic!("no claim to add");
        };
        view.run(move |write| commands::add_claim_as_fact(write, claim).map(drop));
        let added = view
            .inspection()
            .claims
            .iter()
            .find(|each| each.id == claim)
            .and_then(|claim| claim.added_as);
        assert!(added.is_some());
    }

    #[test]
    fn a_claim_needing_a_hidden_fact_is_resolved_once_its_shown() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        let hidden = view.inspection().claims.iter().find_map(|claim| match claim.reason {
            UnsupportedReason::UsesHiddenFact(fact) => Some((claim.id, fact)),
            _ => None,
        });
        let Some((claim, fact)) = hidden else {
            panic!("Ledgerly has no claim needing a hidden fact");
        };
        view.run(move |write| commands::set_hidden(write, fact, false));
        let shown_again = view
            .inspection()
            .claims
            .iter()
            .find(|each| each.id == claim)
            .map(|claim| claim.hidden_fact_shown);
        assert_eq!(shown_again, Some(true));
    }

    #[test]
    fn the_version_sent_shows_instead_and_back() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        let sent = view.read(|view| {
            view.versions
                .as_ref()
                .and_then(|versions| versions.sent.as_ref())
                .map(|sent| sent.id)
        });
        let Some(sent) = sent else {
            panic!("Ledgerly sent no older version");
        };
        view.keys("down");
        view.update(|view, cx| view.show_version(sent, cx));
        assert_eq!(view.shown().id, sent);
        assert_eq!(view.selected(), None, "a new version starts with no selection");
        assert!(view.read(|view| view.pages.is_some()), "typeset");
        let current = view.current();
        if let Some(current) = current {
            view.update(|view, cx| view.show_version(current, cx));
            assert_eq!(view.shown().id, current);
            assert_eq!(view.read(|view| view.showing_sent), None);
        }
    }

    #[test]
    fn a_job_without_a_resume_shows_none() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Halcyon Cloud");
        assert!(view.read(|view| view.shown.is_none()));
        view.keys("down");
        assert_eq!(view.selected(), None);
    }
}

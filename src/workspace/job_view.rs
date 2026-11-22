use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use super::SetUpAi;
use super::application_view::{self, ApplicationView, ApplicationViewEvent};
use super::inspector::{Inspection, Inspector, LINE_EDITOR_CONTEXT};
use super::instructions_form::{InstructionsForm, InstructionsFormEvent};
use super::parts::{call_failed, notify};
use super::previews::{self, Previews, Render as PageRender, RenderedPages};
use crate::database::schema::{
    Blocks, DocumentTexts, Documents, Entries, Events, FactSources, FactVersions, Facts,
    Generations, Jobs, LineCitations, Lines, Sections, Settings as SettingsTable,
    UnsupportedClaims,
};
use crate::database::{Changed, Database, Error};
use crate::llm::{self, Purpose};
use crate::model::generation::{self, Draft, Material};
use crate::model::meaning_check;
use crate::model::redaction::{self, Redaction};
use crate::model::settings;
use crate::model::{
    self, Application, FactChange, FactId, JobId, LineId, ResumeId, commands, verifier,
};
use crate::typeset::{self, LineBox, PageSize, Paper, Template, WordBox};
use chrono::{DateTime, Local, Utc};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{ResizableState, h_resizable, resizable_panel};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab as KitTab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable, WindowExt, h_flex, v_flex,
};
use gpui_kit::{
    Action, Anchor, AnyElement, App, Bounds, Context, Div, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, HighlightStyle, Hsla, IntoElement, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, RenderImage, ScrollHandle, Stateful, StyledText,
    Subscription, Task, Window, actions, canvas, div, img, point, prelude::*, px, rgb,
};
use rusqlite::{Connection, OptionalExtension};

const KEY_CONTEXT: &str = "JobView";
/// Pages are shown this wide: a bit over US Letter at 96 dpi.
const PAGE_WIDTH: Pixels = px(640.);
/// The zoom steps, as a share of `PAGE_WIDTH`.
const ZOOMS: [f32; 9] = [0.5, 0.67, 0.75, 0.9, 1., 1.1, 1.25, 1.5, 2.];
/// Between pages, and above the first and below the last.
const PAGE_GAP: f32 = 24.;
const INSPECTOR_WIDTH: Pixels = px(340.);
const INSPECTOR_MIN_WIDTH: Pixels = px(280.);
const INSPECTOR_MAX_WIDTH: Pixels = px(560.);
const INSTRUCTIONS_FORM_WIDTH: f32 = 520.;
/// Keep this much room around a line scrolled into view.
const SCROLL_MARGIN: f32 = 48.;

// Drawn on the page, which is paper and stays light in both themes.
const PAPER: u32 = 0xfafafa;
const SELECTION: u32 = 0x497858;
const EDITED_MARKER: u32 = 0x497858;
const OUTDATED_MARKER: u32 = 0xbf8700;
/// Words a line has that its facts don't: a highlight, underlined darker.
const REWORDED: u32 = 0xf5c542;
const REWORDED_UNDERLINE: u32 = 0xb8860b;

actions!(
    job_view,
    [
        SelectNextLine,
        SelectPreviousLine,
        Back,
        ShowResumeTab,
        ShowApplicationTab,
        EditLine,
        CancelLineEdit,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        FitWidth
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", SelectNextLine, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectPreviousLine, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-1", ShowResumeTab, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-2", ShowApplicationTab, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", EditLine, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-=", ZoomIn, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-+", ZoomIn, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary--", ZoomOut, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-0", ResetZoom, Some(KEY_CONTEXT)),
        // Esc in a text box does nothing of its own.
        KeyBinding::new(
            "escape",
            CancelLineEdit,
            Some(&format!("{LINE_EDITOR_CONTEXT} > Input")),
        ),
    ]);
    application_view::init(cx);
}

/// Lays the current résumé out with one of `typeset::TEMPLATES`.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = job_view, no_json)]
struct UseTemplate {
    key: &'static str,
}

/// Lays the current résumé out on another paper.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = job_view, no_json)]
struct UsePaper {
    paper: Paper,
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
    /// The selected line's text, to edit in the inspector.
    line_editor: Entity<TextareaState>,
    /// The line `line_editor` holds.
    editing: Option<LineId>,
    hovered: Option<LineId>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    /// The inspector's width.
    layout: Entity<ResizableState>,
    /// The pages' width as a share of `PAGE_WIDTH`.
    zoom: f32,
    /// The pages as last drawn, to map clicks to lines and scroll to them.
    pages: Option<Arc<RenderedPages>>,
    /// Where each page was drawn in the window, written during paint.
    page_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    /// The selection was made from elsewhere; scroll to it once the pages
    /// are ready.
    scroll_pending: bool,
    /// Writing a résumé with the model, which runs on while the user looks
    /// elsewhere.
    generating: bool,
    /// Says how to read a résumé until the user has selected a line once, or
    /// closed the note: saved, so it lasts until they have.
    explain_lines: bool,
    generation: Task<()>,
    /// Saving the résumé shown as a PDF, from asking where to writing it.
    export: Task<()>,
    /// The form asking for instructions before generating's, while it's open.
    _instructions_form: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<JobViewEvent> for JobView {}

impl Render for JobView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.read(cx);
        self.sync_line_editor(window, cx);
        let pixels_per_point = previews::pixels_per_point(self.page_width(), window);
        let shown = self.shown_id();
        let render = shown
            .and_then(|resume| {
                self.previews.update(cx, |previews, cx| {
                    previews.get(resume, pixels_per_point, cx)
                })
            })
            // Zoomed, until the pages render at the new size: the last ones,
            // stretched, rather than a spinner.
            .or_else(|| self.pages.clone().map(PageRender::Ready));
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
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.step_zoom(true, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.step_zoom(false, cx)))
            .on_action(cx.listener(|this, _: &ResetZoom, _, cx| this.set_zoom(1., cx)))
            .on_action(cx.listener(|this, _: &FitWidth, _, cx| this.fit_width(cx)))
            .on_action(cx.listener(Self::edit_line))
            .on_action(cx.listener(Self::cancel_line_edit))
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
            .on_action(cx.listener(|this, action: &UsePaper, _, cx| {
                if let Some(job) = this.job {
                    let paper = action.paper;
                    this.database
                        .read(cx)
                        .queue(move |write| commands::set_paper(write, job, paper));
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
        let application = cx.new(|cx| ApplicationView::new(database.clone(), window, cx));
        let explain_lines = !lines_explained(&database, cx);
        let line_editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 10)
                .submit_on_enter(true)
        });
        Self {
            _subscriptions: vec![
                cx.subscribe_in(
                    &line_editor,
                    window,
                    |this, _, event, window, cx| match event {
                        InputEvent::PressEnter { .. } => {
                            this.save_line(cx);
                            this.focus_handle.focus(window, cx);
                        }
                        InputEvent::Blur => this.save_line(cx),
                        InputEvent::Change | InputEvent::Focus => {}
                    },
                ),
                cx.subscribe(&database, |this, database, changed: &Changed, cx| {
                    if changed.touches(SettingsTable) {
                        this.explain_lines = !lines_explained(&database, cx);
                        cx.notify();
                    }
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
                cx.subscribe_in(
                    &application,
                    window,
                    |this, _, event, window, cx| match event {
                        ApplicationViewEvent::ShowResume(resume) => {
                            this.show_version(*resume, cx);
                            this.set_tab(Tab::Resume, window, cx);
                        }
                    },
                ),
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
            line_editor,
            editing: None,
            hovered: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            layout: cx.new(|_| ResizableState::default()),
            zoom: 1.,
            pages: None,
            page_bounds: Rc::default(),
            scroll_pending: false,
            generating: false,
            explain_lines,
            generation: Task::ready(()),
            export: Task::ready(()),
            _instructions_form: None,
        }
    }

    pub fn job(&self) -> Option<JobId> {
        self.job
    }

    #[cfg(test)]
    pub fn database(&self) -> &Entity<Database> {
        &self.database
    }

    #[cfg(test)]
    pub fn explains_lines(&self) -> bool {
        self.explain_lines
    }

    /// Writes the job's résumé, the user's first, without asking for
    /// instructions.
    pub fn write_first_resume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generate(None, window, cx);
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
        self.lines_now_explained(cx);
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

    /// Enter on a line of the current résumé: puts the cursor in its text.
    fn edit_line(&mut self, _: &EditLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == Tab::Resume && self.showing_sent.is_none() && self.selected.is_some() {
            self.line_editor
                .update(cx, |editor, cx| editor.focus(window, cx));
        }
    }

    /// Esc while editing a line: puts its text back and leaves the editor.
    fn cancel_line_edit(
        &mut self,
        _: &CancelLineEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.editing.and_then(|line| self.line_text(line)) {
            self.line_editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
        }
        self.focus_handle.focus(window, cx);
    }

    fn line_text(&self, line: LineId) -> Option<String> {
        self.shown
            .as_ref()?
            .lines
            .iter()
            .find(|each| each.id == line)
            .map(|line| line.text.clone())
    }

    /// Loads the selected line into the editor when the selection changes,
    /// saving what was typed for the one before, or when its text changed
    /// elsewhere while the editor wasn't in use.
    fn sync_line_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((line, text)) = self
            .selected
            .zip(self.selected.and_then(|line| self.line_text(line)))
        else {
            return;
        };
        let editor = self.line_editor.read(cx);
        let focused = editor.focus_handle(cx).is_focused(window);
        if self.editing != Some(line) {
            self.save_line(cx);
        } else if focused || editor.value() == text {
            return;
        }
        self.editing = Some(line);
        self.line_editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
    }

    fn save_line(&mut self, cx: &mut Context<Self>) {
        let Some(line) = self.editing else {
            return;
        };
        let text = self.line_editor.read(cx).value().trim().to_string();
        if text.is_empty() || self.line_text(line).is_none_or(|current| current == text) {
            return;
        }
        self.database
            .read(cx)
            .queue(move |write| commands::edit_line(write, line, &text));
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
            self.lines_now_explained(cx);
            cx.notify();
        }
    }

    fn page_width(&self) -> Pixels {
        PAGE_WIDTH * self.zoom
    }

    /// Pixels on screen per typographic point.
    fn scale(&self, page: PageSize) -> f32 {
        f32::from(self.page_width()) / page.width
    }

    /// To the next zoom step up or down from the current zoom, which may
    /// be between steps after fitting the width.
    fn step_zoom(&mut self, up: bool, cx: &mut Context<Self>) {
        let next = if up {
            ZOOMS.iter().find(|zoom| **zoom > self.zoom + 0.01)
        } else {
            ZOOMS.iter().rev().find(|zoom| **zoom < self.zoom - 0.01)
        };
        if let Some(&zoom) = next {
            self.set_zoom(zoom, cx);
        }
    }

    /// The pages as wide as the space beside the inspector, within the
    /// zoom steps.
    fn fit_width(&mut self, cx: &mut Context<Self>) {
        let width = f32::from(self.scroll_handle.bounds().size.width);
        if width <= 0. {
            return;
        }
        let fitted = (width - PAGE_GAP * 2.) / f32::from(PAGE_WIDTH);
        // Whole percents, so the same width gives the same render to reuse.
        let fitted = (fitted * 100.).floor() / 100.;
        self.set_zoom(fitted.clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]), cx);
    }

    /// Keeps the same part of the page in view as it grows or shrinks.
    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        if zoom == self.zoom {
            return;
        }
        let ratio = zoom / self.zoom;
        let offset = self.scroll_handle.offset();
        self.scroll_handle
            .set_offset(point(offset.x * ratio, offset.y * ratio));
        self.zoom = zoom;
        cx.notify();
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
            .map(|page| page.height * self.scale(*page) + PAGE_GAP)
            .sum();
        let page_scale = pages
            .layout
            .pages
            .get(first.page)
            .map_or(1., |page| self.scale(*page));
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
        let scale = self.scale(size);
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
        if self
            .summary
            .as_ref()
            .is_none_or(|summary| summary.job != job)
        {
            self.summary = Application::read(connection, job).unwrap_or_else(|error| {
                eprintln!("job view: couldn't read the job: {error}");
                None
            });
        }
        if self
            .versions
            .as_ref()
            .is_none_or(|versions| versions.job != job)
        {
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
            let inspection =
                Inspection::read(connection, resume, self.selected).unwrap_or_else(|error| {
                    eprintln!("job view: couldn't read the inspector's facts: {error}");
                    Inspection::empty(shown, self.selected)
                });
            self.inspection = Some(Rc::new(inspection));
        }
    }

    fn render_header(&self, job: &Application, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_none()
            .flex_wrap()
            .justify_between()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(job_header(job, self.shown_version(), cx))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
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
                    .when(self.shown_id().is_some(), |this| {
                        this.child(self.generate_button(cx)).child(
                            Button::new("export-pdf")
                                .label("Export PDF")
                                .icon(IconName::ArrowDown)
                                .small()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.export(window, cx)),
                                ),
                        )
                    }),
            )
    }

    /// Asks where to save the résumé shown, as it's shown, and saves it there
    /// as a PDF, laid out by the same Typst compile as the page on screen.
    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(resume) = self.shown_id() else {
            return;
        };
        let Some(typesetter) = self.previews.read(cx).typesetter() else {
            notify(
                Notification::info("The fonts are still loading. Try again in a moment."),
                window,
                cx,
            );
            return;
        };
        let connection = self.database.read(cx).connection();
        let read = || -> Result<_, Error> {
            let name: Option<String> = connection
                .query_row(
                    "SELECT lines.text FROM lines
                     JOIN blocks ON blocks.id = lines.block_id
                     JOIN sections ON sections.id = blocks.section_id
                     WHERE sections.generation_id = ?1 AND lines.role = 'name'
                         AND lines.deleted_at IS NULL",
                    [resume.0],
                    |row| row.get(0),
                )
                .optional()?;
            Ok((previews::read_markup(connection, resume)?, name))
        };
        let (markup, name) = match read() {
            Ok((Some(markup), name)) => (markup, name),
            Ok((None, _)) => return,
            Err(error) => {
                eprintln!("job view: couldn't read the résumé to export: {error}");
                notify(Notification::error("Couldn't read the résumé."), window, cx);
                return;
            }
        };
        let company = self.summary.as_ref().map(|job| job.company.clone());
        let file_name = pdf_name(name.as_deref(), company.as_deref());
        let directory = dirs::document_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&directory, Some(&file_name));
        self.export = cx.spawn_in(window, async move |this, cx| {
            let path = match chosen.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    eprintln!("job view: the save dialog failed: {error}");
                    return;
                }
                Err(error) => {
                    eprintln!("job view: the save dialog closed: {error}");
                    return;
                }
            };
            let saved = cx
                .background_spawn(async move {
                    let pdf = typesetter.pdf(&markup)?;
                    std::fs::write(&path, pdf).map_err(|error| error.to_string())?;
                    Ok::<_, String>(path)
                })
                .await;
            let message = match saved {
                Ok(path) => {
                    let shown = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                    Notification::success(format!("Saved {shown}."))
                }
                Err(error) => {
                    eprintln!("job view: couldn't export the résumé: {error}");
                    Notification::error(format!("Couldn't save the PDF: {error}."))
                }
            };
            if let Err(error) = this.update_in(cx, |_, window, cx| notify(message, window, cx)) {
                eprintln!("job view: {error}");
            }
        });
    }

    /// "Generate", or "Regenerate" over a résumé there already.
    fn generate_button(&self, cx: &Context<Self>) -> impl IntoElement {
        let label = match (self.generating, self.shown_id().is_some()) {
            (true, _) => "Writing…",
            (false, true) => "Regenerate",
            (false, false) => "Generate a résumé",
        };
        Button::new("generate")
            .label(label)
            .small()
            .when(self.shown_id().is_none(), |this| this.primary())
            .disabled(self.generating)
            .on_click(cx.listener(|this, _, window, cx| this.ask_to_generate(window, cx)))
    }

    /// Shown when facts the résumé was generated from changed since, with the
    /// way to bring it up to date beside it.
    fn outdated_notice(
        &self,
        resume: &ShownResume,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement> {
        let changed = resume.changes.len();
        if changed == 0 {
            return None;
        }
        let mut text = if changed == 1 {
            "A fact this résumé used has changed.".to_string()
        } else {
            format!("{changed} facts this résumé used have changed.")
        };
        // A fact it left out, like a hidden one shown again, is on no line.
        if resume.lines.iter().any(|line| resume.is_outdated(line)) {
            text.push_str(" Marked lines may be out of date.");
        }
        let theme = cx.theme();
        Some(
            h_flex()
                .id("outdated-notice")
                .test_support()
                .flex_none()
                .gap_3()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.warning.opacity(0.08))
                .text_sm()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(theme.warning),
                )
                // Wraps rather than running under the inspector.
                .child(
                    div()
                        .id("outdated-text")
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .child(text),
                )
                .child(
                    Button::new("regenerate-outdated")
                        .label("Regenerate")
                        .outline()
                        .small()
                        .disabled(self.generating)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.ask_to_generate(window, cx)),
                        ),
                ),
        )
    }

    /// Saves that the user knows how to read a résumé, the first time.
    fn lines_now_explained(&mut self, cx: &mut Context<Self>) {
        if self.explain_lines {
            self.explain_lines = false;
            self.database
                .read(cx)
                .queue(settings::set_resume_lines_explained);
        }
    }

    /// How to read a résumé: where its lines come from, and why some were
    /// left out.
    fn lines_notice(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .flex_none()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.primary.opacity(0.06))
            .text_sm()
            .child(
                Icon::new(IconName::Info)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(div().flex_1().min_w_0().child(
                "Click any line to see the facts it came from. Left out lists what your \
                 facts don't back, so it isn't on the page.",
            ))
            .child(
                Button::new("close-lines-notice")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.lines_now_explained(cx);
                        cx.notify();
                    })),
            )
    }

    /// Asks for anything to steer the résumé, starting from what the last was
    /// asked, then writes it.
    fn ask_to_generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.job else {
            return;
        };
        let last = generation::last_instructions(self.database.read(cx).connection(), job)
            .unwrap_or_else(|error| {
                eprintln!("job view: couldn't read the last instructions: {error}");
                None
            });
        let form = cx.new(|cx| InstructionsForm::new(last, window, cx));
        self._instructions_form =
            Some(
                cx.subscribe_in(&form, window, |this, _, event, window, cx| {
                    window.close_dialog(cx);
                    this._instructions_form = None;
                    if let InstructionsFormEvent::Generate(instructions) = event {
                        this.generate(instructions.clone(), window, cx);
                    }
                }),
            );
        let title = match self.shown_id() {
            Some(_) => "Regenerate the résumé",
            None => "Generate a résumé",
        };
        window.open_dialog(cx, {
            let form = form.clone();
            move |dialog, _, cx| {
                let form = form.clone();
                dialog
                    .title(title)
                    .bg(cx.theme().popover)
                    .w(px(INSTRUCTIONS_FORM_WIDTH))
                    .content(move |content, _, _| content.child(form.clone()))
            }
        });
        window.defer(cx, move |window, cx| {
            form.update(cx, |form, cx| form.focus(window, cx))
        });
    }

    /// Writes a résumé for the job from the user's facts and `instructions`
    /// with the model set up for it, checks each line, and saves it as the
    /// job's current one. It runs on while the user looks elsewhere, and a
    /// notification says how it went.
    fn generate(
        &mut self,
        instructions: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(job) = self.job else {
            return;
        };
        if self.generating {
            return;
        }
        let database = self.database.read(cx);
        let assigned = match llm::assigned(database.device_connection(), Purpose::GenerateResume) {
            Ok(Some(assigned)) => assigned,
            Ok(None) => {
                notify(
                    Notification::warning("To write a résumé, set up an AI provider.").action(
                        |_, _, cx| {
                            Button::new("set-up-ai")
                                .label("Set up AI")
                                .small()
                                .on_click(cx.listener(|notification, _, window, cx| {
                                    notification.dismiss(window, cx);
                                    window.dispatch_action(Box::new(SetUpAi), cx);
                                }))
                        },
                    ),
                    window,
                    cx,
                );
                return;
            }
            Err(error) => {
                eprintln!("job view: couldn't read the AI provider: {error}");
                return;
            }
        };
        let connection = database.connection();
        let read = || -> Result<_, Error> {
            let posting: (String, String, Option<String>) = connection.query_row(
                "SELECT jobs.title, jobs.company, job_descriptions.text FROM jobs
                 LEFT JOIN job_descriptions ON job_descriptions.job_id = jobs.id
                 WHERE jobs.id = ?1",
                [job.0],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let names = redaction::names(connection)?;
            let stand_in = settings::stand_in_name(connection)?;
            let paper = generation::paper(connection)?;
            Ok((posting, Material::read(connection)?, names, stand_in, paper))
        };
        let ((title, company, description), material, names, stand_in, paper) = match read() {
            Ok(read) => read,
            Err(error) => {
                eprintln!("job view: couldn't read what a résumé is written from: {error}");
                notify(
                    Notification::error("Couldn't read your facts to write from."),
                    window,
                    cx,
                );
                return;
            }
        };
        if material.is_empty() {
            notify(
                Notification::warning("Import your CV first: there are no facts to write from."),
                window,
                cx,
            );
            return;
        }
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let sent = material.sent(instructions.as_deref());
        let redaction = Redaction::new(&sent, &redaction::find(&sent, &names, &[]), &stand_in);
        let description = description.unwrap_or_default();
        let request = generation::request(
            &redaction.text,
            &generation::Job {
                title: &title,
                company: &company,
                description: &description,
            },
        );
        let started_at = Utc::now().timestamp_millis();
        let start = Instant::now();
        let call = llm::complete(&assigned, request, cx);
        let database = self.database.clone();
        self.generating = true;
        cx.notify();
        self.generation = cx.spawn_in(window, async move |this, cx| {
            let reply = call.await;
            let mut draft = match &reply {
                Ok(reply) => generation::Answer::parse(&reply.response.text())
                    .map(|answer| {
                        let ad = generation::Job {
                            title: &title,
                            company: &company,
                            description: &description,
                        };
                        Draft::new(&answer, &material, &sent, &redaction, &ad)
                    })
                    .map_err(|error| {
                        eprintln!("job view: the model's answer isn't the JSON asked for: {error}");
                        "the model's answer couldn't be read. Try again.".to_owned()
                    }),
                Err(error) => Err(error.to_string()),
            };
            // The meaning check, on the lines the rules passed. It only takes
            // lines off: when it fails, they stay as the rules left them.
            let check = match draft
                .as_ref()
                .ok()
                .filter(|draft| !draft.checked.is_empty())
            {
                Some(checked) => {
                    let request = meaning_check::request(&checked.checked);
                    match cx.update(|_, cx| llm::complete(&assigned, request, cx)) {
                        Ok(call) => Some(call.await),
                        Err(error) => {
                            eprintln!("job view: {error}");
                            None
                        }
                    }
                }
                None => None,
            };
            let check = match (check, &mut draft) {
                (Some(Ok(check)), Ok(draft)) => {
                    match meaning_check::flags(&check.response.text(), draft.checked.len()) {
                        Ok(flags) => draft.leave_out(&flags, &redaction, &sent),
                        Err(error) => eprintln!(
                            "job view: the meaning check's answer couldn't be read: {error}"
                        ),
                    }
                    Some(check)
                }
                (Some(Err(error)), _) => {
                    eprintln!("job view: the meaning check didn't run: {error}");
                    None
                }
                _ => None,
            };
            let duration = start.elapsed();
            let usage_limit = reply.as_ref().is_err_and(llm::Error::is_usage_limit);
            let claims = draft.as_ref().map_or(0, |draft| draft.claims.len());
            let cost = reply
                .as_ref()
                .ok()
                .and_then(|reply| reply.cost)
                .map(|cost| cost + check.as_ref().and_then(|check| check.cost).unwrap_or(0.));
            let saved = cx.update(|_, cx| {
                database.read(cx).write(cx, move |write| {
                    let reply = reply.ok();
                    let resume = match &draft {
                        Ok(draft) => Some(generation::save(
                            write,
                            job,
                            draft,
                            paper,
                            instructions.as_deref(),
                        )?),
                        Err(_) => None,
                    };
                    llm::record(
                        write,
                        &llm::Run {
                            purpose: Purpose::GenerateResume,
                            assigned: &assigned,
                            started_at,
                            duration,
                            reply: reply.as_ref(),
                            check: check.as_ref(),
                            done: resume.is_some(),
                            document: None,
                            job: Some(job.0),
                            generation: resume.map(|resume| resume.0),
                        },
                    )?;
                    Ok(draft.map(|_| ()))
                })
            });
            let saved = match saved {
                Ok(saved) => saved.await,
                Err(error) => {
                    eprintln!("job view: {error}");
                    return;
                }
            };
            let message = match saved {
                Ok(Ok(())) => {
                    let mut message = match claims {
                        0 => "Wrote the résumé.".to_owned(),
                        1 => "Wrote the résumé. 1 claim your facts don't back was left out."
                            .to_owned(),
                        claims => format!(
                            "Wrote the résumé. {claims} claims your facts don't back were left out."
                        ),
                    };
                    if let Some(cost) = cost {
                        message.push_str(&format!(" It cost {}.", llm::cost_label(cost)));
                    }
                    Notification::success(message)
                }
                Ok(Err(problem)) => {
                    call_failed(format!("Couldn't write the résumé: {problem}"), usage_limit)
                }
                Err(error) => {
                    eprintln!("job view: couldn't save the résumé: {error}");
                    Notification::error("Couldn't save the résumé.")
                }
            };
            if let Err(error) = this.update_in(cx, |this, window, cx| {
                this.generating = false;
                notify(message, window, cx);
                cx.notify();
            }) {
                eprintln!("job view: {error}");
            }
        });
    }

    fn render_resume_tab(&self, render: Option<PageRender>, cx: &Context<Self>) -> AnyElement {
        if self.shown_id().is_none() {
            return self.render_resume(None, render, None, cx);
        }
        let shown = self
            .shown
            .as_ref()
            .filter(|resume| Some(resume.id) == self.shown_id());
        let version = self.shown_version();
        let showing_current = self.showing_sent.is_none();
        let sent = self
            .versions
            .as_ref()
            .and_then(|versions| versions.sent.as_ref());
        h_resizable("job-view")
            .with_state(&self.layout)
            .child(
                resizable_panel().child(
                    v_flex()
                        .size_full()
                        .when_some(sent_notice(sent, self.showing_sent, cx), |this, notice| {
                            this.child(notice)
                        })
                        .when(showing_current, |this| {
                            this.when_some(
                                shown.and_then(|resume| self.outdated_notice(resume, cx)),
                                |this, notice| this.child(notice),
                            )
                            .when(self.explain_lines && shown.is_some(), |this| {
                                this.child(self.lines_notice(cx))
                            })
                        })
                        .child(self.render_resume(
                            shown.map(Rc::as_ref),
                            render,
                            version.map(|version| page_menus(version, !showing_current, cx)),
                            cx,
                        )),
                ),
            )
            .child(
                resizable_panel()
                    .size(INSPECTOR_WIDTH)
                    .size_range(INSPECTOR_MIN_WIDTH..INSPECTOR_MAX_WIDTH)
                    .children(
                        shown
                            .zip(self.inspection.as_ref())
                            .map(|(resume, inspection)| {
                                Inspector::new(
                                    self.database.clone(),
                                    resume.clone(),
                                    inspection.clone(),
                                    showing_current.then(|| self.line_editor.clone()),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    /// `menus` float over the pages' top left corner, and the zoom over the
    /// bottom right.
    fn render_resume(
        &self,
        resume: Option<&ShownResume>,
        render: Option<PageRender>,
        menus: Option<AnyElement>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(resume) = resume else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No résumé for this job yet.")
                .child(self.generate_button(cx))
                .into_any_element();
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
            reworded: resume
                .lines
                .iter()
                .filter(|line| !line.reworded.is_empty())
                .flat_map(|line| {
                    let words: Vec<&str> = line
                        .reworded
                        .iter()
                        .filter_map(|range| line.text.get(range.clone()))
                        .collect();
                    pages.layout.word_boxes(line.id, &words)
                })
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
                    .overflow_x_scroll()
                    .child(
                        v_flex()
                            .min_w(self.page_width() + px(PAGE_GAP * 2.))
                            .items_center()
                            .gap(px(PAGE_GAP))
                            .py(px(PAGE_GAP))
                            .children(page_elements),
                    ),
            )
            .vertical_scrollbar(&self.scroll_handle)
            .horizontal_scrollbar(&self.scroll_handle)
            .children(menus.map(|menus| div().absolute().top_3().left_3().child(menus)))
            .child(
                div()
                    .absolute()
                    .bottom_4()
                    .right_4()
                    .child(zoom_bar(self.zoom, cx)),
            )
            .into_any_element()
    }

    fn render_page(
        &self,
        index: usize,
        image: Arc<RenderImage>,
        size: PageSize,
        boxes: &[LineBox],
        markers: &Markers,
        cx: &Context<Self>,
    ) -> AnyElement {
        let scale = self.scale(size);
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
            .w(self.page_width())
            .h(px(size.height * scale))
            .bg(rgb(PAPER))
            .shadow_md()
            .child(img(image).absolute().size_full())
            // Not part of the printed page either: the words a line has that
            // its facts don't, for the user to check.
            .children(
                markers
                    .reworded
                    .iter()
                    .filter(|word| word.page == index)
                    .map(|word| {
                        div()
                            .absolute()
                            .left(px(word.left * scale - 1.))
                            .top(px(word.top * scale))
                            .w(px((word.right - word.left) * scale + 2.))
                            .h(px((word.bottom - word.top) * scale))
                            .bg(Hsla::from(rgb(REWORDED)).opacity(0.3))
                            .border_b_2()
                            .border_color(rgb(REWORDED_UNDERLINE))
                    }),
            )
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
    /// Words lines have that their facts don't, on every page.
    reworded: Vec<WordBox>,
}

fn margin_marker(id: impl Into<ElementId>, tooltip: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
}

/// A pill floating over the pages, as it's about them, so it takes no row
/// of its own: which template lays the résumé out and on what paper, with
/// menus to pick others. Switching keeps the content, so it's only a new
/// layout. A version that was sent keeps its template and paper, so for one
/// they're only named.
fn page_menus(version: &Version, read_only: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (picker, paper) = if read_only {
        (
            div()
                .px_2()
                .text_sm()
                .child(version.template.name)
                .into_any_element(),
            div()
                .px_2()
                .text_sm()
                .child(version.paper.label())
                .into_any_element(),
        )
    } else {
        (
            template_menu(version.template).into_any_element(),
            paper_menu(version.paper).into_any_element(),
        )
    };
    let divider = || div().w(px(1.)).h_4().bg(theme.border);
    pill(h_flex(), cx)
        .when(read_only, |this| {
            this.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Sent version"),
            )
            .child(divider())
        })
        .child(picker)
        .child(divider())
        .child(paper)
        .into_any_element()
}

/// A pill floating over the pages' bottom right corner: zoom in, back to
/// actual size, zoom out, and fit to the width. The zoom itself is in the
/// tooltips: a percentage among the icons made the pill uneven.
fn zoom_bar(zoom: f32, cx: &App) -> impl IntoElement {
    let divider = || div().h(px(1.)).w_4().bg(cx.theme().border);
    let percent = format!("{}%", (zoom * 100.).round());
    pill(v_flex(), cx)
        .items_center()
        .child(
            Button::new("zoom-in")
                .icon(lucide("zoom-in"))
                .ghost()
                .small()
                .disabled(zoom >= ZOOMS[ZOOMS.len() - 1])
                .tooltip_with_action(
                    format!("Zoom in (now {percent})"),
                    &ZoomIn,
                    Some(KEY_CONTEXT),
                )
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ZoomIn), cx)),
        )
        .child(
            Button::new("actual-size")
                .icon(IconName::Search)
                .ghost()
                .small()
                .disabled(zoom == 1.)
                .tooltip_with_action(
                    format!("Actual size (now {percent})"),
                    &ResetZoom,
                    Some(KEY_CONTEXT),
                )
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ResetZoom), cx)),
        )
        .child(
            Button::new("zoom-out")
                .icon(lucide("zoom-out"))
                .ghost()
                .small()
                .disabled(zoom <= ZOOMS[0])
                .tooltip_with_action(
                    format!("Zoom out (now {percent})"),
                    &ZoomOut,
                    Some(KEY_CONTEXT),
                )
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ZoomOut), cx)),
        )
        .child(divider())
        .child(
            Button::new("fit-width")
                .icon(lucide("move-horizontal"))
                .ghost()
                .small()
                .tooltip("Fit width")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(FitWidth), cx)),
        )
}

/// A Lucide icon `IconName` doesn't name; `AllAssets` has every one.
fn lucide(name: &str) -> Icon {
    Icon::empty().path(format!("icons/{name}.svg"))
}

/// A raised surface over the pages, for controls that float there. In
/// light palettes the popover and background colors and the border are all
/// near the paper's white, so over a zoomed page the pill vanished: it takes
/// the sidebar's darker tint and an edge from the secondary text, which is
/// dark in light palettes and light in dark ones.
fn pill(base: Div, cx: &App) -> Div {
    let theme = cx.theme();
    base.gap_1()
        .p_1()
        .rounded_lg()
        .border_1()
        .border_color(theme.muted_foreground.opacity(0.3))
        .bg(theme.sidebar)
        .shadow_lg()
}

fn template_menu(current: &'static typeset::Template) -> impl IntoElement {
    let current = current.key;
    Button::new("template")
        .label(typeset::template(current).name)
        .dropdown_caret(true)
        .ghost()
        .small()
        .tooltip("Résumé template")
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

fn paper_menu(current: Paper) -> impl IntoElement {
    Button::new("paper")
        .label(current.label())
        .dropdown_caret(true)
        .ghost()
        .small()
        .tooltip("Paper size")
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
            for paper in Paper::ALL {
                menu = menu.menu_with_check(
                    paper.label(),
                    paper == current,
                    Box::new(UsePaper { paper }),
                );
            }
            menu
        })
}

/// `version` is the résumé version on screen.
fn job_header(
    job: &Application,
    version: Option<&Version>,
    cx: &Context<JobView>,
) -> impl IntoElement {
    let (heading, title_length) = heading(&job.title, &job.company);
    let mut details = vec![application_view::status_label(job, Utc::now())];
    if let Some(version) = version {
        details.push(format!(
            "Generated {}",
            version
                .generated_at
                .with_timezone(&Local)
                .format("%b %-d, %H:%M")
        ));
        if let Some(cost) = version.cost {
            details.push(format!("cost {}", llm::cost_label(cost)));
        }
    }

    h_flex()
        .flex_1()
        .min_w(px(160.))
        .gap_2()
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
                        .truncate()
                        .child(StyledText::new(heading).with_highlights([(
                            0..title_length,
                            HighlightStyle {
                                font_weight: Some(FontWeight::SEMIBOLD),
                                ..Default::default()
                            },
                        )])),
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

/// "Title - Company", and how many bytes of it the title takes so it can be
/// set bolder.
fn heading(title: &str, company: &str) -> (String, usize) {
    match (title.trim(), company.trim()) {
        ("", company) => (company.to_string(), 0),
        (title, "") => (title.to_string(), title.len()),
        (title, company) => (format!("{title} - {company}"), title.len()),
    }
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
            .child(
                Icon::new(IconName::Info)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(div().flex_1().min_w_0().child(text))
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
    paper: Paper,
    /// What writing it cost, when every call was priced.
    cost: Option<f64>,
}

impl Versions {
    fn read(connection: &Connection, job: JobId) -> Result<Self, Error> {
        let version = |row: &rusqlite::Row| -> rusqlite::Result<Version> {
            Ok(Version {
                id: ResumeId(row.get(0)?),
                generated_at: DateTime::from_timestamp_millis(row.get(1)?).unwrap_or_default(),
                template: typeset::template_of_version(row.get(2)?),
                paper: Paper::from_stored(row.get::<_, Option<String>>(3)?.as_deref()),
                cost: row.get(4)?,
            })
        };
        // A version's cost is left out when a call wasn't priced: the total
        // would only be a lower bound.
        let current = connection
            .prepare_cached(
                "SELECT id, generated_at, template_version_id, paper,
                        (SELECT CASE WHEN sum(unpriced_calls) > 0 THEN NULL ELSE sum(cost) END
                         FROM llm_runs WHERE generation_id = generations.id)
                 FROM generations
                 WHERE job_id = ?1 AND kind = 'resume' AND deleted_at IS NULL
                 ORDER BY generated_at DESC, id DESC LIMIT 1",
            )?
            .query_row([job.0], version)
            .optional()?;
        let sent = connection
            .prepare_cached(
                "SELECT id, generated_at, template_version_id, paper,
                        (SELECT CASE WHEN sum(unpriced_calls) > 0 THEN NULL ELSE sum(cost) END
                         FROM llm_runs WHERE generation_id = generations.id)
                 FROM generations
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
    /// Where its text has words its facts don't: a rewording, for the user
    /// to check.
    pub reworded: Vec<std::ops::Range<usize>>,
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
                    reworded: Vec::new(),
                })
            })?
            .collect::<Result<_, _>>()?;
        let citations = model::citations(connection, resume)?;
        for (line, fact) in citations {
            if let Some(line) = lines.iter_mut().find(|each| each.id == line) {
                line.cites.push(fact);
            }
        }
        let mut text_of = connection
            .prepare_cached("SELECT text FROM facts WHERE id = ?1 AND deleted_at IS NULL")?;
        for line in &mut lines {
            let mut facts = Vec::new();
            for fact in &line.cites {
                if let Some(text) = text_of
                    .query_row([fact.0], |row| row.get::<_, String>(0))
                    .optional()?
                {
                    facts.push(text);
                }
            }
            if !facts.is_empty() {
                let facts: Vec<&str> = facts.iter().map(String::as_str).collect();
                line.reworded = verifier::new_words(&line.text, &facts);
            }
        }
        Ok(Self {
            id: resume,
            lines,
            changes: model::fact_changes(connection, resume)?,
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

/// Whether the user knows a résumé's lines open their facts. Unreadable,
/// they're taken to know, rather than shown the note for good.
fn lines_explained(database: &Entity<Database>, cx: &App) -> bool {
    settings::resume_lines_explained(database.read(cx).connection()).unwrap_or_else(|error| {
        eprintln!("job view: couldn't read whether lines were explained: {error}");
        true
    })
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

/// The name a résumé's PDF is offered under: the person's name and the
/// company, without characters file names can't hold.
fn pdf_name<'a>(name: Option<&'a str>, company: Option<&'a str>) -> String {
    let filled = |part: Option<&'a str>| part.map(str::trim).filter(|part| !part.is_empty());
    let parts: Vec<&str> = [filled(name).or(Some("Résumé")), filled(company)]
        .into_iter()
        .flatten()
        .collect();
    let name: String = parts
        .join(" - ")
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect();
    format!("{name}.pdf")
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
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;

    #[test]
    fn a_pdf_is_offered_under_the_name_and_company() {
        assert_eq!(
            pdf_name(Some("Alex Rahman"), Some("Acme/Co: Labs")),
            "Alex Rahman - Acme-Co- Labs.pdf"
        );
        assert_eq!(pdf_name(Some(" "), None), "Résumé.pdf");
    }

    #[test]
    fn the_heading_leads_with_the_title_then_the_company() {
        assert_eq!(
            heading("Senior Rust Engineer", "Cinder Labs"),
            ("Senior Rust Engineer - Cinder Labs".to_string(), 20)
        );
        assert_eq!(heading(" ", "Cinder Labs"), ("Cinder Labs".to_string(), 0));
        assert_eq!(heading("Engineer", ""), ("Engineer".to_string(), 8));
    }
    use crate::database::{TemporaryDirectory, Write};
    use crate::model::{ClaimId, FactId, UnsupportedReason, sample};
    use crate::workspace::testing;

    /// A job's company, its current résumé's template, and whether an older
    /// one was sent.
    type JobVersions = (String, Option<&'static str>, bool);
    /// A claim's reason, the fact added from it, and whether the hidden fact
    /// it needs is shown again.
    type ClaimState = (UnsupportedReason, Option<FactId>, bool);

    /// Each job's versions, in the board's order.
    fn versions(connection: &Connection) -> Result<Vec<JobVersions>, Error> {
        let jobs: Vec<(JobId, String)> = connection
            .prepare("SELECT id, company FROM jobs ORDER BY board_sort_key, id")?
            .query_map([], |row| Ok((JobId(row.get(0)?), row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        jobs.into_iter()
            .map(|(job, company)| {
                let versions = Versions::read(connection, job)?;
                Ok((
                    company,
                    versions.current.map(|version| version.template.key),
                    versions.sent.is_some(),
                ))
            })
            .collect()
    }

    /// What the job's current résumé shows changed, and its unsupported
    /// claims.
    fn current(
        connection: &Connection,
        company: &str,
    ) -> Result<(Vec<FactChange>, Vec<ClaimState>), Error> {
        let Some(version) = Versions::read(connection, sample::job(connection, company)?)?.current
        else {
            return Ok(Default::default());
        };
        let shown = ShownResume::read(connection, version.id)?;
        let mut changes: Vec<FactChange> =
            shown.changes.iter().map(|(_, change)| *change).collect();
        changes.sort_by_key(|change| format!("{change:?}"));
        let claims = Inspection::read(connection, &shown, None)?
            .claims
            .into_iter()
            .map(|claim| (claim.reason, claim.added_as, claim.hidden_fact_shown))
            .collect();
        Ok((changes, claims))
    }

    #[test]
    fn a_line_shows_the_words_its_facts_dont_have() -> Result<(), Error> {
        let writer = sample::written()?;
        let connection = writer.kron();
        let Some(version) =
            Versions::read(connection, sample::job(connection, "Cinder Labs")?)?.current
        else {
            panic!("Cinder Labs has a résumé");
        };
        let shown = ShownResume::read(connection, version.id)?;
        let reworded = |start: &str| -> Vec<&str> {
            shown
                .lines
                .iter()
                .find(|line| line.text.starts_with(start))
                .map(|line| {
                    line.reworded
                        .iter()
                        .filter_map(|range| line.text.get(range.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };
        assert_eq!(
            reworded("Reduced the routing"),
            ["Reduced"],
            "its fact says Cut"
        );
        assert_eq!(
            reworded("Led the migration"),
            Vec::<&str>::new(),
            "as its fact says"
        );
        Ok(())
    }

    #[test]
    fn each_job_shows_its_current_resume_and_the_older_one_sent() -> Result<(), Error> {
        let writer = sample::written()?;
        let expected = [
            ("Cinder Labs", Some("classic"), false),
            ("Ledgerly", Some("modern"), true),
            ("Atlas Freight", Some("modern"), false),
            ("Tidewater Systems", Some("sidebar"), false),
            ("Quillpay", Some("modern"), false),
            ("Northstar Metrics", Some("classic"), false),
            ("Brightwave", Some("sidebar"), false),
            ("Kestrel Labs", Some("modern"), false),
            ("Halcyon Cloud", None, false),
        ];
        assert_eq!(
            versions(writer.kron())?,
            expected.map(|(company, template, sent)| (company.to_string(), template, sent))
        );
        Ok(())
    }

    #[test]
    fn a_shown_resume_reads_back_its_lines_and_what_they_cite() -> Result<(), Error> {
        let writer = sample::written()?;
        let connection = writer.kron();
        let resumes: Vec<ResumeId> = connection
            .prepare("SELECT id FROM generations WHERE kind = 'resume'")?
            .query_map([], |row| Ok(ResumeId(row.get(0)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(resumes.len(), 9);
        for resume in resumes {
            let shown = ShownResume::read(connection, resume)?;
            let mut lines: Vec<(LineId, String, String)> = shown
                .lines
                .iter()
                .map(|line| (line.id, line.text.clone(), line.generated.clone()))
                .collect();
            let mut stored: Vec<(LineId, String, String)> = connection
                .prepare(
                    "SELECT lines.id, lines.text, lines.generated FROM lines
                     JOIN blocks ON blocks.id = lines.block_id
                     JOIN sections ON sections.id = blocks.section_id
                     WHERE sections.generation_id = ?1",
                )?
                .query_map([resume.0], |row| {
                    Ok((LineId(row.get(0)?), row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<_, _>>()?;
            lines.sort();
            stored.sort();
            assert_eq!(lines, stored);

            for line in &shown.lines {
                let cited: Vec<FactId> = connection
                    .prepare(
                        "SELECT fact_id FROM line_citations WHERE line_id = ?1 ORDER BY sort_key",
                    )?
                    .query_map([line.id.0], |row| Ok(FactId(row.get(0)?)))?
                    .collect::<Result<_, _>>()?;
                assert_eq!(line.cites, cited, "{:?}", line.text);
                let inspection = Inspection::read(connection, &shown, Some(line.id))?;
                assert_eq!(
                    inspection
                        .facts
                        .iter()
                        .map(|cited| cited.id)
                        .collect::<Vec<_>>(),
                    cited
                );
                for cited in &inspection.facts {
                    let Some(fact) = &cited.fact else {
                        panic!("{:?} cites a missing fact", line.text);
                    };
                    let text: String = connection.query_row(
                        "SELECT text FROM facts WHERE id = ?1",
                        [cited.id.0],
                        |row| row.get(0),
                    )?;
                    assert_eq!(fact.text, text);
                    assert!(!fact.sources.is_empty(), "{:?} has no source", fact.text);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn a_version_costs_what_its_runs_did_unless_one_was_unpriced() -> Result<(), Error> {
        let mut writer = sample::written()?;
        let cost = |writer: &crate::database::Writer| -> Result<Option<f64>, Error> {
            let connection = writer.kron();
            let job = sample::job(connection, "Ledgerly")?;
            Ok(Versions::read(connection, job)?
                .current
                .and_then(|version| version.cost))
        };
        assert_eq!(cost(&writer)?, None, "the sample's weren't run");
        let run = |cost: f64, unpriced: i64| {
            format!(
                "INSERT INTO llm_runs (id, task, started_at, duration, outcome, provider, vendor,
                                       model, calls, input_tokens, cached_input_tokens,
                                       cache_written_tokens, output_tokens, reasoning_tokens,
                                       cost, unpriced_calls, job_id, generation_id)
                 SELECT randomblob(16), 'generate_resume', 0, 0, 'done', 'OpenAI', 'openai',
                        'gpt-5.4-mini', 1, 0, 0, 0, 0, 0, {cost}, {unpriced}, jobs.id,
                        generations.id
                 FROM generations JOIN jobs ON jobs.id = generations.job_id
                 WHERE jobs.company = 'Ledgerly' AND generations.kind = 'resume'
                 ORDER BY generated_at DESC LIMIT 1"
            )
        };
        writer.execute(|write| write.seed(&run(0.03, 0)))?;
        writer.execute(|write| write.seed(&run(0.01, 0)))?;
        assert_eq!(cost(&writer)?.map(|cost| (cost * 100.).round()), Some(4.));
        writer.execute(|write| write.seed(&run(0., 1)))?;
        assert_eq!(cost(&writer)?, None, "a lower bound isn't shown");
        Ok(())
    }

    #[test]
    fn a_current_resume_shows_its_changed_facts_and_unsupported_claims() -> Result<(), Error> {
        let writer = sample::written()?;
        let connection = writer.kron();
        let property_tests = sample::fact(connection, "Introduced property-based tests")?;
        assert_eq!(
            current(connection, "Cinder Labs")?,
            (
                vec![FactChange::Reworded],
                vec![
                    (UnsupportedReason::InventedFigure("30%".into()), None, false),
                    (UnsupportedReason::InventedYear(2016), None, false),
                    (UnsupportedReason::NoSupportingFact, None, false),
                ]
            )
        );
        assert_eq!(
            current(connection, "Ledgerly")?,
            (
                vec![],
                vec![
                    (
                        UnsupportedReason::UsesHiddenFact(property_tests),
                        None,
                        false
                    ),
                    (UnsupportedReason::InventedFigure("$2M".into()), None, false),
                ]
            )
        );
        assert_eq!(
            current(connection, "Atlas Freight")?,
            (
                vec![FactChange::Hidden, FactChange::Reworded],
                vec![(UnsupportedReason::InventedFigure("12%".into()), None, false)]
            )
        );
        assert_eq!(
            current(connection, "Brightwave")?,
            (vec![FactChange::Reworded], vec![])
        );
        for company in [
            "Tidewater Systems",
            "Quillpay",
            "Northstar Metrics",
            "Kestrel Labs",
            "Halcyon Cloud",
        ] {
            assert_eq!(current(connection, company)?, (vec![], vec![]), "{company}");
        }
        Ok(())
    }

    #[test]
    fn the_queries_follow_changes_to_facts_and_claims() -> Result<(), Error> {
        let mut writer = sample::written()?;
        let latency = sample::fact(writer.kron(), "Cut p99 latency")?;
        let property_tests = sample::fact(writer.kron(), "Introduced property-based tests")?;
        let claim: ClaimId = ClaimId(writer.kron().query_row(
            "SELECT unsupported_claims.id FROM unsupported_claims
             JOIN generations ON generations.id = unsupported_claims.generation_id
             JOIN jobs ON jobs.id = generations.job_id
             WHERE jobs.company = 'Cinder Labs' AND unsupported_claims.figure = '30%'",
            [],
            |row| row.get(0),
        )?);
        writer.execute(|write| commands::edit_fact(write, latency, "Cut p99 latency by 80%."))?;
        writer.execute(|write| commands::set_hidden(write, property_tests, false))?;
        let (added, _) = writer.execute(|write| commands::add_claim_as_fact(write, claim))?;
        writer.execute(|write| commands::set_hidden(write, latency, true))?;
        let connection = writer.kron();

        // The claim is marked added, and the latency fact shows as hidden.
        assert_eq!(
            current(connection, "Cinder Labs")?,
            (
                vec![FactChange::Hidden, FactChange::Reworded],
                vec![
                    (
                        UnsupportedReason::InventedFigure("30%".into()),
                        added.first().copied(),
                        false
                    ),
                    (UnsupportedReason::InventedYear(2016), None, false),
                    (UnsupportedReason::NoSupportingFact, None, false),
                ]
            )
        );
        // The hidden fact Ledgerly's claim needed is shown again.
        assert_eq!(
            current(connection, "Ledgerly")?,
            (
                vec![FactChange::Hidden, FactChange::Shown],
                vec![
                    (
                        UnsupportedReason::UsesHiddenFact(property_tests),
                        None,
                        true
                    ),
                    (UnsupportedReason::InventedFigure("$2M".into()), None, false),
                ]
            )
        );
        assert_eq!(
            current(connection, "Tidewater Systems")?,
            (vec![FactChange::Hidden], vec![])
        );
        assert_eq!(current(connection, "Quillpay")?, (vec![], vec![]));
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
        let job = database.read_with(app, |database, _| {
            database.connection().query_row(
                "SELECT id FROM jobs WHERE company = ?1",
                [company],
                |row| row.get(0),
            )
        });
        let job = match job {
            Ok(job) => JobId(job),
            Err(error) => panic!("no job at {company}: {error}"),
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
            cx.subscribe(&view, move |_, _: &JobViewEvent, _| {
                *closed.borrow_mut() += 1
            })
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
        assert!(
            !inspection.facts.is_empty(),
            "the inspector shows its facts"
        );
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
    fn how_to_read_a_resume_is_said_until_a_line_is_selected_even_after_a_restart() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        assert!(view.read(|view| view.explain_lines), "a new user is told");

        let other = view.database.read_with(view.cx, |database, _| {
            database.connection().query_row(
                "SELECT id FROM jobs WHERE company = 'Cinder Labs'",
                [],
                |row| row.get(0),
            )
        });
        let other = match other {
            Ok(job) => JobId(job),
            Err(error) => panic!("no Cinder Labs job: {error}"),
        };
        view.update(|view, cx| view.set_job(other, cx));
        assert!(
            view.read(|view| view.explain_lines),
            "on another résumé too"
        );

        // As the next start would open it.
        let reopened = |view: &mut View| {
            let database = view.database.clone();
            view.cx.update(|window, cx| {
                let previews = cx.new(|cx| Previews::new(database.clone(), cx));
                cx.new(|cx| JobView::new(database, previews, window, cx))
            })
        };
        let restarted = reopened(&mut view);
        assert!(restarted.read_with(view.cx, |view, _| view.explain_lines));

        view.keys("down");
        assert!(!view.read(|view| view.explain_lines));
        let saved = view.database.read_with(view.cx, |database, _| {
            settings::resume_lines_explained(database.connection())
        });
        assert!(matches!(saved, Ok(true)));
        assert!(
            !restarted.read_with(view.cx, |view, _| view.explain_lines),
            "an open view hears it"
        );
        let restarted = reopened(&mut view);
        assert!(!restarted.read_with(view.cx, |view, _| view.explain_lines));
    }

    #[test]
    fn closing_the_note_saves_that_lines_were_explained() {
        use gpui_kit::test::TestWindowExt;

        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        view.cx
            .update(|window, cx| window.click("close-lines-notice", cx));
        view.settle();
        assert!(!view.read(|view| view.explain_lines));
        let saved = view.database.read_with(view.cx, |database, _| {
            settings::resume_lines_explained(database.connection())
        });
        assert!(matches!(saved, Ok(true)));
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
        assert!(
            shown
                .lines
                .iter()
                .any(|each| each.id == line && shown.is_outdated(each))
        );
        let inspection = view.inspection();
        let hidden = inspection
            .facts
            .iter()
            .find(|cited| cited.id == latency)
            .and_then(|cited| cited.fact.as_ref())
            .map(|fact| fact.hidden);
        assert_eq!(hidden, Some(true));

        view.run(move |write| commands::set_hidden(write, latency, false));
        assert!(
            view.shown().changes.is_empty(),
            "showing it again undoes it"
        );
    }

    #[test]
    fn the_changed_facts_notice_wraps_inside_the_page_and_offers_regenerating() {
        use gpui_kit::test::TestWindowExt;

        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Cinder Labs");
        assert!(
            !view.shown().changes.is_empty(),
            "Cinder Labs has a changed fact"
        );
        // Narrow enough that the text can't fit on one line beside the button.
        view.cx.simulate_resize(gpui_kit::size(px(720.), px(720.)));
        view.cx.run_until_parked();
        view.cx.update(|window, cx| {
            window.render_frame(cx);
            let notice = window.find("outdated-notice").bounds();
            let text = window.find("outdated-text").bounds();
            assert!(
                text.right() <= notice.right(),
                "the text runs past its notice: {text:?} in {notice:?}"
            );
            assert!(window.find("regenerate-outdated").visible());
        });
    }

    /// The hint once wrapped between its arrows, ending one line on ↑.
    #[test]
    fn the_source_hint_keeps_its_arrow_keys_together() {
        use gpui_kit::test::TestWindowExt;

        let mut app = TestAppContext::single();
        let view = open(&mut app, "Cinder Labs");
        for width in [720., 1100., 1500.] {
            view.cx.simulate_resize(gpui_kit::size(px(width), px(720.)));
            view.cx.run_until_parked();
            view.cx.update(|window, cx| window.render_frame(cx));
            let (Some(up), Some(down)) =
                (view.cx.debug_bounds("kbd:↑"), view.cx.debug_bounds("kbd:↓"))
            else {
                panic!("at {width}px the hint doesn't show both arrow keys");
            };
            assert_eq!(up.top(), down.top(), "at {width}px the arrows split");
        }
    }

    /// Ledgerly's line citing the latency fact, selected.
    fn select_latency_line(view: &mut View) -> LineId {
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
        view.update(|view, cx| view.select_line(line, cx));
        line
    }

    /// Enter on the selected line, then `text` in place of its own.
    fn type_line(view: &mut View, text: &str) {
        view.keys("enter");
        let editor = view.read(|view| view.line_editor.clone());
        view.cx.update(|window, cx| {
            let focused = editor.read(cx).focus_handle(cx).is_focused(window);
            assert!(focused, "Enter puts the cursor in the line");
            editor.update(cx, |editor, cx| editor.set_value("", window, cx));
        });
        view.cx.simulate_input(text);
        view.settle();
    }

    fn stored_text(view: &mut View, line: LineId) -> String {
        view.connection(|connection| {
            connection.query_row("SELECT text FROM lines WHERE id = ?1", [line.0], |row| {
                row.get(0)
            })
        })
    }

    #[test]
    fn an_edited_line_saves_on_enter_and_says_what_its_facts_dont() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        let line = select_latency_line(&mut view);

        type_line(&mut view, "Cut p99 latency from 480 ms to 15 ms.");
        view.keys("enter");
        assert_eq!(
            stored_text(&mut view, line),
            "Cut p99 latency from 480 ms to 15 ms."
        );
        assert!(
            view.shown()
                .lines
                .iter()
                .any(|each| each.id == line && each.is_edited())
        );
        assert_eq!(
            view.inspection().edit_problem,
            Some(UnsupportedReason::InventedFigure("15".to_owned()))
        );

        type_line(&mut view, "Cut p99 latency to 95 ms.");
        view.keys("enter");
        assert_eq!(stored_text(&mut view, line), "Cut p99 latency to 95 ms.");
        assert_eq!(view.inspection().edit_problem, None);

        view.run(move |write| commands::revert_line(write, line));
        assert!(view.shown().lines.iter().all(|each| !each.is_edited()));
        let generated = stored_text(&mut view, line);
        let editor = view.read(|view| view.line_editor.clone());
        assert_eq!(
            editor.read_with(view.cx, |editor, _| editor.value().to_string()),
            generated,
            "the editor shows the line as reverted"
        );
    }

    #[test]
    fn escape_puts_an_edited_line_back_and_moving_away_saves_it() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        let line = select_latency_line(&mut view);
        let generated = stored_text(&mut view, line);

        type_line(&mut view, "Something else.");
        view.keys("escape");
        assert_eq!(stored_text(&mut view, line), generated);
        assert_eq!(
            view.selected(),
            Some(line),
            "Esc leaves the editor, not the line"
        );

        type_line(&mut view, "Cut p99 latency to 95 ms.");
        let other = view
            .shown()
            .lines
            .iter()
            .find(|each| each.id != line)
            .map(|each| each.id);
        let Some(other) = other else {
            panic!("Ledgerly's résumé has one line");
        };
        view.update(|view, cx| view.select_line(other, cx));
        assert_eq!(stored_text(&mut view, line), "Cut p99 latency to 95 ms.");
    }

    #[test]
    fn zoom_steps_up_and_down_and_back_to_actual_size() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        view.keys("ctrl-=");
        assert_eq!(view.read(|view| view.zoom), 1.1);
        view.keys("ctrl-- ctrl--");
        assert_eq!(view.read(|view| view.zoom), 0.9);
        for _ in 0..ZOOMS.len() {
            view.keys("ctrl--");
        }
        assert_eq!(
            view.read(|view| view.zoom),
            ZOOMS[0],
            "it stops at the smallest step"
        );
        view.keys("ctrl-0");
        assert_eq!(view.read(|view| view.zoom), 1.);
    }

    #[test]
    fn a_fitted_zoom_steps_to_the_next_step() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        view.update(|view, cx| view.set_zoom(1.95, cx));
        view.keys("ctrl--");
        assert_eq!(view.read(|view| view.zoom), 1.5);
        view.update(|view, cx| view.set_zoom(1.95, cx));
        view.keys("ctrl-=");
        assert_eq!(view.read(|view| view.zoom), 2.);
    }

    #[test]
    fn a_template_switch_shows_in_the_page_menus() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        view.cx.dispatch_action(UseTemplate { key: "sidebar" });
        view.settle();
        let template = view.read(|view| view.shown_version().map(|version| version.template.key));
        assert_eq!(template, Some("sidebar"));
    }

    #[test]
    fn a_paper_switch_shows_in_the_page_menus() {
        let mut app = TestAppContext::single();
        let mut view = open(&mut app, "Ledgerly");
        for paper in Paper::ALL {
            view.cx.dispatch_action(UsePaper { paper });
            view.settle();
            let shown = view.read(|view| view.shown_version().map(|version| version.paper));
            assert_eq!(shown, Some(paper));
        }
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
        assert_eq!(
            view.selected(),
            None,
            "a new version starts with no selection"
        );
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

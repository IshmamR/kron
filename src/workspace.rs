mod ai_settings;
mod application_view;
mod fact_list;
mod facts_page;
mod import_page;
mod inspector;
mod instructions_form;
mod job_form;
mod job_view;
mod onboarding;
mod palette_picker;
mod parts;
mod previews;
mod redaction_check;
mod resume_grid;
mod settings_page;
mod shortcuts;
mod stand_in_name;
#[cfg(test)]
mod testing;

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::sidebar::{
    Sidebar, SidebarCollapsible, SidebarGroup, SidebarItem, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme, Collapsible, Icon, IconName, Root, Sizable, WindowExt, h_flex, v_flex,
};
use gpui_kit::{
    Action, App, Bounds, Context, Entity, Focusable, IntoElement, KeyBinding, Pixels, SharedString,
    Subscription, Task, Window, WindowBounds, actions, div, prelude::*, px,
};

use crate::appearance::{Appearance, Motion, Palettes};
use crate::database::schema::{self, Documents, FactSources, Facts, LlmRuns};
use crate::database::{Changed, Database};
use crate::llm::{self, Purpose};
use crate::model::extraction;
use crate::model::settings::{self, DeviceSettings, LastPage};
use crate::model::{Application, DocumentId, FactId, JobId, LineId};
use crate::window;
use facts_page::FactsPage;
use import_page::ImportPage;
use job_form::{JobForm, JobFormEvent};
use job_view::{JobView, JobViewEvent};
use onboarding::{Onboarding, OnboardingEvent};
use previews::Previews;
use resume_grid::{ResumeGrid, ResumeGridEvent};
use settings_page::{SettingsPage, SettingsSection};

const SIDEBAR_WIDTH: Pixels = px(204.);
/// How long the window has to stay put before where it is gets saved, so a
/// drag saves once, not on every step.
const WINDOW_SAVE_DELAY: Duration = Duration::from_millis(500);
/// Wide enough for a description's lines to read as they were written.
const JOB_FORM_WIDTH: f32 = 640.;

actions!(
    workspace,
    [
        ToggleSidebar,
        ShowShortcuts,
        Undo,
        Redo,
        /// Opens Settings on the AI provider.
        SetUpAi,
        ShowSettings,
        LeaveSettings,
        ShowFacts
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-b", ToggleSidebar, None),
        KeyBinding::new("secondary-/", ShowShortcuts, None),
        // A text field's own undo comes first: its key context is deeper.
        KeyBinding::new("secondary-z", Undo, None),
        KeyBinding::new("secondary-y", Redo, None),
        KeyBinding::new("secondary-,", ShowSettings, None),
    ]);
    settings_page::init(cx);
    facts_page::init(cx);
    import_page::init(cx);
    redaction_check::init(cx);
    job_view::init(cx);
    resume_grid::init(cx);
}

/// Opens a job with one of its résumé's lines selected.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct OpenLine {
    pub job: JobId,
    pub line: LineId,
}

/// Opens the facts page on a fact.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct ShowFact {
    pub fact: FactId,
}

/// The sidebar and whichever page it has open.
pub struct Workspace {
    database: Entity<Database>,
    /// Facts that need review, for the sidebar's badge: counted on start,
    /// and again after facts change.
    needs_review: usize,
    /// Documents whose facts haven't been found, as saved with no AI set up,
    /// with their file names: read on start, and again after they change.
    unread: Vec<(DocumentId, Option<String>)>,
    page: Page,
    /// Down to its icons, which give their names as tooltips.
    sidebar_collapsed: bool,
    /// The page last saved to open on start, so it's saved only when it changes.
    saved_page: LastPage,
    /// Waits out `WINDOW_SAVE_DELAY`, then saves the window's bounds.
    saving_window: Task<()>,
    /// The window's bounds when it was last neither maximized nor full
    /// screen: what it restores to.
    windowed_bounds: Bounds<Pixels>,
    resume_grid: Entity<ResumeGrid>,
    job_view: Entity<JobView>,
    facts_page: Entity<FactsPage>,
    import_page: Entity<ImportPage>,
    /// Over everything else, onboarding too, until it's closed.
    settings_page: Option<Entity<SettingsPage>>,
    /// Shown in place of everything else while the user has nothing yet.
    onboarding: Option<Entity<Onboarding>>,
    /// The "New job" form's, while it's open.
    _job_form: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.page {
            Page::Resumes => self.resume_grid.clone().into_any_element(),
            Page::Job => self.job_view.clone().into_any_element(),
            Page::Facts => self.facts_page.clone().into_any_element(),
            Page::Import => self.import_page.clone().into_any_element(),
        };

        div()
            .on_action(cx.listener(|this, action: &OpenLine, window, cx| {
                this.open_job(action.job, window, cx);
                this.job_view
                    .update(cx, |job_view, cx| job_view.select_line(action.line, cx));
            }))
            .on_action(cx.listener(|this, action: &ShowFact, window, cx| {
                this.settings_page = None;
                this.page = Page::Facts;
                this.facts_page.update(cx, |facts_page, cx| {
                    facts_page.show_fact(action.fact, window, cx)
                });
                this.save_page(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SetUpAi, window, cx| {
                this.show_settings(Some(SettingsSection::Ai), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowSettings, window, cx| {
                this.show_settings(None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &LeaveSettings, window, cx| {
                this.leave_settings(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowFacts, window, cx| {
                this.open_page(Page::Facts, window, cx)
            }))
            .on_action(|_: &ShowShortcuts, window, cx| shortcuts::open(window, cx))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(
                cx.listener(|this, _: &Undo, window, cx| this.undo_or_redo(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &Redo, window, cx| this.undo_or_redo(true, window, cx)),
            )
            .size_full()
            .child(match (&self.settings_page, &self.onboarding) {
                (Some(settings), _) => settings.clone().into_any_element(),
                (_, Some(onboarding)) => onboarding.clone().into_any_element(),
                (None, None) => h_flex()
                    .size_full()
                    .child(self.render_sidebar(window, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .children(self.render_unread(cx))
                            .child(div().flex_1().min_h_0().child(page)),
                    )
                    .into_any_element(),
            })
    }
}

impl Workspace {
    pub fn new(database: Entity<Database>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (stored, device) = {
            let database = database.read(cx);
            let stored = settings::Settings::read(database.connection());
            let device = DeviceSettings::read(database.device_connection());
            (stored, device)
        };
        match stored {
            Ok(stored) => apply_settings(stored, cx),
            Err(error) => eprintln!("workspace: couldn't read the settings: {error}"),
        }
        Appearance::current(cx).apply(window, cx);
        let device = device.unwrap_or_else(|error| {
            eprintln!("workspace: couldn't read this device's settings: {error}");
            DeviceSettings::default()
        });

        let previews = cx.new(|cx| Previews::new(database.clone(), cx));
        let resume_grid =
            cx.new(|cx| ResumeGrid::new(database.clone(), previews.clone(), window, cx));
        let job_view = cx.new(|cx| JobView::new(database.clone(), previews, window, cx));
        let facts_page = cx.new(|cx| FactsPage::new(database.clone(), window, cx));
        let import_page = cx.new(|cx| ImportPage::new(database.clone(), window, cx));
        resume_grid.focus_handle(cx).focus(window, cx);

        let mut subscriptions = vec![
            cx.subscribe(&database, |this, database, changed: &Changed, cx| {
                if changed.touches(Facts) {
                    this.count_needs_review(cx);
                }
                if changed.touches(Documents)
                    || changed.touches(FactSources)
                    || changed.touches(LlmRuns)
                {
                    this.read_unread(cx);
                }
                // Read back, not set where they're chosen, so a change synced
                // from another device applies the same way.
                if changed.touches(schema::Settings) {
                    match settings::Settings::read(database.read(cx).connection()) {
                        Ok(stored) => apply_settings(stored, cx),
                        Err(error) => eprintln!("workspace: couldn't read the settings: {error}"),
                    }
                }
            }),
            cx.observe_window_bounds(window, |this, window, cx| this.save_window_soon(window, cx)),
            cx.subscribe_in(
                &resume_grid,
                window,
                |this, _, event, window, cx| match event {
                    ResumeGridEvent::Open(job) => this.open_job(*job, window, cx),
                    ResumeGridEvent::NewJob => this.open_job_form(window, cx),
                    ResumeGridEvent::FiltersChanged => cx.notify(),
                },
            ),
            cx.subscribe_in(
                &job_view,
                window,
                |this, _, event, window, cx| match event {
                    JobViewEvent::Close => this.open_resumes(window, cx),
                },
            ),
            cx.observe_global_in::<Appearance>(window, |_, window, cx| {
                Appearance::current(cx).apply(window, cx)
            }),
            cx.observe_global_in::<Palettes>(window, |_, window, cx| {
                Appearance::current(cx).apply(window, cx)
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                let appearance = Appearance::current(cx);
                if appearance == Appearance::System {
                    appearance.apply(window, cx);
                }
            }),
        ];

        let onboarding = match onboarding::is_new(database.read(cx).connection()) {
            Ok(true) => {
                let onboarding =
                    cx.new(|cx| Onboarding::new(database.clone(), import_page.clone(), window, cx));
                subscriptions.push(cx.subscribe_in(
                    &onboarding,
                    window,
                    |this, _, event, window, cx| match event {
                        OnboardingEvent::Done => {
                            this.onboarding = None;
                            this.open_resumes(window, cx);
                        }
                    },
                ));
                Some(onboarding)
            }
            Ok(false) => None,
            Err(error) => {
                eprintln!("workspace: couldn't check for a new user: {error}");
                None
            }
        };

        let mut workspace = Self {
            needs_review: 0,
            unread: Vec::new(),
            database,
            page: Page::Resumes,
            sidebar_collapsed: device.sidebar_collapsed,
            saved_page: device.last_page,
            saving_window: Task::ready(()),
            // Always opened windowed, then maximized once mapped.
            windowed_bounds: window.bounds(),
            resume_grid,
            job_view,
            facts_page,
            import_page,
            settings_page: None,
            onboarding,
            _job_form: None,
            _subscriptions: subscriptions,
        };
        workspace.count_needs_review(cx);
        workspace.read_unread(cx);
        workspace.restore_page(device.last_page, window, cx);
        if workspace.onboarding.is_some() {
            workspace.import_page.focus_handle(cx).focus(window, cx);
        }
        workspace
    }

    /// Opens the page that was open when the app last closed, or the grid if
    /// its job has gone since.
    fn restore_page(&mut self, page: LastPage, window: &mut Window, cx: &mut Context<Self>) {
        match page {
            LastPage::Resumes => {}
            LastPage::Job(job) => {
                match Application::read(self.database.read(cx).connection(), job) {
                    Ok(Some(_)) => self.open_job(job, window, cx),
                    Ok(None) => self.save_page(cx),
                    Err(error) => eprintln!("workspace: couldn't read job {}: {error}", job.0),
                }
            }
            LastPage::Facts => self.open_page(Page::Facts, window, cx),
        }
    }

    /// Saves the open page to open on start, if it changed.
    fn save_page(&mut self, cx: &mut Context<Self>) {
        let page = match self.page {
            Page::Resumes => LastPage::Resumes,
            Page::Job => self
                .job_view
                .read(cx)
                .job()
                .map_or(LastPage::Resumes, LastPage::Job),
            Page::Facts => LastPage::Facts,
            // Not opened on start: an import half done isn't kept.
            Page::Import => return,
        };
        if page != self.saved_page {
            self.saved_page = page;
            self.database
                .read(cx)
                .queue(move |write| settings::set_last_page(write, page));
        }
    }

    /// The state is read once the delay is over, not with the bounds: on
    /// X11 the bounds change before gpui hears the window is no longer
    /// maximized.
    fn save_window_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.saving_window = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(WINDOW_SAVE_DELAY).await;
            let saving = this.update_in(cx, |this, window, cx| this.save_window(window, cx));
            if let Err(error) = saving {
                eprintln!("workspace: couldn't save the window's bounds: {error}");
            }
        });
    }

    fn save_window(&mut self, window: &Window, cx: &mut Context<Self>) {
        let bounds = window.window_bounds();
        if let WindowBounds::Windowed(windowed) = bounds {
            self.windowed_bounds = windowed;
        }
        let saved = window::saved(bounds, self.windowed_bounds);
        self.database
            .read(cx)
            .queue(move |write| settings::save_window(write, saved));
    }

    fn read_unread(&mut self, cx: &mut Context<Self>) {
        match extraction::unread(self.database.read(cx).connection()) {
            Ok(unread) => {
                self.unread = unread;
                cx.notify();
            }
            Err(error) => {
                eprintln!("workspace: couldn't read the documents to find facts in: {error}")
            }
        }
    }

    /// Above the résumés and facts while a document's facts haven't been
    /// found, as when it was imported with no AI set up: without them,
    /// there's nothing to write a résumé from. Gone while they're found.
    fn render_unread(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        if !matches!(self.page, Page::Resumes | Page::Job | Page::Facts) {
            return None;
        }
        let import_page = self.import_page.read(cx);
        let unread: Vec<&(DocumentId, Option<String>)> = self
            .unread
            .iter()
            .filter(|(document, _)| !import_page.is_finding(*document))
            .collect();
        let &(first, ref file_name) = *unread.first()?;
        let text = match unread.len() {
            1 => format!(
                "The facts in {} haven't been found yet, and résumés are written from them.",
                file_name.as_deref().unwrap_or("the text you added")
            ),
            count => format!(
                "The facts in {count} of your documents haven't been found yet, and résumés \
                 are written from them."
            ),
        };
        let has_provider = llm::assigned(
            self.database.read(cx).device_connection(),
            Purpose::ExtractFacts,
        )
        .unwrap_or_else(|error| {
            eprintln!("workspace: couldn't read the AI provider: {error}");
            None
        })
        .is_some();
        let button = if has_provider {
            Button::new("find-facts")
                .label("Find facts")
                .primary()
                .small()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_page(Page::Import, window, cx);
                    this.import_page.update(cx, |import_page, cx| {
                        import_page.find_facts(first, window, cx)
                    });
                }))
        } else {
            Button::new("set-up-ai")
                .label("Set up AI")
                .primary()
                .small()
                .on_click(|_, window, cx| window.dispatch_action(Box::new(SetUpAi), cx))
        };
        let theme = cx.theme();
        Some(
            h_flex()
                .flex_none()
                .gap_3()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.info.opacity(0.08))
                .text_sm()
                .child(Icon::new(IconName::Info).small().text_color(theme.info))
                .child(div().flex_1().child(text))
                .child(button),
        )
    }

    fn count_needs_review(&mut self, cx: &mut Context<Self>) {
        let counted = self.database.read(cx).connection().query_row(
            "SELECT count(*) FROM facts
             WHERE review IS NOT NULL AND merged_into IS NULL AND deleted_at IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        );
        match counted {
            Ok(count) => {
                self.needs_review = usize::try_from(count).unwrap_or_default();
                cx.notify();
            }
            Err(error) => eprintln!("workspace: couldn't count the facts to review: {error}"),
        }
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        let collapsed = self.sidebar_collapsed;
        self.database
            .read(cx)
            .queue(move |write| settings::set_sidebar_collapsed(write, collapsed));
        cx.notify();
    }

    /// Undoes the last change, or redoes the last undone, wherever it was
    /// made, and says so: it may be on another page.
    fn undo_or_redo(&mut self, redo: bool, window: &mut Window, cx: &mut Context<Self>) {
        let database = self.database.read(cx);
        let undone = if redo {
            database.redo(cx)
        } else {
            database.undo(cx)
        };
        cx.spawn_in(window, async move |_, cx| {
            let message = match (undone.await, redo) {
                (Ok(true), false) => "Undone",
                (Ok(true), true) => "Redone",
                (Ok(false), false) => "Nothing to undo",
                (Ok(false), true) => "Nothing to redo",
                (Err(error), _) => {
                    eprintln!("couldn't undo or redo: {error}");
                    if redo {
                        "Couldn't redo"
                    } else {
                        "Couldn't undo"
                    }
                }
            };
            if let Err(error) = cx.update(|window, cx| {
                // The kit's notifications live in its Root, which a test's
                // window may not have.
                if window.root::<Root>().flatten().is_some() {
                    window.push_notification(Notification::new().message(message), cx);
                }
            }) {
                eprintln!("workspace: {error}");
            }
        })
        .detach();
    }

    fn open_job(&mut self, job: JobId, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_page = None;
        self.page = Page::Job;
        self.job_view
            .update(cx, |job_view, cx| job_view.set_job(job, cx));
        self.job_view.focus_handle(cx).focus(window, cx);
        self.save_page(cx);
        cx.notify();
    }

    /// Opens the form a job is added with; the job opens once it's added.
    fn open_job_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| JobForm::new(self.database.clone(), window, cx));
        self._job_form = Some(
            cx.subscribe_in(&form, window, |this, _, event, window, cx| {
                window.close_dialog(cx);
                this._job_form = None;
                if let JobFormEvent::Added(job) = event {
                    this.open_job(*job, window, cx);
                }
            }),
        );
        window.open_dialog(cx, {
            let form = form.clone();
            move |dialog, _, cx| {
                let form = form.clone();
                dialog
                    .title("New job")
                    .bg(cx.theme().popover)
                    .w(px(JOB_FORM_WIDTH))
                    .content(move |content, _, _| content.child(form.clone()))
            }
        });
        window.defer(cx, move |window, cx| {
            form.update(cx, |form, cx| form.focus(window, cx))
        });
    }

    /// Back to the grid, with the job that was open still selected.
    fn open_resumes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(job) = self.job_view.read(cx).job() {
            self.resume_grid
                .update(cx, |resume_grid, cx| resume_grid.select(job, cx));
        }
        self.settings_page = None;
        self.page = Page::Resumes;
        self.resume_grid.focus_handle(cx).focus(window, cx);
        self.save_page(cx);
        cx.notify();
    }

    fn open_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_page = None;
        self.page = page;
        // The page being left may have had focus; without focus, keys like
        // Ctrl+B have nowhere to go.
        match page {
            Page::Facts => self.facts_page.focus_handle(cx).focus(window, cx),
            Page::Import => self.import_page.focus_handle(cx).focus(window, cx),
            Page::Resumes | Page::Job => {}
        }
        self.save_page(cx);
        cx.notify();
    }

    fn show_settings(
        &mut self,
        section: Option<SettingsSection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (&self.settings_page, section) {
            (Some(settings), Some(section)) => {
                settings.update(cx, |settings, cx| settings.show(section, cx))
            }
            (Some(_), None) => {}
            (None, section) => {
                let section = section.unwrap_or(SettingsSection::Appearance);
                let database = self.database.clone();
                self.settings_page =
                    Some(cx.new(|cx| SettingsPage::new(database, section, window, cx)));
            }
        }
        if let Some(settings) = &self.settings_page {
            settings.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    fn leave_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_page.take().is_none() {
            return;
        }
        let focus = match self.page {
            _ if self.onboarding.is_some() => self.import_page.focus_handle(cx),
            Page::Resumes => self.resume_grid.focus_handle(cx),
            Page::Job => self.job_view.focus_handle(cx),
            Page::Facts => self.facts_page.focus_handle(cx),
            Page::Import => self.import_page.focus_handle(cx),
        };
        focus.focus(window, cx);
        cx.notify();
    }

    fn render_sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let needs_review = self.needs_review;
        let primary = cx.theme().primary;
        let sidebar_border = cx.theme().sidebar_border;
        let nav_icon = |icon| Icon::new(icon).small().text_color(primary);
        let mut facts = SidebarMenuItem::new("Career facts")
            .icon(nav_icon(IconName::BookOpen))
            .active(self.page == Page::Facts)
            .on_click(cx.listener(|this, _, window, cx| this.open_page(Page::Facts, window, cx)));
        if needs_review > 0 {
            facts = facts.suffix(move |_, _| {
                let tooltip = SharedString::from(if needs_review == 1 {
                    "1 fact to review".to_string()
                } else {
                    format!("{needs_review} facts to review")
                });
                div()
                    .id("facts-to-review")
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    // Neutral, like Mail's unread counts: a count isn't an alarm.
                    .child(Tag::secondary().xsmall().child(needs_review.to_string()))
            });
        }

        let collapsed = self.sidebar_collapsed;
        let toggle = Button::new("toggle-sidebar")
            .ghost()
            .small()
            .icon(if collapsed {
                IconName::PanelLeftOpen
            } else {
                IconName::PanelLeftClose
            })
            .tooltip_with_action(
                if collapsed {
                    "Expand sidebar"
                } else {
                    "Collapse sidebar"
                },
                &ToggleSidebar,
                None,
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)));
        // The footer isn't one of the sidebar's groups, so it's told it's
        // collapsed and rendered here.
        let footer_menu = SidebarMenu::new()
            .gap_1()
            .collapsed(collapsed)
            .child(
                SidebarMenuItem::new("Keyboard shortcuts")
                    .h_7()
                    .icon(
                        Icon::new(gpui_kit::assets::IconName::Keyboard)
                            .small()
                            .text_color(primary),
                    )
                    .on_click(|_, window, cx| shortcuts::open(window, cx)),
            )
            .child(
                SidebarMenuItem::new("Settings")
                    .h_7()
                    .icon(nav_icon(IconName::Settings))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_settings(None, window, cx)),
                    ),
            );
        let footer_menu = footer_menu.render("footer-menu", window, cx);
        let footer = v_flex().w_full().gap_1().child(footer_menu).child(
            h_flex()
                .w_full()
                .border_t_1()
                .border_color(sidebar_border)
                .pt_1()
                .when_else(
                    collapsed,
                    |this| this.justify_center(),
                    |this| {
                        this.gap_1()
                            .child(
                                Button::new("undo")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Undo2)
                                    .tooltip_with_action("Undo", &Undo, None)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.undo_or_redo(false, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("redo")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Redo2)
                                    .tooltip_with_action("Redo", &Redo, None)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.undo_or_redo(true, window, cx)
                                    })),
                            )
                            .child(div().flex_1())
                    },
                )
                .child(toggle),
        );

        let library = SidebarMenu::new()
            .gap_1()
            .child(
                SidebarMenuItem::new("Résumés")
                    .h_7()
                    .icon(nav_icon(IconName::FileText))
                    .active(resumes_highlighted(self.page, collapsed))
                    .on_click(cx.listener(|this, _, window, cx| this.open_resumes(window, cx))),
            )
            .child(facts.h_7())
            .child(
                SidebarMenuItem::new("Import")
                    .h_7()
                    .icon(nav_icon(IconName::Inbox))
                    .active(self.page == Page::Import)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_page(Page::Import, window, cx)),
                    ),
            );
        let applications = SidebarMenu::new().gap_1().children(
            self.resume_grid.read(cx).filters().enumerate().map(
                |(index, (label, count, active))| {
                    SidebarMenuItem::new(label)
                        .h_7()
                        .pl_6()
                        .active(active)
                        .suffix(move |_, cx| {
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(count.to_string())
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.resume_grid.focus_handle(cx).focus(window, cx);
                            this.resume_grid
                                .update(cx, |grid, cx| grid.set_filter(index, cx));
                        }))
                },
            ),
        );
        Sidebar::new("sidebar")
            .collapsible(SidebarCollapsible::Icon)
            .collapsed(collapsed)
            .w(SIDEBAR_WIDTH)
            .child(SidebarGroup::new("Library").child(library))
            .when(!collapsed && self.page == Page::Resumes, |this| {
                this.child(SidebarGroup::new("Applications").child(applications.mt_0p5()))
            })
            .footer(footer)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Resumes,
    Job,
    Facts,
    Import,
}

/// Whether the sidebar's "Résumés" is highlighted. On the grid the filter
/// under it carries the highlight; collapsed, the filters aren't there to.
fn resumes_highlighted(page: Page, sidebar_collapsed: bool) -> bool {
    match page {
        Page::Job => true,
        Page::Resumes => sidebar_collapsed,
        Page::Facts | Page::Import => false,
    }
}

/// Sets the theme and motion to the stored settings, where they differ.
fn apply_settings(stored: settings::Settings, cx: &mut App) {
    if Palettes::current(cx) != stored.palettes {
        cx.set_global(stored.palettes);
    }
    if Appearance::current(cx) != stored.appearance {
        stored.appearance.set(cx);
    }
    if Motion::current(cx) != stored.motion {
        stored.motion.set(cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::database::{Error, TemporaryDirectory, Write};
    use crate::model::commands;
    use crate::model::extraction::{Extraction, FoundEntry, FoundFact};
    use crate::model::{EntryKind, FactKind};

    struct Open<'a> {
        workspace: Entity<Workspace>,
        database: Entity<Database>,
        cx: &'a mut VisualTestContext,
        /// `None` for a window on a database another test window opened.
        _directory: Option<TemporaryDirectory>,
    }

    fn open(app: &mut TestAppContext) -> Open<'_> {
        let (database, directory) = testing::open_sample(app);
        let (workspace, cx) =
            app.add_window_view(|window, cx| Workspace::new(database.clone(), window, cx));
        cx.run_until_parked();
        Open {
            workspace,
            database,
            cx,
            _directory: Some(directory),
        }
    }

    impl Open<'_> {
        fn run(&mut self, command: impl FnOnce(&mut Write) -> Result<(), Error> + Send + 'static) {
            testing::run(self.cx, &self.database, command);
        }

        fn read<R>(&mut self, read: impl FnOnce(&Workspace, &App) -> R) -> R {
            self.workspace.read_with(self.cx, read)
        }

        fn id(&mut self, sql: &str) -> uuid::Uuid {
            let id = self.database.read_with(self.cx, |database, _| {
                database.connection().query_row(sql, [], |row| row.get(0))
            });
            match id {
                Ok(id) => id,
                Err(error) => panic!("{sql}: {error}"),
            }
        }
    }

    #[test]
    fn the_badge_counts_the_facts_to_review_and_follows_changes() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        assert_eq!(open.read(|workspace, _| workspace.needs_review), 1);
        let fact = FactId(open.id("SELECT id FROM facts WHERE review IS NOT NULL"));
        open.run(move |write| commands::confirm_fact(write, fact));
        assert_eq!(open.read(|workspace, _| workspace.needs_review), 0);
    }

    #[test]
    fn ctrl_z_undoes_the_last_change_and_ctrl_y_redoes_it() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        let fact = FactId(open.id("SELECT id FROM facts WHERE review IS NOT NULL"));
        open.run(move |write| commands::confirm_fact(write, fact));
        assert_eq!(open.read(|workspace, _| workspace.needs_review), 0);
        let keys = |open: &mut Open, keystrokes: &str| {
            open.cx.simulate_keystrokes(keystrokes);
            testing::settle(open.cx, &open.database);
        };
        keys(&mut open, "ctrl-z");
        assert_eq!(open.read(|workspace, _| workspace.needs_review), 1);
        keys(&mut open, "ctrl-y");
        assert_eq!(open.read(|workspace, _| workspace.needs_review), 0);
    }

    #[test]
    fn show_fact_opens_the_facts_page_on_it() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        let fact = FactId(open.id("SELECT id FROM facts WHERE text LIKE 'Cut p99%'"));
        open.cx.dispatch_action(ShowFact { fact });
        open.cx.run_until_parked();
        assert!(open.read(|workspace, _| workspace.page == Page::Facts));
        let selected = open.read(|workspace, cx| workspace.facts_page.read(cx).selected());
        assert_eq!(selected, Some(fact));
    }

    #[test]
    fn a_document_without_facts_is_offered_until_they_are_found() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        assert_eq!(open.read(|workspace, _| workspace.unread.len()), 0);
        open.run(|write| commands::import_document(write, None, "Rust\n").map(|_| ()));
        let document =
            DocumentId(open.id("SELECT id FROM documents ORDER BY added_at DESC LIMIT 1"));
        assert_eq!(
            open.read(|workspace, _| workspace.unread.clone()),
            [(document, None)]
        );
        let found = Extraction {
            profile: Vec::new(),
            entries: vec![FoundEntry {
                kind: EntryKind::Skills,
                section: None,
                facts: vec![FoundFact {
                    kind: FactKind::Skill,
                    text: "Rust".into(),
                    source: Some((0..4, "Rust".into())),
                    review: None,
                }],
            }],
        };
        open.run(move |write| extraction::save(write, document, &found).map(|_| ()));
        assert_eq!(open.read(|workspace, _| workspace.unread.len()), 0);
    }

    #[test]
    fn set_up_ai_opens_settings_on_ai_and_show_facts_the_facts() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.cx.dispatch_action(SetUpAi);
        open.cx.run_until_parked();
        assert_eq!(
            open.read(|workspace, cx| {
                let section = workspace
                    .settings_page
                    .as_ref()
                    .map(|settings| settings.read(cx).opened_on());
                (workspace.page, section)
            }),
            (Page::Resumes, Some(SettingsSection::Ai)),
            "Settings opens on AI without replacing the library"
        );
        open.cx.dispatch_action(ShowFacts);
        open.cx.run_until_parked();
        assert_eq!(open.read(|workspace, _| workspace.page), Page::Facts);
    }

    #[test]
    fn set_up_ai_while_starting_out_opens_settings_over_onboarding() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_empty(&mut app);
        let (workspace, cx) =
            app.add_window_view(|window, cx| Workspace::new(database.clone(), window, cx));
        cx.run_until_parked();
        cx.dispatch_action(SetUpAi);
        cx.run_until_parked();
        let shown = workspace.read_with(cx, |workspace, cx| {
            (
                workspace.onboarding.is_some(),
                workspace
                    .settings_page
                    .as_ref()
                    .map(|settings| settings.read(cx).opened_on()),
            )
        });
        assert_eq!(shown, (true, Some(SettingsSection::Ai)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        let shown = workspace.read_with(cx, |workspace, _| {
            (
                workspace.onboarding.is_some(),
                workspace.settings_page.is_some(),
            )
        });
        assert_eq!(
            shown,
            (true, false),
            "closing Settings goes back to onboarding"
        );
    }

    #[test]
    fn settings_closes_back_to_the_page_it_was_opened_over() {
        use gpui_kit::test::TestWindowExt;

        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.cx.dispatch_action(ShowFacts);
        open.cx.simulate_keystrokes("ctrl-,");
        open.cx.run_until_parked();
        assert!(open.read(|workspace, _| workspace.settings_page.is_some()));
        open.cx
            .update(|window, cx| window.click("close-settings", cx));
        open.cx.run_until_parked();
        assert_eq!(
            open.read(|workspace, cx| (
                workspace.settings_page.is_none(),
                workspace.page,
                cx.windows().len()
            )),
            (true, Page::Facts, 1)
        );

        open.cx.simulate_keystrokes("ctrl-,");
        open.cx.run_until_parked();
        open.cx.simulate_keystrokes("escape");
        open.cx.run_until_parked();
        assert_eq!(
            open.read(|workspace, _| (workspace.settings_page.is_none(), workspace.page)),
            (true, Page::Facts),
            "Esc closes it too"
        );
    }

    #[test]
    fn on_the_grid_only_the_filter_is_highlighted_in_the_sidebar() {
        assert!(!resumes_highlighted(Page::Resumes, false), "the filter is");
        assert!(
            resumes_highlighted(Page::Resumes, true),
            "no filters when collapsed"
        );
        assert!(resumes_highlighted(Page::Job, false), "no filters in a job");
        assert!(!resumes_highlighted(Page::Facts, false));
    }

    #[test]
    fn open_line_opens_the_job_with_the_line_selected() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        let job = JobId(open.id("SELECT id FROM jobs WHERE company = 'Ledgerly'"));
        let line = LineId(open.id("SELECT lines.id FROM lines
             JOIN blocks ON blocks.id = lines.block_id
             JOIN sections ON sections.id = blocks.section_id
             JOIN current_generations ON current_generations.generation_id = sections.generation_id
             JOIN jobs ON jobs.id = current_generations.job_id
             WHERE jobs.company = 'Ledgerly' AND current_generations.kind = 'resume'
             LIMIT 1"));
        open.cx.dispatch_action(OpenLine { job, line });
        open.cx.run_until_parked();
        assert!(open.read(|workspace, _| workspace.page == Page::Job));
        let shown = open.read(|workspace, cx| {
            let job_view = workspace.job_view.read(cx);
            (job_view.job(), job_view.selected_line())
        });
        assert_eq!(shown, (Some(job), Some(line)));
    }

    /// A second window on the same database, as the next start would open.
    fn reopen<'a>(app: &'a mut TestAppContext, database: &Entity<Database>) -> Open<'a> {
        let (workspace, cx) =
            app.add_window_view(|window, cx| Workspace::new(database.clone(), window, cx));
        cx.run_until_parked();
        Open {
            workspace,
            database: database.clone(),
            cx,
            _directory: None,
        }
    }

    fn device_settings(open: &mut Open) -> DeviceSettings {
        let read = open.database.read_with(open.cx, |database, _| {
            DeviceSettings::read(database.device_connection())
        });
        match read {
            Ok(settings) => settings,
            Err(error) => panic!("couldn't read the device settings: {error}"),
        }
    }

    #[test]
    fn the_sidebar_and_page_open_as_they_were_left() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_sample(&mut app);
        {
            let mut open = reopen(&mut app, &database);
            open.cx.dispatch_action(ToggleSidebar);
            open.workspace.update_in(open.cx, |workspace, window, cx| {
                workspace.open_page(Page::Facts, window, cx)
            });
            testing::settle(open.cx, &database);
            let saved = device_settings(&mut open);
            assert!(saved.sidebar_collapsed);
            assert_eq!(saved.last_page, LastPage::Facts);
        }
        let mut open = reopen(&mut app, &database);
        assert!(open.read(|workspace, _| workspace.sidebar_collapsed));
        assert!(open.read(|workspace, _| workspace.page == Page::Facts));
    }

    #[test]
    fn the_job_page_opens_on_the_job_it_was_left_on() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_sample(&mut app);
        let job = {
            let mut open = reopen(&mut app, &database);
            let job = JobId(open.id("SELECT id FROM jobs WHERE company = 'Ledgerly'"));
            open.workspace.update_in(open.cx, |workspace, window, cx| {
                workspace.open_job(job, window, cx)
            });
            testing::settle(open.cx, &database);
            assert_eq!(device_settings(&mut open).last_page, LastPage::Job(job));
            job
        };
        let mut open = reopen(&mut app, &database);
        assert!(open.read(|workspace, _| workspace.page == Page::Job));
        assert_eq!(
            open.read(|workspace, cx| workspace.job_view.read(cx).job()),
            Some(job)
        );

        open.workspace.update_in(open.cx, |workspace, window, cx| {
            workspace.open_resumes(window, cx)
        });
        testing::settle(open.cx, &database);
        assert_eq!(device_settings(&mut open).last_page, LastPage::Resumes);
    }

    #[test]
    fn a_job_gone_since_opens_the_grid_instead() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_sample(&mut app);
        database.read_with(&app, |database, _| {
            database.queue(|write| settings::set_last_page(write, LastPage::Job(JobId::new())));
            database.flush();
        });
        let mut open = reopen(&mut app, &database);
        assert!(open.read(|workspace, _| workspace.page == Page::Resumes));
        testing::settle(open.cx, &database);
        assert_eq!(device_settings(&mut open).last_page, LastPage::Resumes);
    }

    #[test]
    fn the_theme_and_motion_follow_the_stored_settings() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        open.run(|write| {
            settings::set_appearance(write, Appearance::Dark)?;
            settings::set_palette(write, crate::appearance::Palette::Moss)?;
            settings::set_palette(write, crate::appearance::Palette::Glacier)?;
            settings::set_motion(write, Motion::Reduced)
        });
        assert_eq!(open.read(|_, cx| Appearance::current(cx)), Appearance::Dark);
        assert_eq!(open.read(|_, cx| Motion::current(cx)), Motion::Reduced);
        assert_eq!(
            open.read(|_, cx| cx.theme().primary),
            gpui_kit::Hsla::from(gpui_kit::rgb(0x93c4ac)),
        );
        open.run(|write| settings::set_appearance(write, Appearance::Light));
        assert_eq!(
            open.read(|_, cx| Appearance::current(cx)),
            Appearance::Light
        );
        assert_eq!(
            open.read(|_, cx| cx.theme().primary),
            gpui_kit::Hsla::from(gpui_kit::rgb(0x376dbb)),
        );
    }
}

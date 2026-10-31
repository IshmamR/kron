mod application_view;
mod fact_list;
mod facts_page;
mod import_page;
mod inspector;
mod job_view;
mod parts;
mod previews;
mod resume_grid;
mod shortcuts;
#[cfg(test)]
mod testing;

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::sidebar::{Sidebar, SidebarCollapsible, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{IconName, Root, Sizable, WindowExt, h_flex};
use gpui_kit::{
    Action, AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, IntoElement,
    KeyBinding, Pixels, SharedString, Subscription, Task, Window, WindowBounds, actions, div,
    prelude::*, px,
};

use crate::appearance::{Appearance, Motion};
use crate::database::schema::{self, Facts};
use crate::database::{Changed, Database};
use crate::model::settings::{self, DeviceSettings, LastPage};
use crate::model::{Application, FactId, JobId, LineId};
use crate::window;
use facts_page::FactsPage;
use import_page::ImportPage;
use job_view::{JobView, JobViewEvent};
use previews::Previews;
use resume_grid::{ResumeGrid, ResumeGridEvent};

/// Fixed rather than resizable: the kit animates a sidebar of fixed width
/// down to its icons, and three short labels don't need more room.
const SIDEBAR_WIDTH: Pixels = px(220.);
/// The collapse toggle's width: a collapsed menu item's, the strip's 48px
/// less its padding.
const TOGGLE_WIDTH: Pixels = px(32.);
/// How long the window has to stay put before where it is gets saved, so a
/// drag saves once, not on every step.
const WINDOW_SAVE_DELAY: Duration = Duration::from_millis(500);

actions!(workspace, [ToggleSidebar, ShowShortcuts, Undo, Redo]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-b", ToggleSidebar, None),
        KeyBinding::new("secondary-/", ShowShortcuts, None),
        // A text field's own undo comes first: its key context is deeper.
        KeyBinding::new("secondary-z", Undo, None),
        KeyBinding::new("secondary-y", Redo, None),
    ]);
    facts_page::init(cx);
    import_page::init(cx);
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
    /// Focused on pages with nothing focusable of their own, so app-wide keys
    /// still arrive.
    focus_handle: FocusHandle,
    resume_grid: Entity<ResumeGrid>,
    job_view: Entity<JobView>,
    facts_page: Entity<FactsPage>,
    import_page: Entity<ImportPage>,
    _subscriptions: Vec<Subscription>,
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.page {
            Page::Resumes => self.resume_grid.clone().into_any_element(),
            Page::Job => self.job_view.clone().into_any_element(),
            Page::Facts => self.facts_page.clone().into_any_element(),
            Page::Import => self.import_page.clone().into_any_element(),
            Page::Settings => div()
                .track_focus(&self.focus_handle)
                .size_full()
                .child(settings_page(self.database.clone()))
                .into_any_element(),
        };

        div()
            .on_action(cx.listener(|this, action: &OpenLine, window, cx| {
                this.open_job(action.job, window, cx);
                this.job_view
                    .update(cx, |job_view, cx| job_view.select_line(action.line, cx));
            }))
            .on_action(cx.listener(|this, action: &ShowFact, window, cx| {
                this.page = Page::Facts;
                this.facts_page.update(cx, |facts_page, cx| {
                    facts_page.show_fact(action.fact, window, cx)
                });
                this.save_page(cx);
                cx.notify();
            }))
            .on_action(|_: &ShowShortcuts, window, cx| shortcuts::open(window, cx))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &Undo, window, cx| this.undo_or_redo(false, window, cx)))
            .on_action(cx.listener(|this, _: &Redo, window, cx| this.undo_or_redo(true, window, cx)))
            .size_full()
            .child(
                h_flex()
                    .size_full()
                    .child(self.render_sidebar(cx))
                    .child(div().flex_1().min_w_0().h_full().child(page)),
            )
    }
}

impl Workspace {
    pub fn new(
        database: Entity<Database>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
        let resume_grid = cx.new(|cx| {
            ResumeGrid::new(database.clone(), previews.clone(), cx)
        });
        let job_view = cx.new(|cx| {
            JobView::new(database.clone(), previews, window, cx)
        });
        let facts_page =
            cx.new(|cx| FactsPage::new(database.clone(), window, cx));
        let import_page = cx.new(|cx| ImportPage::new(database.clone(), window, cx));
        resume_grid.focus_handle(cx).focus(window, cx);

        let subscriptions = vec![
            cx.subscribe(&database, |this, database, changed: &Changed, cx| {
                if changed.touches(Facts) {
                    this.count_needs_review(cx);
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
            cx.observe_window_appearance(window, |_, window, cx| {
                let appearance = Appearance::current(cx);
                if appearance == Appearance::System {
                    appearance.apply(window, cx);
                }
            }),
        ];

        let mut workspace = Self {
            needs_review: 0,
            database,
            page: Page::Resumes,
            sidebar_collapsed: device.sidebar_collapsed,
            saved_page: device.last_page,
            saving_window: Task::ready(()),
            // Always opened windowed, then maximized once mapped.
            windowed_bounds: window.bounds(),
            focus_handle: cx.focus_handle(),
            resume_grid,
            job_view,
            facts_page,
            import_page,
            _subscriptions: subscriptions,
        };
        workspace.count_needs_review(cx);
        workspace.restore_page(device.last_page, window, cx);
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
            LastPage::Settings => self.open_page(Page::Settings, window, cx),
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
            Page::Settings => LastPage::Settings,
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
        let undone = if redo { database.redo(cx) } else { database.undo(cx) };
        cx.spawn_in(window, async move |_, cx| {
            let message = match (undone.await, redo) {
                (Ok(true), false) => "Undone",
                (Ok(true), true) => "Redone",
                (Ok(false), false) => "Nothing to undo",
                (Ok(false), true) => "Nothing to redo",
                (Err(error), _) => {
                    eprintln!("couldn't undo or redo: {error}");
                    if redo { "Couldn't redo" } else { "Couldn't undo" }
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
        self.page = Page::Job;
        self.job_view
            .update(cx, |job_view, cx| job_view.set_job(job, cx));
        self.job_view.focus_handle(cx).focus(window, cx);
        self.save_page(cx);
        cx.notify();
    }

    /// Back to the grid, with the job that was open still selected.
    fn open_resumes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(job) = self.job_view.read(cx).job() {
            self.resume_grid
                .update(cx, |resume_grid, cx| resume_grid.select(job, cx));
        }
        self.page = Page::Resumes;
        self.resume_grid.focus_handle(cx).focus(window, cx);
        self.save_page(cx);
        cx.notify();
    }

    fn open_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        // The page being left may have had focus; without focus, keys like
        // Ctrl+B have nowhere to go.
        match page {
            Page::Facts => self.facts_page.focus_handle(cx).focus(window, cx),
            Page::Import => self.import_page.focus_handle(cx).focus(window, cx),
            Page::Settings => self.focus_handle.focus(window, cx),
            Page::Resumes | Page::Job => {}
        }
        self.save_page(cx);
        cx.notify();
    }

    fn render_sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let needs_review = self.needs_review;
        let mut facts = SidebarMenuItem::new("Facts")
            .icon(IconName::BookOpen)
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
        let footer = h_flex()
            .w_full()
            .justify_between()
            .child(
                Button::new("keyboard-shortcuts")
                    .ghost()
                    .small()
                    .label("Keyboard shortcuts")
                    .on_click(|_, window, cx| shortcuts::open(window, cx)),
            )
            .children(
                shortcuts::keystrokes_for(&ShowShortcuts, cx)
                    .into_iter()
                    .next()
                    .map(Kbd::new),
            );

        let item = |item: SidebarMenuItem| item.h_7();
        let menu = SidebarMenu::new()
            .child(item(
                SidebarMenuItem::new("Résumés")
                    .icon(IconName::FileText)
                    .active(matches!(self.page, Page::Resumes | Page::Job))
                    .on_click(cx.listener(|this, _, window, cx| this.open_resumes(window, cx))),
            ))
            .child(item(facts))
            .child(item(
                SidebarMenuItem::new("Import")
                    .icon(IconName::Inbox)
                    .active(self.page == Page::Import)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_page(Page::Import, window, cx)
                    })),
            ))
            .child(item(
                SidebarMenuItem::new("Settings")
                    .icon(IconName::Settings)
                    .active(self.page == Page::Settings)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_page(Page::Settings, window, cx)
                    })),
            ));
        // At the top of the sidebar, as in Windows' navigation pane: always
        // there, its icon in the column of the menu's, so it stays put as
        // the sidebar collapses. Sized like a menu item.
        let toggle = Button::new("toggle-sidebar")
            .ghost()
            .w(TOGGLE_WIDTH)
            .h_7()
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

        // Collapsed, the kit pads the header 4px less, the menu 8px on top
        // and 8px at the sides against none and 12px expanded, puts a gap
        // between the two, and centres the icons. Matching that when
        // expanded keeps every icon where it is on toggling; items are kept
        // as tall either way above. Both then move the menu up, closing
        // most of the kit's gap under the toggle: collapsed by shortening the
        // header, as the menu's own area clips what moves out of it.
        let (toggle, menu) = if collapsed {
            (toggle.mb(px(-15.)), menu)
        } else {
            (toggle.mt(px(-4.)).ml(px(-4.)), menu.mx(px(-4.)))
        };

        Sidebar::new("sidebar")
            .collapsible(SidebarCollapsible::Icon)
            .collapsed(collapsed)
            .w(SIDEBAR_WIDTH)
            .header(toggle)
            .child(menu)
            .when(!collapsed, |this| this.footer(footer))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Resumes,
    Job,
    Facts,
    Import,
    Settings,
}

/// Sets the theme and motion to the stored settings, where they differ.
fn apply_settings(stored: settings::Settings, cx: &mut App) {
    if Appearance::current(cx) != stored.appearance {
        stored.appearance.set(cx);
    }
    if Motion::current(cx) != stored.motion {
        stored.motion.set(cx);
    }
}

/// Each choice is written to `settings`; the workspace applies it once the
/// write commits.
fn settings_page(database: Entity<Database>) -> AnyElement {
    let options = Appearance::ALL
        .into_iter()
        .map(|appearance| {
            (
                SharedString::from(appearance.stored()),
                SharedString::from(appearance.label()),
            )
        })
        .collect();

    Settings::new("settings")
        .with_group_variant(GroupBoxVariant::Outline)
        .page(
            SettingPage::new("General").group(
                SettingGroup::new().title("Appearance").item(
                    SettingItem::new(
                        "Theme",
                        SettingField::dropdown(
                            options,
                            |cx| Appearance::current(cx).stored().into(),
                            {
                                let database = database.clone();
                                move |key, cx| {
                                    if let Some(appearance) = Appearance::from_stored(&key) {
                                        database.read(cx).queue(move |write| {
                                            settings::set_appearance(write, appearance)
                                        });
                                    }
                                }
                            },
                        ),
                    )
                    .description("Match system follows your desktop's light or dark setting."),
                )
                .item(
                    SettingItem::new(
                        "Motion",
                        SettingField::dropdown(
                            Motion::ALL
                                .into_iter()
                                .map(|motion| {
                                    (
                                        SharedString::from(motion.stored()),
                                        SharedString::from(motion.label()),
                                    )
                                })
                                .collect(),
                            |cx| Motion::current(cx).stored().into(),
                            move |key, cx| {
                                if let Some(motion) = Motion::from_stored(&key) {
                                    database
                                        .read(cx)
                                        .queue(move |write| settings::set_motion(write, motion));
                                }
                            },
                        ),
                    )
                    .description(
                        "Reduced stops things sliding, like the sidebar collapsing. Match system \
                         follows your desktop's reduce-animation setting.",
                    ),
                ),
            ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::database::{Error, TemporaryDirectory, Write};
    use crate::model::commands;

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
    fn open_line_opens_the_job_with_the_line_selected() {
        let mut app = TestAppContext::single();
        let mut open = open(&mut app);
        let job = JobId(open.id("SELECT id FROM jobs WHERE company = 'Ledgerly'"));
        let line = LineId(open.id(
            "SELECT lines.id FROM lines
             JOIN blocks ON blocks.id = lines.block_id
             JOIN sections ON sections.id = blocks.section_id
             JOIN current_generations ON current_generations.generation_id = sections.generation_id
             JOIN jobs ON jobs.id = current_generations.job_id
             WHERE jobs.company = 'Ledgerly' AND current_generations.kind = 'resume'
             LIMIT 1",
        ));
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
            open.workspace
                .update_in(open.cx, |workspace, window, cx| workspace.open_job(job, window, cx));
            testing::settle(open.cx, &database);
            assert_eq!(device_settings(&mut open).last_page, LastPage::Job(job));
            job
        };
        let mut open = reopen(&mut app, &database);
        assert!(open.read(|workspace, _| workspace.page == Page::Job));
        assert_eq!(open.read(|workspace, cx| workspace.job_view.read(cx).job()), Some(job));

        open.workspace
            .update_in(open.cx, |workspace, window, cx| workspace.open_resumes(window, cx));
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
            settings::set_motion(write, Motion::Reduced)
        });
        assert_eq!(open.read(|_, cx| Appearance::current(cx)), Appearance::Dark);
        assert_eq!(open.read(|_, cx| Motion::current(cx)), Motion::Reduced);
        open.run(|write| settings::set_appearance(write, Appearance::Light));
        assert_eq!(open.read(|_, cx| Appearance::current(cx)), Appearance::Light);
    }
}

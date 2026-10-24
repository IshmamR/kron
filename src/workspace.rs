mod application_view;
mod fact_list;
mod facts_page;
mod inspector;
mod job_view;
mod parts;
mod previews;
mod resume_grid;
mod shortcuts;
#[cfg(test)]
mod testing;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::sidebar::{Sidebar, SidebarCollapsible, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{IconName, Sizable, h_flex};
use gpui_kit::{
    Action, AnyElement, App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding,
    Pixels, SharedString, Subscription, Window, actions, div, prelude::*, px,
};

use crate::appearance::{Appearance, Motion};
use crate::database::schema::Facts;
use crate::database::{Changed, Database};
use crate::model::{FactId, JobId, LineId};
use facts_page::FactsPage;
use job_view::{JobView, JobViewEvent};
use previews::Previews;
use resume_grid::{ResumeGrid, ResumeGridEvent};

/// Fixed rather than resizable: the kit animates a sidebar of fixed width
/// down to its icons, and three short labels don't need more room.
const SIDEBAR_WIDTH: Pixels = px(220.);
/// The collapse toggle's width: a collapsed menu item's, the strip's 48px
/// less its padding.
const TOGGLE_WIDTH: Pixels = px(32.);

actions!(workspace, [ToggleSidebar, ShowShortcuts]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-b", ToggleSidebar, None),
        KeyBinding::new("secondary-/", ShowShortcuts, None),
    ]);
    facts_page::init(cx);
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
    /// Focused on pages with nothing focusable of their own, so app-wide keys
    /// still arrive.
    focus_handle: FocusHandle,
    resume_grid: Entity<ResumeGrid>,
    job_view: Entity<JobView>,
    facts_page: Entity<FactsPage>,
    _subscriptions: Vec<Subscription>,
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.page {
            Page::Resumes => self.resume_grid.clone().into_any_element(),
            Page::Job => self.job_view.clone().into_any_element(),
            Page::Facts => self.facts_page.clone().into_any_element(),
            Page::Settings => div()
                .track_focus(&self.focus_handle)
                .size_full()
                .child(settings())
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
                cx.notify();
            }))
            .on_action(|_: &ShowShortcuts, window, cx| shortcuts::open(window, cx))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
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
        Appearance::current(cx).apply(window, cx);

        let previews = cx.new(|cx| Previews::new(database.clone(), cx));
        let resume_grid = cx.new(|cx| {
            ResumeGrid::new(database.clone(), previews.clone(), cx)
        });
        let job_view = cx.new(|cx| {
            JobView::new(database.clone(), previews, window, cx)
        });
        let facts_page =
            cx.new(|cx| FactsPage::new(database.clone(), window, cx));
        resume_grid.focus_handle(cx).focus(window, cx);

        let subscriptions = vec![
            cx.subscribe(&database, |this, _, changed: &Changed, cx| {
                if changed.touches(Facts) {
                    this.count_needs_review(cx);
                }
            }),
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
            sidebar_collapsed: false,
            focus_handle: cx.focus_handle(),
            resume_grid,
            job_view,
            facts_page,
            _subscriptions: subscriptions,
        };
        workspace.count_needs_review(cx);
        workspace
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
        cx.notify();
    }

    fn open_job(&mut self, job: JobId, window: &mut Window, cx: &mut Context<Self>) {
        self.page = Page::Job;
        self.job_view
            .update(cx, |job_view, cx| job_view.set_job(job, cx));
        self.job_view.focus_handle(cx).focus(window, cx);
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
        cx.notify();
    }

    fn open_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        // The page being left may have had focus; without focus, keys like
        // Ctrl+B have nowhere to go.
        match page {
            Page::Facts => self.facts_page.focus_handle(cx).focus(window, cx),
            Page::Settings => self.focus_handle.focus(window, cx),
            Page::Resumes | Page::Job => {}
        }
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
    Settings,
}

fn settings() -> AnyElement {
    let options = Appearance::ALL
        .into_iter()
        .map(|appearance| (appearance.key(), SharedString::from(appearance.label())))
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
                            |cx| Appearance::current(cx).key(),
                            |key, cx| {
                                if let Some(appearance) = Appearance::from_key(&key) {
                                    appearance.set(cx);
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
                                .map(|motion| (motion.key(), SharedString::from(motion.label())))
                                .collect(),
                            |cx| Motion::current(cx).key(),
                            |key, cx| {
                                if let Some(motion) = Motion::from_key(&key) {
                                    motion.set(cx);
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
        _directory: TemporaryDirectory,
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
            _directory: directory,
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
}

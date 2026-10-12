mod fact_list;
mod facts_page;
mod inspector;
mod job_view;
mod parts;
mod previews;
mod resume_grid;
mod shortcuts;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::resizable::{ResizableState, h_resizable, resizable_panel};
use gpui_kit::component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{IconName, Sizable, h_flex};
use gpui_kit::{
    Action, AnyElement, App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Pixels, SharedString,
    Subscription, Window, actions, div, prelude::*, px,
};

use crate::appearance::Appearance;
use crate::model::{FactId, JobId, Library, LineId};
use facts_page::FactsPage;
use job_view::{JobView, JobViewEvent};
use previews::Previews;
use resume_grid::{ResumeGrid, ResumeGridEvent};

const SIDEBAR_WIDTH: Pixels = px(220.);
/// Narrower clips the footer's shortcut.
const SIDEBAR_MIN_WIDTH: Pixels = px(210.);
const SIDEBAR_MAX_WIDTH: Pixels = px(320.);

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
    library: Entity<Library>,
    page: Page,
    sidebar_visible: bool,
    /// Focused on pages with nothing focusable of their own, so app-wide keys
    /// still arrive.
    focus_handle: FocusHandle,
    /// The sidebar's width, kept while it's hidden.
    layout: Entity<ResizableState>,
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
                this.facts_page
                    .update(cx, |facts_page, cx| facts_page.show_fact(action.fact, window, cx));
                cx.notify();
            }))
            .on_action(|_: &ShowShortcuts, window, cx| shortcuts::open(window, cx))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .size_full()
            .child(
                h_resizable("workspace")
                    .with_state(&self.layout)
                    .child(
                        resizable_panel()
                            .visible(self.sidebar_visible)
                            .size(SIDEBAR_WIDTH)
                            .size_range(SIDEBAR_MIN_WIDTH..SIDEBAR_MAX_WIDTH)
                            .child(self.render_sidebar(cx)),
                    )
                    .child(resizable_panel().child(page)),
            )
    }
}

impl Workspace {
    pub fn new(library: Entity<Library>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Appearance::current(cx).apply(window, cx);

        let previews = cx.new(|cx| Previews::new(library.clone(), cx));
        let resume_grid = cx.new(|cx| ResumeGrid::new(library.clone(), previews.clone(), cx));
        let job_view = cx.new(|cx| JobView::new(library.clone(), previews, cx));
        let facts_page = cx.new(|cx| FactsPage::new(library.clone(), window, cx));
        resume_grid.focus_handle(cx).focus(window, cx);

        let subscriptions = vec![
            cx.observe(&library, |_, _, cx| cx.notify()),
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

        Self {
            library,
            page: Page::Resumes,
            sidebar_visible: true,
            focus_handle: cx.focus_handle(),
            layout: cx.new(|_| ResizableState::default()),
            resume_grid,
            job_view,
            facts_page,
            _subscriptions: subscriptions,
        }
    }

    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
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
        let unreviewed = self.library.read(cx).unreviewed_count();
        let mut facts = SidebarMenuItem::new("Facts")
            .icon(IconName::BookOpen)
            .active(self.page == Page::Facts)
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_page(Page::Facts, window, cx)
            }));
        if unreviewed > 0 {
            facts = facts.suffix(move |_, _| {
                let tooltip = SharedString::from(if unreviewed == 1 {
                    "1 fact to review".to_string()
                } else {
                    format!("{unreviewed} facts to review")
                });
                div()
                    .id("unreviewed-facts")
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    // Neutral, like Mail's unread counts: a count isn't an alarm.
                    .child(Tag::secondary().xsmall().child(unreviewed.to_string()))
            });
        }

        Sidebar::new("sidebar").collapsible(false).w_full().child(
            SidebarMenu::new()
                .child(
                    SidebarMenuItem::new("Résumés")
                        .icon(IconName::FileText)
                        .active(matches!(self.page, Page::Resumes | Page::Job))
                        .on_click(cx.listener(|this, _, window, cx| this.open_resumes(window, cx))),
                )
                .child(facts)
                .child(
                    SidebarMenuItem::new("Settings")
                        .icon(IconName::Settings)
                        .active(self.page == Page::Settings)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_page(Page::Settings, window, cx)
                        })),
                ),
        )
        .footer(
            h_flex()
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
                ),
        )
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
                ),
            ),
        )
        .into_any_element()
}

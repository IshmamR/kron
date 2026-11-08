// Settings, taking over the window until it's closed, like Discord's and
// Slack's: the kit's settings pages, with a close button over them.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::setting::{
    SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::{ActiveTheme, IconName, h_flex};
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, SharedString,
    Subscription, Window, div, prelude::*, px,
};

use super::LeaveSettings;
use super::ai_settings::AiSettings;
use super::palette_picker::palette_picker;
use super::stand_in_name::StandInName;
use crate::appearance::{Appearance, Motion, Palette};
use crate::database::schema::Settings as SettingsTable;
use crate::database::{Changed, Database};
use crate::model::settings;

const KEY_CONTEXT: &str = "SettingsPage";

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", LeaveSettings, Some(KEY_CONTEXT))]);
}

pub struct SettingsPage {
    database: Entity<Database>,
    ai_settings: Entity<AiSettings>,
    stand_in_name: Entity<StandInName>,
    /// The kit keeps the page picked under the settings' id; a new one opens
    /// on `start` instead.
    generation: usize,
    start: SettingsSection,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Render for SettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .relative()
            .size_full()
            .child(
                Settings::new(("settings", self.generation))
                    .with_group_variant(GroupBoxVariant::Outline)
                    .default_selected_index(SelectIndex {
                        page_ix: self.start as usize,
                        group_ix: None,
                    })
                    .page(self.appearance_page())
                    .page(self.ai_page(cx)),
            )
            .child(
                // Level with the page's title, in its header.
                h_flex()
                    .absolute()
                    .top(px(14.))
                    .right_4()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Esc"),
                    )
                    .child(
                        Button::new("close-settings")
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip_with_action("Close settings", &LeaveSettings, None)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(LeaveSettings), cx)
                            }),
                    ),
            )
    }
}

impl Focusable for SettingsPage {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SettingsPage {
    pub fn new(
        database: Entity<Database>,
        start: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ai_settings = cx.new(|cx| AiSettings::new(database.clone(), window, cx));
        let stand_in_name = cx.new(|cx| StandInName::new(database.clone(), window, cx));
        let subscriptions = vec![
            cx.subscribe(&database, |_, _, changed: &Changed, cx| {
                if changed.touches(SettingsTable) {
                    cx.notify();
                }
            }),
            // Its rows are drawn by the page.
            cx.observe(&ai_settings, |_, _, cx| cx.notify()),
        ];
        Self {
            database,
            ai_settings,
            stand_in_name,
            generation: 0,
            start,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// Opens on `section`, wherever it was.
    pub fn show(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        self.start = section;
        self.generation += 1;
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn opened_on(&self) -> SettingsSection {
        self.start
    }

    fn appearance_page(&self) -> SettingPage {
        let database = self.database.clone();
        let palette_names = |dark: bool| {
            Palette::ALL
                .into_iter()
                .filter(move |palette| palette.is_dark() == dark)
                .map(|palette| SharedString::from(palette.label()))
        };
        let palette = |dark: bool, title: &'static str, description: &'static str| {
            let database = database.clone();
            SettingItem::new(
                title,
                SettingField::render(move |_, _, cx| palette_picker(dark, database.clone(), cx)),
            )
            .description(description)
            .keywords(palette_names(dark).chain(["theme".into(), "color".into()]))
        };
        SettingPage::new("Appearance")
            .icon(IconName::Palette)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(
                        SettingItem::new(
                            "Color mode",
                            SettingField::dropdown(
                                Appearance::ALL
                                    .into_iter()
                                    .map(|appearance| {
                                        (appearance.stored().into(), appearance.label().into())
                                    })
                                    .collect(),
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
                        .description("Match system follows your desktop's light or dark setting.")
                        .keywords(["theme", "light", "dark"]),
                    )
                    .item(palette(
                        false,
                        "Light palette",
                        "The colors Kron uses while it's light.",
                    ))
                    .item(palette(
                        true,
                        "Dark palette",
                        "The colors Kron uses while it's dark.",
                    ))
                    .item(
                        SettingItem::new(
                            "Animations",
                            SettingField::dropdown(
                                Motion::ALL
                                    .into_iter()
                                    .map(|motion| (motion.stored().into(), motion.label().into()))
                                    .collect(),
                                |cx| Motion::current(cx).stored().into(),
                                move |key, cx| {
                                    if let Some(motion) = Motion::from_stored(&key) {
                                        database.read(cx).queue(move |write| {
                                            settings::set_motion(write, motion)
                                        });
                                    }
                                },
                            ),
                        )
                        .description(
                            "Reduced stops things sliding, like the sidebar collapsing. Match \
                             system follows your desktop's reduce-animation setting.",
                        )
                        .keywords(["motion"]),
                    ),
            )
    }

    /// The providers set up, each with its own key, the model each task
    /// runs on, and the name sent in place of the user's.
    fn ai_page(&self, cx: &App) -> SettingPage {
        let ai_settings = self.ai_settings.clone();
        // A row like the others, with nothing on its right: the buttons to
        // add go under the list.
        let heading = SettingItem::new("Providers", SettingField::render(|_, _, _| div()))
            .description(
                "Keys are kept in your system keyring, never in Kron's data. Your name and \
                 contact details are hidden before anything is sent.",
            )
            .keywords(["api key", "credentials", "keyring", "ollama", "server"]);
        let add = {
            let ai_settings = ai_settings.clone();
            SettingItem::render(move |_, _, cx| {
                ai_settings.update(cx, |settings, cx| settings.render_add(cx))
            })
        };
        let providers = self.ai_settings.read(cx).providers();
        let listed: Vec<_> = providers
            .iter()
            .map(|provider| {
                let ai_settings = ai_settings.clone();
                let row = provider.clone();
                let item = SettingItem::new(
                    provider.name.clone(),
                    SettingField::render(move |_, _, cx| {
                        ai_settings
                            .update(cx, |settings, cx| settings.render_provider(row.clone(), cx))
                    }),
                );
                match &provider.base_url {
                    Some(url) => item.description(url.clone()),
                    None => item,
                }
            })
            .collect();
        let empty = providers.is_empty().then(|| {
            SettingItem::render(|_, _, cx| {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No providers yet. Add one to find facts and write résumés.")
            })
        });
        let picker = |index: usize, title: &'static str, description: &'static str| {
            let ai_settings = ai_settings.clone();
            SettingItem::new(
                title,
                SettingField::render(move |_, _, cx| ai_settings.read(cx).render_picker(index)),
            )
            .description(description)
            .keywords(["model"])
        };
        let stand_in_name = self.stand_in_name.clone();
        SettingPage::new("AI")
            .icon(gpui_kit::assets::IconName::Sparkles)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(heading)
                    .items(empty)
                    .items(listed)
                    // Under the list, where the eye ends up, not beside the
                    // heading.
                    .item(add),
            )
            .group(
                SettingGroup::new()
                    .item(picker(
                        0,
                        "Default",
                        "Used for everything without a model of its own.",
                    ))
                    .item(picker(
                        1,
                        "Finding facts",
                        "Copies the facts out of your documents. A fast, cheap model does it well.",
                    ))
                    .item(picker(
                        2,
                        "Writing résumés",
                        "Picks and words the facts for a job.",
                    )),
            )
            .group(
                SettingGroup::new().item(
                    SettingItem::new(
                        "Your name is sent as",
                        SettingField::render(move |_, _, _| stand_in_name.clone()),
                    )
                    .description(
                        "A made-up name the AI sees in place of yours, so it reads your \
                         documents as written. Yours is put back in what comes back.",
                    )
                    .keywords(["stand-in", "redaction", "personal", "privacy"]),
                ),
            )
    }
}

/// Settings' pages, in the order they're listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsSection {
    Appearance,
    Ai,
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;

    #[test]
    fn choosing_a_palette_saves_it_and_closes_the_popover() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_sample(&mut app);
        let (_, cx) = app.add_window_view(|window, cx| {
            SettingsPage::new(database.clone(), SettingsSection::Appearance, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.click("choose-light-palette", cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(window.find("palette-clay").visible());
            window.click("palette-clay", cx);
        });
        testing::settle(cx, &database);
        let stored = database.read_with(cx, |database, _| {
            settings::Settings::read(database.connection())
        });
        assert_eq!(
            stored.map(|settings| settings.palettes.light).ok(),
            Some(Palette::Clay)
        );
        cx.update(|window, _| {
            assert!(
                window
                    .try_find("palette-clay")
                    .is_none_or(|element| !element.visible())
            )
        });
    }

    #[test]
    fn showing_a_section_opens_its_page() {
        let mut app = TestAppContext::single();
        let (database, _directory) = testing::open_sample(&mut app);
        let (page, cx) = app.add_window_view(|window, cx| {
            SettingsPage::new(database, SettingsSection::Appearance, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, _| assert!(window.find("choose-dark-palette").visible()));
        page.update(cx, |page, cx| page.show(SettingsSection::Ai, cx));
        cx.run_until_parked();
        cx.update(|window, _| {
            assert!(window.try_find("choose-dark-palette").is_none());
            assert!(window.find("add-provider").visible());
        });
    }
}

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
use crate::database::schema::{LlmAssignments, LlmProviders, Settings as SettingsTable};
use crate::database::{Changed, Database};
use crate::llm::{self, Purpose};
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
    _subscription: Subscription,
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
        let subscription = cx.subscribe(&database, |_, _, changed: &Changed, cx| {
            if changed.touches(SettingsTable)
                || changed.touches(LlmAssignments)
                || changed.touches(LlmProviders)
            {
                cx.notify();
            }
        });
        Self {
            database,
            ai_settings,
            stand_in_name,
            generation: 0,
            start,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
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

    /// The provider and model facts are found with, the provider's key, and
    /// the name sent in place of the user's.
    fn ai_page(&self, cx: &App) -> SettingPage {
        let database = self.database.clone();
        let assigned = {
            let database = database.clone();
            move |cx: &App| {
                llm::assigned(database.read(cx).device_connection(), Purpose::ExtractFacts)
                    .unwrap_or_else(|error| {
                        eprintln!("settings: couldn't read the AI provider: {error}");
                        None
                    })
            }
        };
        let vendor = assigned(cx).map_or(llm::VENDORS[0].0.to_owned(), |provider| provider.vendor);
        let ai_settings = self.ai_settings.clone();
        let stand_in_name = self.stand_in_name.clone();
        SettingPage::new("AI")
            .icon(gpui_kit::assets::IconName::Sparkles)
            .resettable(false)
            .group(
                SettingGroup::new()
                    .item(
                        SettingItem::new(
                            "Provider",
                            SettingField::dropdown(
                                llm::VENDORS
                                    .iter()
                                    .map(|(id, name)| ((*id).into(), (*name).into()))
                                    .collect(),
                                {
                                    let assigned = assigned.clone();
                                    move |cx| {
                                        assigned(cx)
                                            .map_or(llm::VENDORS[0].0.to_owned(), |provider| {
                                                provider.vendor
                                            })
                                            .into()
                                    }
                                },
                                {
                                    let assigned = assigned.clone();
                                    let database = database.clone();
                                    move |vendor, cx| {
                                        let model = assigned(cx)
                                            .filter(|provider| provider.vendor == vendor.as_ref())
                                            .map_or(llm::DEFAULT_MODEL.to_owned(), |provider| {
                                                provider.model
                                            });
                                        database.read(cx).queue(move |write| {
                                            llm::set_default(write, &vendor, &model).map(|_| ())
                                        });
                                    }
                                },
                            ),
                        )
                        .description(
                            "Your documents are sent here to find their facts, with your name \
                             and contact details hidden first.",
                        ),
                    )
                    .item(
                        SettingItem::new(
                            "Model",
                            SettingField::scrollable_dropdown(
                                llm::models(&vendor)
                                    .into_iter()
                                    .map(|(id, name)| (id.into(), name.into()))
                                    .collect(),
                                {
                                    let assigned = assigned.clone();
                                    move |cx| {
                                        assigned(cx)
                                            .map_or(llm::DEFAULT_MODEL.to_owned(), |provider| {
                                                provider.model
                                            })
                                            .into()
                                    }
                                },
                                move |model, cx| {
                                    let vendor = assigned(cx)
                                        .map_or(llm::VENDORS[0].0.to_owned(), |provider| {
                                            provider.vendor
                                        });
                                    database.read(cx).queue(move |write| {
                                        llm::set_default(write, &vendor, &model).map(|_| ())
                                    });
                                },
                            ),
                        )
                        .description("Used to find facts and write your résumés."),
                    )
                    .item(
                        SettingItem::new(
                            "API key",
                            SettingField::render(move |_, _, _| ai_settings.clone()),
                        )
                        .description("Kept in your system keyring, never in Kron's data.")
                        .keywords(["credentials", "password", "keyring"]),
                    )
                    .item(
                        SettingItem::new(
                            "Your name is sent as",
                            SettingField::render(move |_, _, _| stand_in_name.clone()),
                        )
                        .description(
                            "A made-up name the AI sees in place of yours, so it reads your \
                             documents as written. Yours is put back in what comes back.",
                        )
                        .keywords([
                            "stand-in",
                            "redaction",
                            "personal",
                            "privacy",
                        ]),
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
            assert!(window.find("add-api-key").visible());
        });
    }
}

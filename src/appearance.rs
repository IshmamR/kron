use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Global, SharedString, Window};

/// gpui's Linux default (IBM Plex Sans) is rarely installed, and the fallback
/// it lands on often has no bold. Inter is common and has every weight.
const UI_FONT: &str = "Inter";
const LIGHT_LIST_ACTIVE_OPACITY: f32 = 0.3;

pub fn init(cx: &mut App) {
    cx.set_global(Appearance::default());
    // Theme files only replace the font when they name one, so this survives
    // switching between light and dark.
    Theme::update(cx, |theme| theme.font_family = UI_FONT.into());
}

/// Light or dark. Follows the desktop unless the user picks one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Global for Appearance {}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn current(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    /// Remembers the choice. The workspace observes it and calls `apply`,
    /// since applying needs the window.
    pub fn set(self, cx: &mut App) {
        cx.set_global(self);
    }

    /// The window is used to read the desktop's appearance, which is more
    /// reliable on Linux than the app-wide one.
    pub fn apply(self, window: &mut Window, cx: &mut App) {
        match self {
            Self::System => Theme::sync_system_appearance(Some(window), cx),
            Self::Light => Theme::change(ThemeMode::Light, Some(window), cx),
            Self::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
        }
        // The light theme's selected list row is too faint to find at a
        // glance; tint it with the selection border's blue instead.
        Theme::update(cx, |theme| {
            if !theme.is_dark() {
                theme.list_active = theme.list_active_border.opacity(LIGHT_LIST_ACTIVE_OPACITY);
            }
        });
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Match system",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// Stable name, used as the settings dropdown's value.
    pub fn key(self) -> SharedString {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
        .into()
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|appearance| appearance.key() == key)
    }
}

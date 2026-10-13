use std::process::Command;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, AppContext as _, Global, SharedString, Window};

/// gpui's Linux default (IBM Plex Sans) is rarely installed, and the fallback
/// it lands on often has no bold. Inter is common and has every weight.
const UI_FONT: &str = "Inter";
const LIGHT_LIST_ACTIVE_OPACITY: f32 = 0.3;

pub fn init(cx: &mut App) {
    cx.set_global(Appearance::default());
    // Theme files only replace the font when they name one, so this survives
    // switching between light and dark.
    Theme::update(cx, |theme| theme.font_family = UI_FONT.into());
    Motion::default().set(cx);
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

/// Whether things slide and fade, like the sidebar collapsing. Reduced, they
/// jump straight to where they end. Follows the desktop's accessibility
/// setting unless the user picks one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Motion {
    #[default]
    System,
    Reduced,
    Full,
}

impl Global for Motion {}

impl Motion {
    pub const ALL: [Self; 3] = [Self::System, Self::Reduced, Self::Full];

    pub fn current(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    /// Remembers the choice and applies it to every animation, through
    /// gpui's own switch, which its animations already respect.
    pub fn set(self, cx: &mut App) {
        cx.set_global(self);
        match self {
            Self::Reduced => cx.set_reduce_motion(true),
            Self::Full => cx.set_reduce_motion(false),
            // Asking the desktop runs a command, so it's done off the main
            // thread; motion stays as it was until the answer comes.
            Self::System => cx
                .spawn(async move |cx| {
                    let reduce = cx
                        .background_spawn(async { system_reduces_motion() })
                        .await
                        .unwrap_or(false);
                    cx.update(|cx| {
                        // The choice may have changed while the desktop was
                        // asked.
                        if Self::current(cx) == Self::System {
                            cx.set_reduce_motion(reduce);
                        }
                    });
                })
                .detach(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Match system",
            Self::Reduced => "Reduced",
            Self::Full => "Full",
        }
    }

    /// Stable name, used as the settings dropdown's value.
    pub fn key(self) -> SharedString {
        match self {
            Self::System => "system",
            Self::Reduced => "reduced",
            Self::Full => "full",
        }
        .into()
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|motion| motion.key() == key)
    }
}

/// The desktop's reduce-motion setting, or `None` when it can't be read.
/// gpui doesn't ask the platform, so this does: GNOME's "Reduce animation"
/// and KDE's animation speed set to instant on Linux, and "Reduce motion" on
/// macOS.
fn system_reduces_motion() -> Option<bool> {
    if cfg!(target_os = "macos") {
        let output = command_output("defaults", &["read", "com.apple.universalaccess", "reduceMotion"])?;
        return Some(output.trim() == "1");
    }
    if cfg!(target_os = "linux") {
        if let Some(output) =
            command_output("gsettings", &["get", "org.gnome.desktop.interface", "enable-animations"])
        {
            return Some(output.trim() == "false");
        }
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".config")))?;
        let kdeglobals = std::fs::read_to_string(config.join("kdeglobals")).ok()?;
        return kdeglobals
            .lines()
            .find_map(|line| line.trim().strip_prefix("AnimationDurationFactor="))
            .and_then(|factor| factor.trim().parse::<f32>().ok())
            .map(|factor| factor == 0.);
    }
    // Windows keeps it behind a system call this doesn't make yet.
    None
}

/// What the command printed, if it ran and succeeded. A missing command just
/// means the desktop isn't the one it belongs to.
fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

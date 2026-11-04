mod palette;

pub use palette::{Palette, Palettes};

use std::process::Command;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{Anchor, App, AppContext as _, Global, Hsla, Window, rgb};

/// gpui's Linux default (IBM Plex Sans) is rarely installed, and the fallback
/// it lands on often has no bold. Inter is common and has every weight.
#[cfg(target_os = "linux")]
const UI_FONT: &str = "Inter";

pub fn init(cx: &mut App) {
    cx.set_global(Appearance::default());
    cx.set_global(Palettes::default());
    #[cfg(target_os = "linux")]
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
        let palettes = Palettes::current(cx);
        Theme::update(cx, |theme| {
            let palette = palettes.for_mode(theme.is_dark());
            apply_gallery_palette(theme, palette);
        });
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Match system",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

fn apply_gallery_palette(theme: &mut Theme, palette: Palette) {
    theme.notification.placement = Anchor::BottomRight;
    let palette = palette.colors();
    let color = |value| Hsla::from(rgb(value));
    let background = color(palette.background);
    let sidebar = color(palette.sidebar);
    let surface = color(palette.surface);
    let muted = color(palette.muted_surface);
    let hover = color(palette.hover);
    let border = color(palette.border);
    let ink = color(palette.ink);
    let secondary = color(palette.secondary);
    let accent = color(palette.accent);
    let selection = color(palette.selection);
    let accent_text = color(palette.accent_text);

    theme.background = background;
    theme.foreground = ink;
    theme.secondary = muted;
    theme.secondary_foreground = ink;
    theme.secondary_hover = hover;
    theme.secondary_active = selection;
    theme.button_secondary = muted;
    theme.button_secondary_foreground = ink;
    theme.button_secondary_hover = hover;
    theme.button_secondary_active = selection;
    theme.muted = muted;
    theme.muted_foreground = secondary;
    theme.border = border;
    theme.input = border;
    theme.ring = accent;
    theme.caret = accent;
    theme.selection = selection;
    theme.link = accent;
    theme.info = accent;
    theme.danger = color(palette.danger);
    theme.warning = color(palette.warning);
    theme.accent = selection;
    theme.accent_foreground = ink;
    theme.popover = surface;
    theme.popover_foreground = ink;
    theme.group_box = surface;
    theme.group_box_foreground = ink;

    theme.title_bar = sidebar;
    theme.title_bar_border = border;
    theme.sidebar = sidebar;
    theme.sidebar_border = border;
    theme.sidebar_foreground = ink;
    theme.sidebar_accent = selection;
    theme.sidebar_accent_foreground = ink;
    theme.colors.list = background;
    theme.list_hover = hover;
    theme.list_active = selection;
    theme.list_active_border = accent;

    theme.tab_bar = surface;
    theme.tab = background;
    theme.tab_bar_segmented = muted;
    theme.tab_active = surface;
    theme.tab_active_foreground = ink;
    theme.tab_foreground = secondary;
    theme.button = surface;
    theme.button_foreground = ink;
    theme.button_hover = muted;
    theme.button_active = hover;
    theme.primary = accent;
    theme.primary_foreground = accent_text;
    theme.primary_hover = accent.opacity(0.9);
    theme.primary_active = accent.opacity(0.8);
    theme.button_primary = theme.primary;
    theme.button_primary_foreground = theme.primary_foreground;
    theme.button_primary_hover = theme.primary_hover;
    theme.button_primary_active = theme.primary_active;
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

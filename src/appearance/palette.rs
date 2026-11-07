use gpui_kit::{App, Global};

/// A curated color family. Appearance mode is chosen separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    Moss,
    Midnight,
    Copper,
    Orchid,
    Olive,
    Graphite,
    Porcelain,
    Linen,
    Glacier,
    Clay,
    Paper,
    Lilac,
}

impl Palette {
    pub const ALL: [Self; 12] = [
        Self::Moss,
        Self::Midnight,
        Self::Copper,
        Self::Orchid,
        Self::Olive,
        Self::Graphite,
        Self::Porcelain,
        Self::Linen,
        Self::Glacier,
        Self::Clay,
        Self::Paper,
        Self::Lilac,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Moss => "Moss",
            Self::Midnight => "Midnight",
            Self::Copper => "Copper",
            Self::Orchid => "Orchid",
            Self::Olive => "Olive",
            Self::Porcelain => "Porcelain",
            Self::Linen => "Linen & Sage",
            Self::Glacier => "Glacier",
            Self::Graphite => "Graphite",
            Self::Clay => "Clay",
            Self::Paper => "Paper",
            Self::Lilac => "Lilac",
        }
    }

    pub fn is_dark(self) -> bool {
        matches!(
            self,
            Self::Moss
                | Self::Midnight
                | Self::Copper
                | Self::Orchid
                | Self::Olive
                | Self::Graphite
        )
    }

    pub fn colors(self) -> PaletteColors {
        match self {
            Self::Moss => PaletteColors {
                background: 0x141719,
                sidebar: 0x1a1d1e,
                surface: 0x1d2122,
                muted_surface: 0x252a2b,
                hover: 0x2b3131,
                border: 0x303636,
                ink: 0xeef0eb,
                secondary: 0xa1aaa8,
                accent: 0x93c4ac,
                selection: 0x234337,
                accent_text: 0x17261f,
                danger: 0xec9b94,
                warning: 0xd8b782,
            },
            Self::Midnight => PaletteColors {
                background: 0x101722,
                sidebar: 0x17202c,
                surface: 0x1b2533,
                muted_surface: 0x243142,
                hover: 0x2b394b,
                border: 0x344254,
                ink: 0xf0f3f9,
                secondary: 0xadbacb,
                accent: 0x93b4f6,
                selection: 0x293e63,
                accent_text: 0x13223c,
                danger: 0xe9a0a7,
                warning: 0xe9ba80,
            },
            Self::Copper => PaletteColors {
                background: 0x1a1919,
                sidebar: 0x22201f,
                surface: 0x262321,
                muted_surface: 0x302b28,
                hover: 0x3b3430,
                border: 0x4b4039,
                ink: 0xf3ede8,
                secondary: 0xbeafa5,
                accent: 0xdfa47f,
                selection: 0x4e3228,
                accent_text: 0x382114,
                danger: 0xe7a19b,
                warning: 0xe9c28a,
            },
            Self::Orchid => PaletteColors {
                background: 0x17141d,
                sidebar: 0x201a28,
                surface: 0x261e2c,
                muted_surface: 0x31273a,
                hover: 0x3d3047,
                border: 0x493951,
                ink: 0xf5f0f7,
                secondary: 0xbeafc7,
                accent: 0xd1a9ec,
                selection: 0x432d53,
                accent_text: 0x34223f,
                danger: 0xf0a0a6,
                warning: 0xe1ba86,
            },
            Self::Olive => PaletteColors {
                background: 0x171a14,
                sidebar: 0x20251b,
                surface: 0x262c20,
                muted_surface: 0x31382a,
                hover: 0x3d4733,
                border: 0x414c3a,
                ink: 0xf3f5eb,
                secondary: 0xb7c1aa,
                accent: 0xc2d794,
                selection: 0x364a2a,
                accent_text: 0x2c381e,
                danger: 0xe5a29c,
                warning: 0xe7c383,
            },
            Self::Porcelain => PaletteColors {
                background: 0xf6f5f2,
                sidebar: 0xeae9e5,
                surface: 0xfffefc,
                muted_surface: 0xf0efeb,
                hover: 0xe3e2dc,
                border: 0xd8d7d1,
                ink: 0x22272a,
                secondary: 0x656c70,
                accent: 0x476878,
                selection: 0xdbe7eb,
                accent_text: 0xffffff,
                danger: 0xa8524d,
                warning: 0x9e752d,
            },
            Self::Linen => PaletteColors {
                background: 0xf5f4ed,
                sidebar: 0xecece2,
                surface: 0xfffef8,
                muted_surface: 0xf0f1e7,
                hover: 0xe3e9dd,
                border: 0xd7dccf,
                ink: 0x27322b,
                secondary: 0x5d6b60,
                accent: 0x497858,
                selection: 0xdbebde,
                accent_text: 0xffffff,
                danger: 0xa24e47,
                warning: 0x946924,
            },
            Self::Glacier => PaletteColors {
                background: 0xf2f6fa,
                sidebar: 0xe7edf5,
                surface: 0xffffff,
                muted_surface: 0xebf1f8,
                hover: 0xdde7f1,
                border: 0xd4dee9,
                ink: 0x192b3b,
                secondary: 0x5b7086,
                accent: 0x376dbb,
                selection: 0xdbeafd,
                accent_text: 0xffffff,
                danger: 0xb54e62,
                warning: 0xa37534,
            },
            // Grays with no hue; the accent is off-white rather than a color.
            Self::Graphite => PaletteColors {
                background: 0x1b1b1d,
                sidebar: 0x202022,
                surface: 0x242426,
                muted_surface: 0x2b2b2e,
                hover: 0x323235,
                border: 0x3a3a3d,
                ink: 0xe4e4e2,
                secondary: 0xa0a0a3,
                accent: 0xd8d8d5,
                selection: 0x38383b,
                accent_text: 0x1b1b1d,
                danger: 0xe8a09a,
                warning: 0xdcbc84,
            },
            Self::Clay => PaletteColors {
                background: 0xf3f0e8,
                sidebar: 0xebe7dc,
                surface: 0xfaf8f3,
                muted_surface: 0xeee9de,
                hover: 0xe5dfd1,
                border: 0xddd5c5,
                ink: 0x2f2a24,
                secondary: 0x6b6256,
                accent: 0xb35733,
                selection: 0xf1dccf,
                accent_text: 0xffffff,
                danger: 0xa83a4a,
                warning: 0x8d6a1f,
            },
            // Graphite turned over: an off-white page with a near-black accent.
            Self::Paper => PaletteColors {
                background: 0xefefec,
                sidebar: 0xe7e7e4,
                surface: 0xf7f7f5,
                muted_surface: 0xe9e9e6,
                hover: 0xdfdfdb,
                border: 0xd2d2ce,
                ink: 0x232325,
                secondary: 0x626266,
                accent: 0x2b2b2d,
                selection: 0xdcdcd8,
                accent_text: 0xf2f2ef,
                danger: 0xa8463f,
                warning: 0x8f6a22,
            },
            Self::Lilac => PaletteColors {
                background: 0xf4f2f7,
                sidebar: 0xebe8f1,
                surface: 0xfbfafd,
                muted_surface: 0xeeebf3,
                hover: 0xe3dfeb,
                border: 0xd9d4e3,
                ink: 0x2a2534,
                secondary: 0x675f74,
                accent: 0x7350a6,
                selection: 0xe6dcf5,
                accent_text: 0xffffff,
                danger: 0xb04a5a,
                warning: 0x93702a,
            },
        }
    }
}

/// The palettes used when the current appearance is light or dark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palettes {
    pub light: Palette,
    pub dark: Palette,
}

impl Default for Palettes {
    fn default() -> Self {
        Self {
            light: Palette::Linen,
            dark: Palette::Orchid,
        }
    }
}

impl Global for Palettes {}

impl Palettes {
    pub fn current(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    pub fn for_mode(self, dark: bool) -> Palette {
        if dark { self.dark } else { self.light }
    }
}

pub struct PaletteColors {
    pub background: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub muted_surface: u32,
    pub hover: u32,
    pub border: u32,
    pub ink: u32,
    pub secondary: u32,
    pub accent: u32,
    pub selection: u32,
    pub accent_text: u32,
    pub danger: u32,
    pub warning: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance of a 0xRRGGBB color.
    fn luminance(color: u32) -> f32 {
        let channel = |shift: u32| {
            let value = ((color >> shift) & 0xff) as f32 / 255.;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    fn contrast(first: u32, second: u32) -> f32 {
        let (first, second) = (luminance(first), luminance(second));
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }

    /// The picker's swatch is the background beside the accent: both halves
    /// must be told apart, which WCAG puts at 3:1 for non-text marks.
    #[test]
    fn every_accent_stands_out_from_its_background() {
        for palette in Palette::ALL {
            let colors = palette.colors();
            let ratio = contrast(colors.accent, colors.background);
            assert!(ratio >= 3., "{}: accent at {ratio:.2}:1", palette.label());
        }
    }

    /// WCAG AA's 4.5:1 for small text: captions, error text and primary
    /// buttons' labels are all small.
    #[test]
    fn every_text_color_is_readable_on_what_it_sits_on() {
        for palette in Palette::ALL {
            let colors = palette.colors();
            for (name, text, behind) in [
                ("ink", colors.ink, colors.background),
                ("ink on a selection", colors.ink, colors.selection),
                ("secondary", colors.secondary, colors.background),
                ("danger", colors.danger, colors.background),
                (
                    "a primary button's label",
                    colors.accent_text,
                    colors.accent,
                ),
            ] {
                let ratio = contrast(text, behind);
                assert!(ratio >= 4.5, "{}: {name} at {ratio:.2}:1", palette.label());
            }
        }
    }
}

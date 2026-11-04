use gpui_kit::{App, Global};

/// A curated color family. Appearance mode is chosen separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    Moss,
    Midnight,
    Copper,
    Orchid,
    Olive,
    Porcelain,
    Linen,
    Glacier,
}

impl Palette {
    pub const ALL: [Self; 8] = [
        Self::Moss,
        Self::Midnight,
        Self::Copper,
        Self::Orchid,
        Self::Olive,
        Self::Porcelain,
        Self::Linen,
        Self::Glacier,
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
        }
    }

    pub fn is_dark(self) -> bool {
        matches!(
            self,
            Self::Moss | Self::Midnight | Self::Copper | Self::Orchid | Self::Olive
        )
    }

    pub fn preview(self) -> [u32; 4] {
        let colors = self.colors();
        [colors.background, colors.sidebar, colors.accent, colors.ink]
    }

    pub(super) fn colors(self) -> PaletteColors {
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
                secondary: 0x6b7276,
                accent: 0x476878,
                selection: 0xdbe7eb,
                accent_text: 0xffffff,
                danger: 0xb85d58,
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
                secondary: 0x667c91,
                accent: 0x376dbb,
                selection: 0xdbeafd,
                accent_text: 0xffffff,
                danger: 0xb54e62,
                warning: 0xa37534,
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

pub(super) struct PaletteColors {
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

//! Colour themes for the Audian window and the recording indicator.
//!
//! Plain RGB data, so every component can use it: the egui window, the tiny-skia overlay
//! drawn by the tray app, and the installer. Surfaces (backgrounds, cards, text) are the
//! same dark set for every theme; a theme changes the accent family and the logo.

use serde::{Deserialize, Serialize};

pub type Rgb = (u8, u8, u8);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeId {
    #[default]
    Ivory,
    Violet,
    Ocean,
    Mint,
    Sunset,
    Rose,
}

impl ThemeId {
    pub const ALL: [ThemeId; 6] = [Self::Ivory, Self::Violet, Self::Ocean, Self::Mint, Self::Sunset, Self::Rose];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ivory => "Ivory",
            Self::Violet => "Violet",
            Self::Ocean => "Ocean",
            Self::Mint => "Mint",
            Self::Sunset => "Sunset",
            Self::Rose => "Rose",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Ivory => "Off-white on graphite. Calm and minimal.",
            Self::Violet => "The original purple and blue glow.",
            Self::Ocean => "Clear blue with a hint of teal.",
            Self::Mint => "Fresh green, easy on the eyes.",
            Self::Sunset => "Warm orange fading into coral.",
            Self::Rose => "Soft pink with a lilac touch.",
        }
    }

    pub fn palette(self) -> &'static Palette {
        &PALETTES[self as usize]
    }

    pub fn from_index(i: u8) -> ThemeId {
        Self::ALL.get(i as usize).copied().unwrap_or_default()
    }
}

/// Light or dark window.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    #[default]
    Dark,
    Light,
    /// Follow Windows' app mode (Settings › Personalization › Colors).
    System,
}

/// Background of the recording indicator ("pill").
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorStyle {
    /// Dark glass with a light waveform.
    #[default]
    Dark,
    /// Off-white with a dark, theme-coloured waveform.
    Light,
}

impl IndicatorStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }
}

pub struct Palette {
    /// Main accent on dark surfaces: buttons, switches, selection, icons.
    pub accent: Rgb,
    /// Accent under the pointer.
    pub accent_hover: Rgb,
    /// Text and icons drawn on an accent fill.
    pub on_accent: Rgb,
    /// Second accent, for gradients and secondary highlights.
    pub accent_2: Rgb,
    /// Waveform on the dark indicator.
    pub wave: Rgb,
    /// Accent pair for light surfaces (the light indicator).
    pub ink: Rgb,
    pub ink_2: Rgb,
    /// Logo: background gradient (top, bottom), glow, and the mark's gradient.
    pub logo_bg: (Rgb, Rgb),
    pub logo_glow: Rgb,
    pub logo_mark: [Rgb; 3],
}

pub const PALETTES: [Palette; 6] = [
    // Ivory
    Palette {
        accent: (236, 231, 221),
        accent_hover: (250, 247, 240),
        on_accent: (24, 24, 28),
        accent_2: (196, 189, 176),
        wave: (246, 242, 234),
        ink: (38, 38, 44),
        ink_2: (120, 114, 104),
        logo_bg: ((42, 42, 46), (14, 14, 16)),
        logo_glow: (255, 244, 225),
        logo_mark: [(252, 250, 245), (236, 230, 218), (204, 196, 182)],
    },
    // Violet
    Palette {
        accent: (139, 123, 255),
        accent_hover: (160, 147, 255),
        on_accent: (255, 255, 255),
        accent_2: (91, 157, 255),
        wave: (255, 255, 255),
        ink: (104, 84, 232),
        ink_2: (60, 120, 230),
        logo_bg: ((30, 27, 58), (13, 12, 26)),
        logo_glow: (124, 92, 255),
        logo_mark: [(216, 205, 255), (167, 160, 255), (125, 185, 255)],
    },
    // Ocean
    Palette {
        accent: (72, 160, 255),
        accent_hover: (110, 182, 255),
        on_accent: (255, 255, 255),
        accent_2: (45, 212, 191),
        wave: (214, 234, 255),
        ink: (22, 112, 220),
        ink_2: (14, 150, 140),
        logo_bg: ((16, 32, 58), (8, 14, 28)),
        logo_glow: (56, 140, 255),
        logo_mark: [(206, 230, 255), (110, 182, 255), (80, 220, 200)],
    },
    // Mint
    Palette {
        accent: (60, 214, 158),
        accent_hover: (100, 228, 182),
        on_accent: (6, 34, 24),
        accent_2: (56, 189, 248),
        wave: (214, 248, 234),
        ink: (12, 150, 104),
        ink_2: (14, 120, 170),
        logo_bg: ((14, 40, 34), (6, 18, 16)),
        logo_glow: (40, 200, 140),
        logo_mark: [(206, 250, 230), (100, 228, 182), (90, 200, 240)],
    },
    // Sunset
    Palette {
        accent: (255, 157, 77),
        accent_hover: (255, 182, 120),
        on_accent: (40, 18, 4),
        accent_2: (255, 99, 132),
        wave: (255, 232, 212),
        ink: (214, 104, 24),
        ink_2: (214, 60, 96),
        logo_bg: ((52, 26, 18), (22, 10, 8)),
        logo_glow: (255, 130, 60),
        logo_mark: [(255, 228, 200), (255, 170, 110), (255, 120, 140)],
    },
    // Rose
    Palette {
        accent: (244, 124, 190),
        accent_hover: (249, 156, 208),
        on_accent: (44, 8, 28),
        accent_2: (167, 139, 250),
        wave: (255, 226, 242),
        ink: (200, 60, 140),
        ink_2: (124, 92, 230),
        logo_bg: ((50, 18, 40), (20, 8, 18)),
        logo_glow: (240, 90, 170),
        logo_mark: [(255, 216, 238), (244, 140, 200), (180, 150, 255)],
    },
];

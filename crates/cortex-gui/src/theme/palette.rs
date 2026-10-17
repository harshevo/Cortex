//! Every colour the app uses, for each of its themes. One theme is in
//! use at a time; the functions here give its colours.

use std::sync::atomic::{AtomicUsize, Ordering};

use eframe::egui::Color32;

/// The colours of one theme.
pub struct Theme {
    pub name: &'static str,

    /// Light text on dark grounds, rather than the other way round.
    pub dark: bool,

    pub background: Color32,
    pub sidebar: Color32,
    pub sunken: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub border: Color32,

    pub text: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,

    /// Code set inside a sentence.
    pub code: Color32,

    /// What the agent does (send and other primary actions).
    pub accent: Color32,

    pub good: Color32,
    pub bad: Color32,
    pub warning: Color32,

    /// A model response in the loop strip.
    pub model: Color32,
}

const fn rgb(red: u8, green: u8, blue: u8) -> Color32 {
    Color32::from_rgb(red, green, blue)
}

/// The theme the app starts with, and the one a name that is not known
/// falls back to.
pub const DEFAULT: &str = "Cortex Dark";

/// The first is the app's own; the rest follow well-known editor themes,
/// the dark ones before the light.
pub const THEMES: [Theme; 19] = [
    // The app's own: neutral greys only, one accent, used sparingly.
    Theme {
        name: "Cortex Dark",
        dark: true,
        background: rgb(0x0a, 0x0a, 0x0a),
        sidebar: rgb(0x0f, 0x0f, 0x0f),
        sunken: rgb(0x05, 0x05, 0x05),
        surface: rgb(0x16, 0x16, 0x16),
        raised: rgb(0x22, 0x22, 0x22),
        border: rgb(0x26, 0x26, 0x26),
        text: rgb(0xed, 0xed, 0xed),
        text_secondary: rgb(0xa3, 0xa3, 0xa3),
        text_muted: rgb(0x7a, 0x7a, 0x7a),
        code: rgb(0xd4, 0xd4, 0xd4),
        accent: rgb(0xd9, 0x77, 0x57),
        good: rgb(0x5d, 0xca, 0x9a),
        bad: rgb(0xe5, 0x67, 0x5f),
        warning: rgb(0xe2, 0xab, 0x52),
        model: rgb(0x8f, 0x87, 0xe8),
    },
    Theme {
        name: "Catppuccin Mocha",
        dark: true,
        background: rgb(0x1e, 0x1e, 0x2e),
        sidebar: rgb(0x18, 0x18, 0x25),
        sunken: rgb(0x11, 0x11, 0x1b),
        surface: rgb(0x31, 0x32, 0x44),
        raised: rgb(0x45, 0x47, 0x5a),
        border: rgb(0x58, 0x5b, 0x70),
        text: rgb(0xcd, 0xd6, 0xf4),
        text_secondary: rgb(0xa6, 0xad, 0xc8),
        text_muted: rgb(0x8d, 0x92, 0xab),
        code: rgb(0xfa, 0xb3, 0x87),
        accent: rgb(0xcb, 0xa6, 0xf7),
        good: rgb(0xa6, 0xe3, 0xa1),
        bad: rgb(0xf3, 0x8b, 0xa8),
        warning: rgb(0xf9, 0xe2, 0xaf),
        model: rgb(0xb4, 0xbe, 0xfe),
    },
    Theme {
        name: "Tokyo Night",
        dark: true,
        background: rgb(0x1a, 0x1b, 0x26),
        sidebar: rgb(0x16, 0x16, 0x1e),
        sunken: rgb(0x13, 0x13, 0x1a),
        surface: rgb(0x24, 0x28, 0x3b),
        raised: rgb(0x2f, 0x35, 0x49),
        border: rgb(0x3b, 0x42, 0x61),
        text: rgb(0xc0, 0xca, 0xf5),
        text_secondary: rgb(0x9a, 0xa5, 0xce),
        text_muted: rgb(0x73, 0x7a, 0xa2),
        code: rgb(0xff, 0x9e, 0x64),
        accent: rgb(0x7a, 0xa2, 0xf7),
        good: rgb(0x9e, 0xce, 0x6a),
        bad: rgb(0xf7, 0x76, 0x8e),
        warning: rgb(0xe0, 0xaf, 0x68),
        model: rgb(0xbb, 0x9a, 0xf7),
    },
    Theme {
        name: "Dracula",
        dark: true,
        background: rgb(0x28, 0x2a, 0x36),
        sidebar: rgb(0x21, 0x22, 0x2c),
        sunken: rgb(0x19, 0x1a, 0x21),
        surface: rgb(0x34, 0x37, 0x46),
        raised: rgb(0x44, 0x47, 0x5a),
        border: rgb(0x56, 0x5a, 0x70),
        text: rgb(0xf8, 0xf8, 0xf2),
        text_secondary: rgb(0xbf, 0xc2, 0xd4),
        text_muted: rgb(0x8d, 0x95, 0xb8),
        code: rgb(0xff, 0xb8, 0x6c),
        accent: rgb(0xbd, 0x93, 0xf9),
        good: rgb(0x50, 0xfa, 0x7b),
        bad: rgb(0xff, 0x55, 0x55),
        warning: rgb(0xf1, 0xfa, 0x8c),
        model: rgb(0x8b, 0xe9, 0xfd),
    },
    Theme {
        name: "Nord",
        dark: true,
        background: rgb(0x2e, 0x34, 0x40),
        sidebar: rgb(0x27, 0x2c, 0x36),
        sunken: rgb(0x24, 0x29, 0x33),
        surface: rgb(0x3b, 0x42, 0x52),
        raised: rgb(0x43, 0x4c, 0x5e),
        border: rgb(0x4c, 0x56, 0x6a),
        text: rgb(0xec, 0xef, 0xf4),
        text_secondary: rgb(0xc5, 0xcd, 0xdb),
        text_muted: rgb(0x9a, 0xa5, 0xbb),
        code: rgb(0xd0, 0x87, 0x70),
        accent: rgb(0x88, 0xc0, 0xd0),
        good: rgb(0xa3, 0xbe, 0x8c),
        bad: rgb(0xbf, 0x61, 0x6a),
        warning: rgb(0xeb, 0xcb, 0x8b),
        model: rgb(0xb4, 0x8e, 0xad),
    },
    Theme {
        name: "Gruvbox Dark",
        dark: true,
        background: rgb(0x28, 0x28, 0x28),
        sidebar: rgb(0x1d, 0x20, 0x21),
        sunken: rgb(0x1b, 0x1b, 0x1b),
        surface: rgb(0x3c, 0x38, 0x36),
        raised: rgb(0x50, 0x49, 0x45),
        border: rgb(0x66, 0x5c, 0x54),
        text: rgb(0xeb, 0xdb, 0xb2),
        text_secondary: rgb(0xc9, 0xb9, 0x9a),
        text_muted: rgb(0xa8, 0x99, 0x84),
        code: rgb(0xfa, 0xbd, 0x2f),
        accent: rgb(0xfe, 0x80, 0x19),
        good: rgb(0xb8, 0xbb, 0x26),
        bad: rgb(0xfb, 0x49, 0x34),
        warning: rgb(0xfa, 0xbd, 0x2f),
        model: rgb(0xd3, 0x86, 0x9b),
    },
    Theme {
        name: "One Dark",
        dark: true,
        background: rgb(0x28, 0x2c, 0x34),
        sidebar: rgb(0x21, 0x25, 0x2b),
        sunken: rgb(0x1b, 0x1d, 0x23),
        surface: rgb(0x2f, 0x34, 0x3d),
        raised: rgb(0x3a, 0x3f, 0x4b),
        border: rgb(0x4b, 0x52, 0x63),
        text: rgb(0xd7, 0xda, 0xe0),
        text_secondary: rgb(0xa7, 0xaf, 0xbd),
        text_muted: rgb(0x85, 0x8b, 0x97),
        code: rgb(0xd1, 0x9a, 0x66),
        accent: rgb(0x61, 0xaf, 0xef),
        good: rgb(0x98, 0xc3, 0x79),
        bad: rgb(0xe0, 0x6c, 0x75),
        warning: rgb(0xe5, 0xc0, 0x7b),
        model: rgb(0xc6, 0x78, 0xdd),
    },
    Theme {
        name: "Rosé Pine",
        dark: true,
        background: rgb(0x19, 0x17, 0x24),
        sidebar: rgb(0x1f, 0x1d, 0x2e),
        sunken: rgb(0x16, 0x14, 0x1f),
        surface: rgb(0x26, 0x23, 0x3a),
        raised: rgb(0x32, 0x2f, 0x47),
        border: rgb(0x40, 0x3d, 0x52),
        text: rgb(0xe0, 0xde, 0xf4),
        text_secondary: rgb(0xa3, 0x9f, 0xbb),
        text_muted: rgb(0x81, 0x7c, 0x9c),
        code: rgb(0xf6, 0xc1, 0x77),
        accent: rgb(0xc4, 0xa7, 0xe7),
        good: rgb(0x9c, 0xcf, 0xd8),
        bad: rgb(0xeb, 0x6f, 0x92),
        warning: rgb(0xf6, 0xc1, 0x77),
        model: rgb(0xeb, 0xbc, 0xba),
    },
    Theme {
        name: "Solarized Dark",
        dark: true,
        background: rgb(0x00, 0x2b, 0x36),
        sidebar: rgb(0x00, 0x21, 0x2b),
        sunken: rgb(0x00, 0x1e, 0x26),
        surface: rgb(0x07, 0x36, 0x42),
        raised: rgb(0x0f, 0x46, 0x54),
        border: rgb(0x1a, 0x53, 0x61),
        text: rgb(0xee, 0xe8, 0xd5),
        text_secondary: rgb(0x9f, 0xad, 0xad),
        text_muted: rgb(0x7b, 0x91, 0x96),
        code: rgb(0xcb, 0x4b, 0x16),
        accent: rgb(0x26, 0x8b, 0xd2),
        good: rgb(0x85, 0x99, 0x00),
        bad: rgb(0xdc, 0x32, 0x2f),
        warning: rgb(0xb5, 0x89, 0x00),
        model: rgb(0x6c, 0x71, 0xc4),
    },
    Theme {
        name: "GitHub Dark",
        dark: true,
        background: rgb(0x0d, 0x11, 0x17),
        sidebar: rgb(0x01, 0x04, 0x09),
        sunken: rgb(0x01, 0x04, 0x09),
        surface: rgb(0x16, 0x1b, 0x22),
        raised: rgb(0x21, 0x26, 0x2d),
        border: rgb(0x30, 0x36, 0x3d),
        text: rgb(0xe6, 0xed, 0xf3),
        text_secondary: rgb(0x91, 0x98, 0xa1),
        text_muted: rgb(0x7d, 0x85, 0x90),
        code: rgb(0xff, 0xa6, 0x57),
        accent: rgb(0x2f, 0x81, 0xf7),
        good: rgb(0x3f, 0xb9, 0x50),
        bad: rgb(0xf8, 0x51, 0x49),
        warning: rgb(0xd2, 0x99, 0x22),
        model: rgb(0xa3, 0x71, 0xf7),
    },
    Theme {
        name: "Cortex Light",
        dark: false,
        background: rgb(0xff, 0xff, 0xff),
        sidebar: rgb(0xfa, 0xfa, 0xfa),
        sunken: rgb(0xf5, 0xf5, 0xf5),
        surface: rgb(0xf5, 0xf5, 0xf5),
        raised: rgb(0xeb, 0xeb, 0xeb),
        border: rgb(0xe5, 0xe5, 0xe5),
        text: rgb(0x0a, 0x0a, 0x0a),
        text_secondary: rgb(0x52, 0x52, 0x52),
        text_muted: rgb(0x73, 0x73, 0x73),
        code: rgb(0x40, 0x40, 0x40),
        accent: rgb(0xc9, 0x64, 0x42),
        good: rgb(0x15, 0x80, 0x3d),
        bad: rgb(0xdc, 0x26, 0x26),
        warning: rgb(0xb4, 0x53, 0x09),
        model: rgb(0x6d, 0x28, 0xd9),
    },
    Theme {
        name: "Pierre Light",
        dark: false,
        background: rgb(0xff, 0xff, 0xff),
        sidebar: rgb(0xf7, 0xf7, 0xf7),
        sunken: rgb(0xf2, 0xf2, 0xf3),
        surface: rgb(0xf0, 0xf0, 0xf1),
        raised: rgb(0xe4, 0xe4, 0xe7),
        border: rgb(0xdc, 0xdc, 0xe0),
        text: rgb(0x07, 0x07, 0x07),
        text_secondary: rgb(0x55, 0x55, 0x5c),
        text_muted: rgb(0x84, 0x84, 0x8a),
        code: rgb(0x9d, 0x4e, 0xdd),
        accent: rgb(0x00, 0x9f, 0xff),
        good: rgb(0x19, 0x9f, 0x43),
        bad: rgb(0xe5, 0x48, 0x4d),
        warning: rgb(0xc9, 0x83, 0x00),
        model: rgb(0x6e, 0x56, 0xcf),
    },
    Theme {
        name: "One Light",
        dark: false,
        background: rgb(0xfa, 0xfa, 0xfa),
        sidebar: rgb(0xea, 0xea, 0xeb),
        sunken: rgb(0xf0, 0xf0, 0xf1),
        surface: rgb(0xe5, 0xe5, 0xe6),
        raised: rgb(0xd8, 0xd8, 0xda),
        border: rgb(0xd0, 0xd0, 0xd2),
        text: rgb(0x38, 0x3a, 0x42),
        text_secondary: rgb(0x69, 0x6c, 0x77),
        text_muted: rgb(0x84, 0x86, 0x8f),
        code: rgb(0x98, 0x68, 0x01),
        accent: rgb(0x40, 0x78, 0xf2),
        good: rgb(0x50, 0xa1, 0x4f),
        bad: rgb(0xe4, 0x56, 0x49),
        warning: rgb(0xc1, 0x84, 0x01),
        model: rgb(0xa6, 0x26, 0xa4),
    },
    Theme {
        name: "Catppuccin Latte",
        dark: false,
        background: rgb(0xef, 0xf1, 0xf5),
        sidebar: rgb(0xe6, 0xe9, 0xef),
        sunken: rgb(0xdc, 0xe0, 0xe8),
        surface: rgb(0xdc, 0xe0, 0xe8),
        raised: rgb(0xcc, 0xd0, 0xda),
        border: rgb(0xbc, 0xc0, 0xcc),
        text: rgb(0x4c, 0x4f, 0x69),
        text_secondary: rgb(0x5c, 0x5f, 0x77),
        text_muted: rgb(0x7c, 0x7f, 0x93),
        code: rgb(0xfe, 0x64, 0x0b),
        accent: rgb(0x88, 0x39, 0xef),
        good: rgb(0x40, 0xa0, 0x2b),
        bad: rgb(0xd2, 0x0f, 0x39),
        warning: rgb(0xdf, 0x8e, 0x1d),
        model: rgb(0x72, 0x87, 0xfd),
    },
    Theme {
        name: "Solarized Light",
        dark: false,
        background: rgb(0xfd, 0xf6, 0xe3),
        sidebar: rgb(0xee, 0xe8, 0xd5),
        sunken: rgb(0xf5, 0xef, 0xdc),
        surface: rgb(0xee, 0xe8, 0xd5),
        raised: rgb(0xe1, 0xdb, 0xc8),
        border: rgb(0xd6, 0xcf, 0xba),
        text: rgb(0x07, 0x36, 0x42),
        text_secondary: rgb(0x58, 0x6e, 0x75),
        text_muted: rgb(0x7b, 0x8e, 0x91),
        code: rgb(0xcb, 0x4b, 0x16),
        accent: rgb(0x26, 0x8b, 0xd2),
        good: rgb(0x85, 0x99, 0x00),
        bad: rgb(0xdc, 0x32, 0x2f),
        warning: rgb(0xb5, 0x89, 0x00),
        model: rgb(0x6c, 0x71, 0xc4),
    },
    Theme {
        name: "Vitesse Light",
        dark: false,
        background: rgb(0xff, 0xff, 0xff),
        sidebar: rgb(0xf7, 0xf7, 0xf7),
        sunken: rgb(0xf1, 0xf1, 0xf1),
        surface: rgb(0xf0, 0xf0, 0xf0),
        raised: rgb(0xe4, 0xe4, 0xe4),
        border: rgb(0xdd, 0xdd, 0xdd),
        text: rgb(0x39, 0x3a, 0x34),
        text_secondary: rgb(0x5f, 0x60, 0x5a),
        text_muted: rgb(0x8a, 0x8f, 0x8a),
        code: rgb(0xa6, 0x5e, 0x2b),
        accent: rgb(0x1c, 0x6b, 0x48),
        good: rgb(0x1e, 0x75, 0x4f),
        bad: rgb(0xab, 0x59, 0x59),
        warning: rgb(0xb0, 0x7d, 0x00),
        model: rgb(0x2e, 0x8f, 0x82),
    },
    Theme {
        name: "Min Light",
        dark: false,
        background: rgb(0xff, 0xff, 0xff),
        sidebar: rgb(0xf6, 0xf6, 0xf6),
        sunken: rgb(0xf0, 0xf0, 0xf0),
        surface: rgb(0xef, 0xef, 0xef),
        raised: rgb(0xe2, 0xe2, 0xe2),
        border: rgb(0xda, 0xda, 0xda),
        text: rgb(0x24, 0x29, 0x2e),
        text_secondary: rgb(0x58, 0x60, 0x69),
        text_muted: rgb(0x8b, 0x90, 0x96),
        code: rgb(0xd3, 0x2f, 0x2f),
        accent: rgb(0x19, 0x76, 0xd2),
        good: rgb(0x22, 0x86, 0x3a),
        bad: rgb(0xd3, 0x2f, 0x2f),
        warning: rgb(0xc7, 0x77, 0x00),
        model: rgb(0x6f, 0x42, 0xc1),
    },
    Theme {
        name: "Rosé Pine Dawn",
        dark: false,
        background: rgb(0xfa, 0xf4, 0xed),
        sidebar: rgb(0xf2, 0xe9, 0xe1),
        sunken: rgb(0xf4, 0xed, 0xe8),
        surface: rgb(0xf2, 0xe9, 0xe1),
        raised: rgb(0xe4, 0xdc, 0xd4),
        border: rgb(0xdf, 0xda, 0xd9),
        text: rgb(0x57, 0x52, 0x79),
        text_secondary: rgb(0x79, 0x75, 0x93),
        text_muted: rgb(0x8f, 0x8a, 0xa0),
        code: rgb(0xd7, 0x82, 0x7e),
        accent: rgb(0xb4, 0x63, 0x7a),
        good: rgb(0x28, 0x69, 0x83),
        bad: rgb(0xb4, 0x63, 0x7a),
        warning: rgb(0xea, 0x9d, 0x34),
        model: rgb(0x90, 0x7a, 0xa9),
    },
    Theme {
        name: "Slack Ochin",
        dark: false,
        background: rgb(0xff, 0xff, 0xff),
        sidebar: rgb(0xe9, 0xed, 0xf2),
        sunken: rgb(0xf2, 0xf4, 0xf7),
        surface: rgb(0xed, 0xf0, 0xf4),
        raised: rgb(0xdc, 0xe2, 0xe9),
        border: rgb(0xd0, 0xd7, 0xdf),
        text: rgb(0x00, 0x23, 0x39),
        text_secondary: rgb(0x3d, 0x54, 0x68),
        text_muted: rgb(0x6f, 0x84, 0x96),
        code: rgb(0xa4, 0x41, 0x85),
        accent: rgb(0x2f, 0x6a, 0xa3),
        good: rgb(0x35, 0x7b, 0x42),
        bad: rgb(0xb0, 0x27, 0x67),
        warning: rgb(0xb0, 0x74, 0x00),
        model: rgb(0x7b, 0x30, 0xd0),
    },
];

/// Which of [`THEMES`] is in use.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Puts the theme called `name` in use. Returns whether there is one.
pub fn choose(name: &str) -> bool {
    let Some(index) = THEMES.iter().position(|theme| theme.name == name) else {
        return false;
    };

    ACTIVE.store(index, Ordering::Relaxed);

    true
}

/// The theme in use.
pub fn current() -> &'static Theme {
    &THEMES[ACTIVE.load(Ordering::Relaxed)]
}

/// Which theme is in use, as a number that changes when another is
/// chosen. Whatever was laid out in one theme's colours is keyed by it.
pub fn index() -> usize {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn background() -> Color32 {
    current().background
}

pub fn sidebar() -> Color32 {
    current().sidebar
}

pub fn sunken() -> Color32 {
    current().sunken
}

pub fn surface() -> Color32 {
    current().surface
}

pub fn raised() -> Color32 {
    current().raised
}

pub fn border() -> Color32 {
    current().border
}

pub fn text() -> Color32 {
    current().text
}

pub fn text_secondary() -> Color32 {
    current().text_secondary
}

pub fn text_muted() -> Color32 {
    current().text_muted
}

pub fn code() -> Color32 {
    current().code
}

pub fn accent() -> Color32 {
    current().accent
}

pub fn good() -> Color32 {
    current().good
}

pub fn bad() -> Color32 {
    current().bad
}

pub fn warning() -> Color32 {
    current().warning
}

pub fn model() -> Color32 {
    current().model
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance, as WCAG defines it.
    fn luminance(color: Color32) -> f32 {
        let channel = |value: u8| {
            let value = f32::from(value) / 255.0;

            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };

        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(one: Color32, other: Color32) -> f32 {
        let (light, dark) = (
            luminance(one).max(luminance(other)),
            luminance(one).min(luminance(other)),
        );

        (light + 0.05) / (dark + 0.05)
    }

    /// Body text meets the usual 4.5 to 1; the quieter voices are held
    /// to less, as the themes these follow are soft on purpose.
    #[test]
    fn text_can_be_read_on_every_ground_of_every_theme() {
        for theme in &THEMES {
            for ground in [theme.background, theme.sidebar, theme.surface, theme.raised] {
                assert!(
                    contrast(theme.text, ground) >= 4.5,
                    "{}: text on {ground:?}",
                    theme.name
                );
                assert!(
                    contrast(theme.text_secondary, ground) >= 3.0,
                    "{}: secondary text on {ground:?}",
                    theme.name
                );
                assert!(
                    contrast(theme.text_muted, ground) >= 2.4,
                    "{}: muted text on {ground:?}",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn a_theme_is_dark_exactly_when_its_ground_is() {
        for theme in &THEMES {
            assert_eq!(
                theme.dark,
                luminance(theme.background) < 0.2,
                "{}",
                theme.name
            );
        }
    }

    #[test]
    fn the_default_theme_exists_and_names_are_distinct() {
        assert!(THEMES.iter().any(|theme| theme.name == DEFAULT));

        for (index, theme) in THEMES.iter().enumerate() {
            assert!(
                THEMES[..index]
                    .iter()
                    .all(|earlier| earlier.name != theme.name)
            );
        }
    }
}

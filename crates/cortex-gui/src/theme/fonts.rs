//! Fonts.
//!
//! The app is set in the system's own typefaces: San Francisco for the
//! interface, New York for the assistant's words, SF Mono for code. All
//! three are variable fonts, so each use picks the weight and the optical
//! size that suit it: a serif drawn for headlines is thin and spiky at
//! reading size, and the same font's text cut is not.
//!
//! Windows and Linux have other typefaces. There each family takes the
//! nearest one the system has: Segoe UI, Georgia and Cascadia or Consolas
//! on Windows; Noto or DejaVu on Linux. Those are mostly not variable, so
//! the heavier families name a bold file of their own. Where none of a
//! family's files is found, egui's built-in fonts are used instead.

use std::fs;
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontTweak};
use eframe::epaint::text::VariationCoords;

/// The serif the assistant speaks in.
pub const VOICE: &str = "voice";

/// The same serif, heavier, for headings in the assistant's words.
pub const VOICE_STRONG: &str = "voice-strong";

/// The interface font, heavier, for titles and labels that need weight.
pub const STRONG: &str = "strong";

/// Each list is tried in order. The first file is the Mac's.
const SANS: &[&str] = &[
    "/System/Library/Fonts/SFNS.ttf",
    "C:/Windows/Fonts/SegUIVar.ttf",
    "C:/Windows/Fonts/segoeui.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
];

const SANS_STRONG: &[&str] = &[
    "/System/Library/Fonts/SFNS.ttf",
    "C:/Windows/Fonts/SegUIVar.ttf",
    "C:/Windows/Fonts/seguisb.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
];

const SERIF: &[&str] = &[
    "/System/Library/Fonts/NewYork.ttf",
    "C:/Windows/Fonts/georgia.ttf",
    "/usr/share/fonts/truetype/noto/NotoSerif-Regular.ttf",
    "/usr/share/fonts/noto/NotoSerif-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSerif-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
    "/usr/share/fonts/TTF/DejaVuSerif.ttf",
    "/usr/share/fonts/dejavu-serif-fonts/DejaVuSerif.ttf",
];

const SERIF_STRONG: &[&str] = &[
    "/System/Library/Fonts/NewYork.ttf",
    "C:/Windows/Fonts/georgiab.ttf",
    "/usr/share/fonts/truetype/noto/NotoSerif-Bold.ttf",
    "/usr/share/fonts/noto/NotoSerif-Bold.ttf",
    "/usr/share/fonts/google-noto/NotoSerif-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf",
    "/usr/share/fonts/TTF/DejaVuSerif-Bold.ttf",
    "/usr/share/fonts/dejavu-serif-fonts/DejaVuSerif-Bold.ttf",
];

const MONO: &[&str] = &[
    "/System/Library/Fonts/SFNSMono.ttf",
    "C:/Windows/Fonts/CascadiaMono.ttf",
    "C:/Windows/Fonts/consola.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
];

/// One cut of a typeface: which file, and where on its axes.
struct Face {
    family: Family,

    /// The files that will do, best first.
    files: &'static [&'static str],

    /// 400 is regular, 600 semibold.
    weight: f32,

    /// The size, in points, the letterforms are drawn for. Lower values
    /// are sturdier and more widely spaced, for small text.
    optical_size: f32,
}

#[derive(Clone, Copy)]
enum Family {
    Interface,
    Strong,
    Mono,
    Voice,
    VoiceStrong,
}

impl Family {
    fn key(self) -> FontFamily {
        match self {
            Self::Interface => FontFamily::Proportional,
            Self::Mono => FontFamily::Monospace,
            Self::Strong => FontFamily::Name(STRONG.into()),
            Self::Voice => FontFamily::Name(VOICE.into()),
            Self::VoiceStrong => FontFamily::Name(VOICE_STRONG.into()),
        }
    }

    /// What the family falls back to when its typeface is missing.
    fn fallback(self) -> FontFamily {
        match self {
            Self::Mono => FontFamily::Monospace,
            _ => FontFamily::Proportional,
        }
    }
}

const FACES: [Face; 5] = [
    Face {
        family: Family::Interface,
        files: SANS,
        weight: 400.0,
        optical_size: 17.0,
    },
    Face {
        family: Family::Strong,
        files: SANS_STRONG,
        weight: 600.0,
        optical_size: 17.0,
    },
    Face {
        family: Family::Mono,
        files: MONO,
        weight: 400.0,
        optical_size: 13.0,
    },
    Face {
        family: Family::Voice,
        files: SERIF,
        weight: 400.0,
        optical_size: 14.0,
    },
    Face {
        family: Family::VoiceStrong,
        files: SERIF_STRONG,
        weight: 600.0,
        optical_size: 18.0,
    },
];

pub fn install(context: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    for (index, face) in FACES.iter().enumerate() {
        // Every family starts as a copy of its fallback, so that text is
        // still drawn when the typeface is missing, and so that characters
        // the typeface lacks are found in the built-in fonts behind it.
        let mut stack = fonts
            .families
            .get(&face.family.fallback())
            .cloned()
            .unwrap_or_default();

        if let Some(data) = load(face) {
            let name = format!("face-{index}");

            fonts.font_data.insert(name.clone(), Arc::new(data));

            stack.insert(0, name);
        }

        fonts.families.insert(face.family.key(), stack);
    }

    context.set_fonts(fonts);
}

fn load(face: &Face) -> Option<FontData> {
    let bytes = face.files.iter().find_map(|file| fs::read(file).ok())?;

    let data = FontData::from_owned(bytes);

    let coords = coordinates(&data, face);

    Some(data.tweak(FontTweak {
        coords,
        ..FontTweak::default()
    }))
}

/// The face's place on the font's axes. Only axes the font really has are
/// set, each kept inside the range the font allows.
fn coordinates(data: &FontData, face: &Face) -> VariationCoords {
    let wanted = [(*b"wght", face.weight), (*b"opsz", face.optical_size)];

    let mut coords = VariationCoords::default();

    for axis in data.variation_axes() {
        let Some((tag, value)) = wanted.iter().find(|(tag, _)| axis.tag == *tag) else {
            continue;
        };

        coords.push(tag, value.clamp(axis.range.min, axis.range.max));
    }

    coords
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a Mac every face should load and be placed on both axes. On
    /// other machines there is nothing to check.
    #[test]
    fn the_system_typefaces_are_variable_in_weight_and_optical_size() {
        for face in &FACES {
            // Only the Mac's own files are variable on both axes.
            let file = face.files[0];

            let Ok(bytes) = fs::read(file) else {
                continue;
            };

            let data = FontData::from_owned(bytes);
            let coords = coordinates(&data, face);

            assert!(coords != VariationCoords::default(), "{file} has no axes");
        }
    }
}

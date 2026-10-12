//! Every colour the app uses. Warm neutrals, one accent.

use eframe::egui::Color32;

use crate::memory::Region;

pub const BACKGROUND: Color32 = Color32::from_rgb(0x1f, 0x1e, 0x1d);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x19, 0x18, 0x17);
pub const SUNKEN: Color32 = Color32::from_rgb(0x17, 0x16, 0x15);
pub const SURFACE: Color32 = Color32::from_rgb(0x29, 0x28, 0x26);
pub const RAISED: Color32 = Color32::from_rgb(0x34, 0x33, 0x30);
pub const BORDER: Color32 = Color32::from_rgb(0x3a, 0x39, 0x36);

pub const TEXT: Color32 = Color32::from_rgb(0xec, 0xe9, 0xe1);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xa9, 0xa6, 0x9d);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x96, 0x93, 0x89);

/// Code set inside a sentence.
pub const CODE: Color32 = Color32::from_rgb(0xe6, 0xc9, 0xa8);

/// Clay: what the agent does (send, the newest memory).
pub const ACCENT: Color32 = Color32::from_rgb(0xd9, 0x77, 0x57);

pub const GOOD: Color32 = Color32::from_rgb(0x5d, 0xca, 0x9a);
pub const BAD: Color32 = Color32::from_rgb(0xe5, 0x67, 0x5f);
pub const WARNING: Color32 = Color32::from_rgb(0xe2, 0xab, 0x52);

/// A model response in the loop strip.
pub const MODEL: Color32 = Color32::from_rgb(0x8f, 0x87, 0xe8);

/// The colour of each kind of memory, in the brain and on its labels.
pub fn region(region: Region) -> Color32 {
    match region {
        Region::ShortTerm => Color32::from_rgb(0x5c, 0xe6, 0xff),
        Region::Episodic => Color32::from_rgb(0xff, 0x8f, 0x5c),
        Region::LongTerm => Color32::from_rgb(0xd8, 0x9a, 0xff),
        Region::Neural => Color32::from_rgb(0x50, 0xc8, 0xff),
        Region::Intuition => Color32::from_rgb(0xff, 0xd8, 0x5c),
    }
}

/// An episode whose run failed or stopped early.
pub const TROUBLED: Color32 = Color32::from_rgb(0xff, 0x4d, 0x5e);

/// The links that carry episodes into long-term memory, and the tracts.
pub const TRACT: Color32 = Color32::from_rgb(0x4d, 0xff, 0x7a);

//! Text laid out ahead of drawing, and kept.
//!
//! Laying text out (finding where each line breaks and where each glyph
//! sits) is the dear part of drawing a conversation. A message that has
//! not changed is laid out once here and found again on every later
//! frame by what it says, how wide it may run, the theme and the scale
//! of the screen. Because the result is known before anything is drawn,
//! its height is too: that is what lets a long conversation draw only
//! the messages on screen and still scroll truly.
//!
//! The store is bounded. Past its limits the layouts that were used
//! longest ago are dropped; they are laid out again if they are scrolled
//! back to.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::text::LayoutJob;
use eframe::egui::{self, Galley};

use crate::theme::palette;

/// Most layouts kept.
const MAX_ENTRIES: usize = 4_000;

/// Most memory the kept layouts may take, by [`weight`]'s estimate.
const MAX_BYTES: usize = 48 * 1024 * 1024;

/// What a layout is found by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    /// A hash of the text and of how it is set.
    content: u64,

    /// Widest a line may run, in tenths of a point.
    width: u32,

    /// The theme whose colours it was laid out in.
    theme: u16,

    /// Screen pixels to a point, in thousandths.
    scale: u32,
}

impl Key {
    /// `content` is anything that tells this text and its styling from
    /// every other: see [`hash`].
    pub fn new(context: &egui::Context, content: u64, width: f32) -> Self {
        Self {
            content,
            width: tenths(width),
            theme: palette::index() as u16,
            scale: (context.pixels_per_point() * 1000.0).round() as u32,
        }
    }
}

/// A width as the whole number of tenths of a point it is nearest to, so
/// that two widths a rounding error apart find the same layout.
fn tenths(width: f32) -> u32 {
    if width.is_finite() {
        (width.max(0.0) * 10.0).round() as u32
    } else {
        u32::MAX
    }
}

/// A hash to make a [`Key`] of.
pub fn hash(value: impl std::hash::Hash) -> u64 {
    egui::util::hash(value)
}

struct Entry {
    galley: Arc<Galley>,
    weight: usize,

    /// When it was last asked for, by the store's own clock.
    used: u64,
}

/// How often the store was asked, and how often it had the answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub found: u64,
    pub laid_out: u64,
    pub dropped: u64,
}

pub struct Typesetter {
    entries: HashMap<Key, Entry>,
    weight: usize,
    clock: u64,
    max_entries: usize,
    max_weight: usize,

    /// How full the glyph atlas was when last looked at.
    atlas_fill: f32,

    /// How many times the atlas has been seen to start afresh.
    restarts: u64,

    counts: Counts,
}

impl Default for Typesetter {
    fn default() -> Self {
        Self::with_limits(MAX_ENTRIES, MAX_BYTES)
    }
}

impl Typesetter {
    pub fn with_limits(max_entries: usize, max_weight: usize) -> Self {
        Self {
            entries: HashMap::new(),
            weight: 0,
            clock: 0,
            max_entries: max_entries.max(1),
            max_weight,
            atlas_fill: 0.0,
            restarts: 0,
            counts: Counts::default(),
        }
    }

    /// The layout kept under `key`, or the one `lay_out` makes, which is
    /// then kept.
    pub fn get(&mut self, key: Key, lay_out: impl FnOnce() -> Arc<Galley>) -> Arc<Galley> {
        self.clock += 1;

        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.clock;
            self.counts.found += 1;

            return entry.galley.clone();
        }

        let galley = lay_out();
        let weight = weight(&galley);

        self.counts.laid_out += 1;
        self.weight += weight;

        self.entries.insert(
            key,
            Entry {
                galley: galley.clone(),
                weight,
                used: self.clock,
            },
        );

        self.trim();

        galley
    }

    /// Drops the layouts used longest ago until the store is inside its
    /// limits again. It goes a little under them, so that a store at its
    /// limit does not sort itself on every new layout.
    fn trim(&mut self) {
        if self.entries.len() <= self.max_entries && self.weight <= self.max_weight {
            return;
        }

        let entries = self.max_entries - self.max_entries / 8;
        let weight = self.max_weight - self.max_weight / 8;

        let mut oldest: Vec<(u64, Key, usize)> = self
            .entries
            .iter()
            .map(|(key, entry)| (entry.used, *key, entry.weight))
            .collect();

        oldest.sort_unstable_by_key(|(used, ..)| *used);

        // The newest is the one just asked for; it always stays.
        let droppable = oldest.len().saturating_sub(1);

        for (_, key, entry_weight) in oldest.into_iter().take(droppable) {
            if self.entries.len() <= entries && self.weight <= weight {
                break;
            }

            self.entries.remove(&key);
            self.weight -= entry_weight;
            self.counts.dropped += 1;
        }
    }

    /// Forgets everything. A layout holds places in the glyph atlas, so
    /// none survives the atlas being made again.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.weight = 0;
    }

    /// Looks at the glyph atlas, once a frame. egui starts the atlas
    /// afresh when it fills up, which leaves every kept layout pointing
    /// at glyphs that are no longer where it says. An atlas that is
    /// emptier than it was has been started afresh. Returns whether the
    /// store was emptied because of it.
    pub fn watch_atlas(&mut self, fill: f32) -> bool {
        let restarted = fill < self.atlas_fill;

        self.atlas_fill = fill;

        if restarted {
            self.restarts += 1;
            self.clear();
        }

        restarted
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn weight(&self) -> usize {
        self.weight
    }

    #[cfg(test)]
    pub fn counts(&self) -> Counts {
        self.counts
    }
}

/// Roughly the memory a layout takes: its text, its glyphs, and the
/// triangles they are drawn with.
fn weight(galley: &Galley) -> usize {
    const VERTEX: usize = 20;
    const INDEX: usize = 4;
    const GLYPH: usize = 48;
    const ROW: usize = 96;

    let glyphs: usize = galley.rows.iter().map(|row| row.glyphs.len()).sum();

    galley.job.text.len()
        + galley.job.sections.len() * 96
        + galley.num_vertices * VERTEX
        + galley.num_indices * INDEX
        + glyphs * GLYPH
        + galley.rows.len() * ROW
        + 256
}

type Shared = Arc<Mutex<Typesetter>>;

/// The window's own store. Each `egui::Context` has one, since a layout
/// belongs to the glyph atlas of the context it was made in.
fn shared(context: &egui::Context) -> Shared {
    context.data_mut(|data| {
        data.get_temp_mut_or_default::<Shared>(egui::Id::new("typesetter"))
            .clone()
    })
}

/// Lays `job` out, or finds it laid out already. `content` tells the
/// job from every other (see [`hash`]); the job's own wrap width is part
/// of what it is found by.
pub fn layout(
    context: &egui::Context,
    content: u64,
    job: impl FnOnce() -> LayoutJob,
) -> Arc<Galley> {
    layout_at(context, content, f32::INFINITY, job)
}

/// As [`layout`], for a job whose width is not fixed by `content`.
pub fn layout_at(
    context: &egui::Context,
    content: u64,
    width: f32,
    job: impl FnOnce() -> LayoutJob,
) -> Arc<Galley> {
    let key = Key::new(context, content, width);
    let store = shared(context);

    // The store is not the context's, so the context is free to be asked
    // for its fonts while the store is held.
    let mut store = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    store.get(key, || direct(context, job()))
}

/// Lays `job` out without keeping it: for text that will be different
/// on the next frame, such as a response still being written.
pub fn direct(context: &egui::Context, job: LayoutJob) -> Arc<Galley> {
    context.fonts_mut(|fonts| fonts.layout_job(job))
}

/// Called once a frame, whatever is on screen. Returns whether every
/// kept layout was dropped, in which case anything holding one of its
/// own must drop it too.
pub fn begin_frame(context: &egui::Context) -> bool {
    let fill = context.fonts(|fonts| fonts.font_atlas_fill_ratio());
    let store = shared(context);

    let mut store = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    store.watch_atlas(fill)
}

/// A number that changes whenever the kept layouts were all dropped.
/// Whoever holds layouts of their own compares it with the one they
/// last saw.
pub fn generation(context: &egui::Context) -> u64 {
    let store = shared(context);
    let store = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    store.restarts
}

/// What the store has done so far, for tests and for measuring.
#[cfg(test)]
pub fn counts(context: &egui::Context) -> Counts {
    let store = shared(context);
    let store = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    store.counts()
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Color32, FontId};

    use super::*;

    fn context() -> egui::Context {
        let context = egui::Context::default();

        // Fonts exist only once a frame has begun.
        let mut output = context.run_ui(egui::RawInput::default(), |_| {});

        output.textures_delta.clear();

        context
    }

    fn job(text: &str) -> LayoutJob {
        LayoutJob::simple(
            text.to_string(),
            FontId::proportional(14.0),
            Color32::WHITE,
            200.0,
        )
    }

    #[test]
    fn the_same_text_is_laid_out_once() {
        let context = context();

        let first = layout_at(&context, hash("hello"), 200.0, || job("hello"));
        let again = layout_at(&context, hash("hello"), 200.0, || panic!("laid out twice"));

        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(counts(&context).laid_out, 1);
        assert_eq!(counts(&context).found, 1);
    }

    #[test]
    fn another_width_theme_or_scale_is_another_layout() {
        let context = context();

        let narrow = Key::new(&context, 7, 200.0);
        let wide = Key::new(&context, 7, 300.0);

        assert_ne!(narrow, wide);
        assert_eq!(narrow, Key::new(&context, 7, 200.004));

        context.set_pixels_per_point(2.0);

        // A new scale takes effect with the next frame.
        let mut output = context.run_ui(egui::RawInput::default(), |_| {});

        output.textures_delta.clear();

        assert_ne!(narrow, Key::new(&context, 7, 200.0));
    }

    #[test]
    fn the_layouts_used_longest_ago_are_dropped_first() {
        let context = context();
        let mut store = Typesetter::with_limits(8, usize::MAX);

        let key = |number: u64| Key::new(&context, number, 200.0);
        let lay_out = |number: u64| direct(&context, job(&format!("line {number}")));

        for number in 0..8 {
            store.get(key(number), || lay_out(number));
        }

        // The first is used again, so it is no longer the oldest.
        store.get(key(0), || panic!("it was kept"));

        store.get(key(8), || lay_out(8));

        assert!(store.len() <= 8);
        assert!(store.counts().dropped >= 1);

        store.get(key(0), || panic!("the one used most recently was dropped"));
        store.get(key(8), || panic!("the newest was dropped"));

        let before = store.counts().laid_out;

        store.get(key(1), || lay_out(1));

        assert_eq!(store.counts().laid_out, before + 1, "the oldest was kept");
    }

    #[test]
    fn the_store_stays_under_its_byte_limit() {
        let context = context();

        let one = weight(&direct(&context, job(&"word ".repeat(40))));
        let mut store = Typesetter::with_limits(usize::MAX, one * 5);

        for number in 0..200_u64 {
            let text = format!("{number} {}", "word ".repeat(40));

            store.get(Key::new(&context, number, 200.0), || {
                direct(&context, job(&text))
            });

            assert!(store.weight() <= one * 6, "{} bytes kept", store.weight());
        }

        assert!(store.len() <= 6);
        assert_eq!(
            store.weight(),
            store
                .entries
                .values()
                .map(|entry| entry.weight)
                .sum::<usize>()
        );
    }

    #[test]
    fn an_atlas_started_afresh_empties_the_store() {
        let context = context();
        let mut store = Typesetter::default();

        store.get(Key::new(&context, 1, 200.0), || {
            direct(&context, job("kept"))
        });

        assert!(!store.watch_atlas(0.4));
        assert!(!store.watch_atlas(0.85));
        assert_eq!(store.len(), 1);

        assert!(store.watch_atlas(0.02));
        assert_eq!(store.len(), 0);
        assert_eq!(store.weight(), 0);
    }
}

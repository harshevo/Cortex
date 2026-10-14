//! The whole cortex, drawn: every cell, every synapse, and what each part
//! of the brain is doing right now.
//!
//! Columns sit on a sheet shaped like a brain seen from the side, where
//! the brain's own map put them (columns that fire together drift
//! together during sleep). Each column's cells are a small cluster around
//! it. All synapses are rasterised into one image, brighter where more
//! run, connected ones brighter than those still forming; it is redrawn
//! when the wiring is fetched again.
//!
//! Over that, the last few seconds of activity are played back a moment
//! behind real time: each cell a process used glows in that process's
//! colour and fades, cells it primed (what it expects next) get a ring,
//! cells whose synapses just changed get a plasticity ring, and while a
//! wave is fresh the synapses carrying it, from the spiking cells to the
//! cells they primed, are drawn in its colour.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use cortex_living::Living;
use cortex_living::neural::map;
use cortex_living::neural::mind::{self, Activity, Wiring};
use cortex_living::neural::NeuralSnapshot;
use cortex_living::service::Request;
use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, Rect, Sense, Stroke, TextureHandle, Ui, pos2, vec2,
};

use crate::theme;
use crate::views::brain::count;

/// How far behind real time activity is played, so that a snapshot fetched
/// once a second still plays smoothly, and how long a lit cell glows.
const DELAY_MS: f64 = 1_400.0;
const GLOW_MS: f64 = 2_600.0;
/// How long the synapses carrying a wave stay drawn.
const WAVE_MS: f64 = 1_800.0;
const REFETCH: Duration = Duration::from_secs(15);

/// The connectome image's size; the frame is 1 wide and `map::HEIGHT` high.
const IMAGE_WIDTH: usize = 1400;

const CANVAS: Color32 = Color32::from_rgb(10, 13, 19);
const RESTING: Color32 = Color32::from_rgb(70, 96, 140);
const PLASTIC: Color32 = Color32::from_rgb(45, 212, 191);
const MEMORY: Color32 = Color32::from_rgb(250, 250, 250);

/// The colour of each process, everywhere it is shown.
pub fn colour(process: &str) -> Color32 {
    match process {
        "learn" => Color32::from_rgb(59, 130, 246),
        "hear" => Color32::from_rgb(34, 211, 238),
        "speak" => Color32::from_rgb(34, 197, 94),
        "recall" => Color32::from_rgb(234, 179, 8),
        "replay" => Color32::from_rgb(129, 140, 248),
        "dream" => Color32::from_rgb(168, 85, 247),
        "think" => Color32::from_rgb(163, 230, 53),
        "imagine" => Color32::from_rgb(236, 72, 153),
        "reason" => Color32::from_rgb(249, 115, 22),
        "intuit" => Color32::from_rgb(239, 68, 68),
        "decide" => Color32::from_rgb(245, 245, 244),
        "aware" => Color32::from_rgb(253, 224, 171),
        "reflect" => Color32::from_rgb(253, 224, 171),
        _ => Color32::from_rgb(148, 163, 184),
    }
}

/// What each process is, for the legend.
pub fn meaning(process: &str) -> &'static str {
    match process {
        "learn" => "being taught",
        "hear" => "hearing you",
        "speak" => "speaking",
        "recall" => "recalling a memory",
        "replay" => "deep sleep: replaying a memory and learning it again",
        "dream" => "dreaming: loose replay that jumps between memories",
        "think" => "thinking: one thing leading to another, unprompted",
        "imagine" => "imagining: two memories spliced at a shared word",
        "reason" => "reasoning: searching for a chain of learned steps",
        "intuit" => "intuition: the first thing primed, taken at once",
        "decide" => "deciding between near rivals",
        "aware" => "attending to itself",
        _ => "",
    }
}

/// The wiring, placed and drawn, ready to show.
pub struct Drawn {
    clock: u64,
    area: usize,
    /// Where each cell is, in the frame; `None` for a dead one.
    at: Vec<Option<(f32, f32)>>,
    column: Vec<u32>,
    alive: usize,
    synapses: usize,
    connected: usize,
    /// Each cell's incoming synapses' source cells.
    incoming: HashMap<u32, Vec<u32>>,
    engrams: HashMap<u64, Vec<u32>>,
    image: Option<ColorImage>,
    texture: Option<TextureHandle>,
}

#[derive(Default)]
pub struct MapView {
    drawn: Option<Drawn>,
    pending: Option<Receiver<Result<Drawn, String>>>,
    fetched: Option<Instant>,
    pub problem: Option<String>,
    /// Show only this process's activity.
    pub only: Option<String>,
}

/// Places every cell and rasterises every synapse.
fn place(wiring: Wiring) -> Drawn {
    let slots = map::slots(wiring.area);
    let spacing = map::spacing(wiring.area);
    let mut in_column: HashMap<u32, u32> = HashMap::new();
    for column in &wiring.cells {
        if *column != u32::MAX {
            *in_column.entry(*column).or_default() += 1;
        }
    }
    let mut rank: HashMap<u32, u32> = HashMap::new();
    let at: Vec<Option<(f32, f32)>> = wiring
        .cells
        .iter()
        .map(|column| {
            if *column == u32::MAX {
                return None;
            }
            let slot = *wiring.slot_of.get(*column as usize).unwrap_or(column) as usize;
            let (x, y) = *slots.get(slot)?;
            let r = rank.entry(*column).or_default();
            let n = in_column.get(column).copied().unwrap_or(1) as f32;
            // A sunflower spiral within the column's patch.
            let radius = spacing * 0.42 * ((*r as f32 + 0.5) / n).sqrt();
            let angle = *r as f32 * 2.399_963;
            *r += 1;
            Some((x + radius * angle.cos(), y + radius * angle.sin()))
        })
        .collect();
    let synapses = mind::unpack(&wiring.synapses);
    let mut incoming: HashMap<u32, Vec<u32>> = HashMap::new();
    let width = IMAGE_WIDTH;
    let height = (IMAGE_WIDTH as f32 * map::HEIGHT) as usize;
    let mut density = vec![0f32; width * height];
    let mut strong = vec![0f32; width * height];
    let mut connected = 0;
    for (from, to, is_connected) in &synapses {
        incoming.entry(*to).or_default().push(*from);
        connected += usize::from(*is_connected);
        let (Some(Some(a)), Some(Some(b))) = (at.get(*from as usize), at.get(*to as usize)) else {
            continue;
        };
        let (ax, ay) = (a.0 * width as f32, a.1 * width as f32);
        let (bx, by) = (b.0 * width as f32, b.1 * width as f32);
        let steps = (bx - ax).abs().max((by - ay).abs()).ceil().max(1.0) as usize;
        let target = if *is_connected { &mut strong } else { &mut density };
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let x = (ax + (bx - ax) * t) as usize;
            let y = (ay + (by - ay) * t) as usize;
            if x < width && y < height {
                target[y * width + x] += 1.0;
            }
        }
    }
    // Brightness grows with the log of how many synapses cross a pixel,
    // so that both a lone synapse and a dense bundle are seen.
    let high = strong
        .iter()
        .zip(&density)
        .map(|(s, d)| s + 0.4 * d)
        .fold(0.0_f32, f32::max)
        .max(1.0);
    let scale = (1.0 + high).ln();
    let pixels = strong
        .iter()
        .zip(&density)
        .map(|(s, d)| {
            if *s == 0.0 && *d == 0.0 {
                return Color32::TRANSPARENT;
            }
            let level = ((1.0 + s + 0.4 * d).ln() / scale).clamp(0.0, 1.0);
            let share = s / (s + 0.4 * d).max(1e-6);
            let alpha = 0.02 + 0.30 * level.powf(1.4);
            // Connected: blue; still forming: grey-violet.
            let r = 70.0 + 50.0 * (1.0 - share) + 80.0 * level;
            let g = 90.0 + 20.0 * share + 100.0 * level;
            let b = 150.0 + 60.0 * share + 45.0 * level;
            Color32::from_rgba_unmultiplied(r.min(255.0) as u8, g.min(255.0) as u8, b.min(255.0) as u8, (alpha * 255.0) as u8)
        })
        .collect();
    Drawn {
        clock: wiring.clock,
        area: wiring.area,
        alive: at.iter().filter(|a| a.is_some()).count(),
        column: wiring.cells,
        at,
        synapses: wiring.synapse_count,
        connected,
        incoming,
        engrams: wiring.engrams.into_iter().collect(),
        image: Some(ColorImage::new([width, height], pixels)),
        texture: None,
    }
}

impl Drawn {
    /// The synapses a step's activity ran along: from the cells that
    /// spiked onto the cells they primed, as the ends' places.
    fn carrying(&self, activity: &Activity) -> Vec<((f32, f32), (f32, f32))> {
        let spiking: HashSet<u32> = activity.cells.iter().copied().collect();
        let mut lines = Vec::new();
        for target in &activity.primed {
            let Some(Some(b)) = self.at.get(*target as usize) else { continue };
            for from in self.incoming.get(target).into_iter().flatten() {
                if spiking.contains(from)
                    && let Some(Some(a)) = self.at.get(*from as usize)
                {
                    lines.push((*a, *b));
                }
            }
        }
        lines
    }
}

impl MapView {
    /// Fetches the wiring in the background now and then while shown.
    fn refresh(&mut self, living: Option<&Arc<Living>>, clock: u64) {
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(Ok(drawn)) => {
                    self.drawn = Some(drawn);
                    self.problem = None;
                    self.pending = None;
                }
                Ok(Err(problem)) => {
                    self.problem = Some(problem);
                    self.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => self.pending = None,
            }
            return;
        }
        let stale = self.fetched.is_none_or(|at| at.elapsed() >= REFETCH)
            && self.drawn.as_ref().is_none_or(|d| d.clock != clock);
        let (true, Some(living)) = (stale, living) else {
            return;
        };
        self.fetched = Some(Instant::now());
        let living = living.clone();
        let (send, receive) = channel();
        self.pending = Some(receive);
        let _ = std::thread::Builder::new().name("brain-wiring".into()).spawn(move || {
            let drawn = living
                .neural_request(&Request::NeuralWiring)
                .map_err(|e| e.to_string())
                .and_then(|value| serde_json::from_value::<Wiring>(value).map_err(|e| e.to_string()))
                .map(place);
            let _ = send.send(drawn);
        });
    }
}

/// A cell lit by a process.
struct Lit<'a> {
    process: &'a str,
    glow: f32,
}

pub fn show(
    ui: &mut Ui,
    view: &mut MapView,
    brain: &NeuralSnapshot,
    living: Option<&Arc<Living>>,
    engram: Option<u64>,
) {
    view.refresh(living, brain.clock);
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
    let whole = response.rect;
    painter.rect_filled(whole, CornerRadius::ZERO, CANVAS);
    let inner = whole.shrink(16.0);

    // The frame, as large as fits, under the header and above the legend.
    let room = Rect::from_min_max(inner.left_top() + vec2(0.0, 40.0), inner.right_bottom() - vec2(0.0, 58.0));
    let width = room.width().min(room.height() / map::HEIGHT).max(10.0);
    let frame = Rect::from_center_size(room.center(), vec2(width, width * map::HEIGHT));
    let to_screen = |p: (f32, f32)| pos2(frame.left() + p.0 * width, frame.top() + p.1 * width);

    let Some(drawn) = view.drawn.as_mut() else {
        painter.text(
            frame.center(),
            Align2::CENTER_CENTER,
            match &view.problem {
                Some(problem) => format!("Cannot fetch the wiring: {problem}"),
                None => "Fetching every cell and synapse…".into(),
            },
            theme::ui(),
            Color32::from_gray(170),
        );
        ui.ctx().request_repaint_after(Duration::from_millis(200));
        return;
    };
    if let Some(image) = drawn.image.take() {
        drawn.texture = Some(ui.ctx().load_texture("brain connectome", image, egui::TextureOptions::LINEAR));
    }
    if let Some(texture) = &drawn.texture {
        painter.image(
            texture.id(),
            frame,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    // What every cell is doing, played back a moment behind real time.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64;
    let playing = now - DELAY_MS;
    let shown: Vec<&Activity> = brain
        .mind
        .activity
        .iter()
        .filter(|a| (a.time_ms as f64) <= playing && playing - (a.time_ms as f64) < GLOW_MS)
        .filter(|a| view.only.as_ref().is_none_or(|only| *only == a.process))
        .collect();
    let mut lit: HashMap<u32, Lit> = HashMap::new();
    let mut primed: HashMap<u32, (&str, f32)> = HashMap::new();
    let mut plastic: HashMap<u32, f32> = HashMap::new();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for activity in &shown {
        let glow = 1.0 - ((playing - activity.time_ms as f64) / GLOW_MS) as f32;
        *counts.entry(activity.process.as_str()).or_default() += activity.cells.len();
        for cell in &activity.cells {
            let entry = lit.entry(*cell).or_insert(Lit { process: &activity.process, glow: 0.0 });
            if glow >= entry.glow {
                *entry = Lit { process: &activity.process, glow };
            }
            if activity.learned {
                let p = plastic.entry(*cell).or_default();
                *p = p.max(glow);
            }
        }
        for cell in &activity.primed {
            let entry = primed.entry(*cell).or_insert((&activity.process, 0.0));
            if glow * 0.8 >= entry.1 {
                *entry = (&activity.process, glow * 0.8);
            }
        }
    }

    // The synapses carrying the freshest waves: from the cells that
    // spiked to the cells they primed.
    let mut carried = 0usize;
    for activity in shown.iter().filter(|a| playing - (a.time_ms as f64) < WAVE_MS) {
        let fresh = 1.0 - ((playing - activity.time_ms as f64) / WAVE_MS) as f32;
        let colour = colour(&activity.process).gamma_multiply(0.35 * fresh);
        for (a, b) in drawn.carrying(activity) {
            painter.line_segment([to_screen(a), to_screen(b)], Stroke::new(0.6, colour));
            carried += 1;
        }
    }

    // Every cell: dim at rest, glowing in its process's colour when used.
    let dot = (width / 900.0).clamp(0.6, 2.2);
    let memory: HashSet<u32> = engram
        .and_then(|id| drawn.engrams.get(&id))
        .map(|cells| cells.iter().copied().collect())
        .unwrap_or_default();
    let pointer = response.hover_pos();
    let mut hovered: Option<(u32, f32)> = None;
    for (cell, at) in drawn.at.iter().enumerate() {
        let Some(at) = at else { continue };
        let centre = to_screen(*at);
        let cell = cell as u32;
        if let Some(p) = pointer {
            let d = p.distance(centre);
            if d < 8.0 && hovered.is_none_or(|h| d < h.1) {
                hovered = Some((cell, d));
            }
        }
        match lit.get(&cell) {
            Some(l) => {
                let c = colour(l.process);
                painter.circle_filled(centre, dot * 6.0 * l.glow.max(0.3), c.gamma_multiply(0.16 * l.glow));
                painter.circle_filled(centre, dot * (1.6 + 1.8 * l.glow), c.gamma_multiply(0.5 + 0.5 * l.glow));
            }
            None => {
                painter.circle_filled(centre, dot * 0.9, RESTING.gamma_multiply(0.7));
            }
        }
        if let Some((process, strength)) = primed.get(&cell) {
            painter.circle_stroke(centre, dot * 2.6, Stroke::new(0.8, colour(process).gamma_multiply(*strength)));
        }
        if let Some(strength) = plastic.get(&cell) {
            painter.circle_stroke(centre, dot * 3.6, Stroke::new(0.9, PLASTIC.gamma_multiply(*strength)));
        }
        if memory.contains(&cell) {
            painter.circle_stroke(centre, dot * 2.2, Stroke::new(0.9, MEMORY.gamma_multiply(0.8)));
        }
    }

    painter.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        format!(
            "Every cell and synapse drawn: {} cells in {} columns · {} synapses ({} connected) · {} memories",
            count(drawn.alive as u64),
            count(drawn.area as u64),
            count(drawn.synapses as u64),
            count(drawn.connected as u64),
            count(drawn.engrams.len() as u64),
        ),
        theme::small(),
        Color32::from_gray(200),
    );
    let felt = &brain.mind.felt;
    let now_doing = if felt.asleep {
        format!("Asleep: {} · cycle {}", felt.stage, felt.cycle)
    } else {
        format!("Awake · {}", if brain.mind.me.doing.is_empty() { "idle".into() } else { meaning(&brain.mind.me.doing).to_string() })
    };
    painter.text(
        inner.left_top() + vec2(0.0, 18.0),
        Align2::LEFT_TOP,
        format!(
            "{now_doing} · {} cells lit and {} synapses carrying activity now · wiring as of step {}",
            count(lit.len() as u64),
            count(carried as u64),
            count(drawn.clock)
        ),
        theme::small(),
        Color32::from_gray(150),
    );

    if let Some((cell, _)) = hovered
        && let Some(Some(at)) = drawn.at.get(cell as usize)
    {
        let centre = to_screen(*at);
        let doing = lit.get(&cell).map_or("resting".to_string(), |l| meaning(l.process).to_string());
        let inputs = drawn.incoming.get(&cell).map_or(0, Vec::len);
        painter.circle_stroke(centre, 6.0, Stroke::new(1.0, Color32::WHITE));
        painter.text(
            centre + vec2(9.0, -9.0),
            Align2::LEFT_BOTTOM,
            format!(
                "cell {cell} · column {} · {inputs} incoming synapses · {doing}{}",
                drawn.column.get(cell as usize).copied().unwrap_or_default(),
                if primed.contains_key(&cell) { " · primed" } else { "" }
            ),
            theme::small(),
            Color32::WHITE,
        );
    }

    legend(ui, view, Rect::from_min_max(pos2(inner.left(), inner.bottom() - 50.0), inner.right_bottom()), &counts);
    ui.ctx().request_repaint_after(Duration::from_millis(33));
}

/// The colour key, which also filters: click a process to see only it.
fn legend(ui: &mut Ui, view: &mut MapView, rect: Rect, counts: &HashMap<&str, usize>) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(10.0, 4.0);
        for process in mind::PROCESSES {
            let active = view.only.as_deref() == Some(process);
            let dimmed = view.only.is_some() && !active;
            let c = colour(process).gamma_multiply(if dimmed { 0.35 } else { 1.0 });
            let lit = counts.get(process).copied().unwrap_or(0);
            let text = egui::RichText::new(if lit > 0 { format!("● {process} {lit}") } else { format!("● {process}") })
                .font(theme::small())
                .color(c);
            let response = ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text(meaning(process));
            if response.clicked() {
                view.only = if active { None } else { Some(process.to_string()) };
            }
        }
        ui.label(egui::RichText::new("◯ ring: primed, what it expects next").font(theme::small()).color(Color32::from_gray(170)));
        ui.label(egui::RichText::new("◯ plasticity: synapses just changed").font(theme::small()).color(PLASTIC));
        ui.label(egui::RichText::new("◯ chosen memory").font(theme::small()).color(MEMORY));
        ui.label(
            egui::RichText::new("haze: all synapses, brighter where more run; blue connected, grey still forming")
                .font(theme::small())
                .color(Color32::from_gray(130)),
        );
    });
}

#[cfg(test)]
mod tests {
    use cortex_living::neural::Brain;

    use super::*;

    #[test]
    fn every_cell_is_placed_and_every_synapse_counted() {
        let mut brain = Brain::new();
        for _ in 0..3 {
            brain.learn("<story> the cat sat on the mat .", "test");
        }
        let wiring = brain.wiring();
        let synapses = wiring.synapse_count;
        let cells = wiring.cells.iter().filter(|c| **c != u32::MAX).count();
        let drawn = place(wiring);
        assert_eq!(drawn.alive, cells);
        assert_eq!(drawn.synapses, synapses);
        assert!(drawn.at.iter().flatten().all(|(x, y)| (0.0..=1.0).contains(x) && (0.0..=map::HEIGHT).contains(y)));
        assert!(drawn.image.as_ref().is_some_and(|i| i.pixels.iter().any(|p| p.a() > 0)));
    }

    #[test]
    fn the_map_draws_every_process_lit() {
        let context = egui::Context::default();
        theme::apply(&context);
        let mut brain = Brain::new();
        for _ in 0..4 {
            brain.learn("<story> the cat sat on the mat . the dog sat on the rug .", "test");
        }
        brain.think();
        brain.dream();
        brain.reason("the cat", "mat");
        let mut snapshot = brain.snapshot();
        // As if a moment had passed, so that playback has reached them.
        for activity in &mut snapshot.mind.activity {
            activity.time_ms -= (DELAY_MS + 200.0) as u64;
        }
        let mut view = MapView {
            drawn: Some(place(brain.wiring())),
            fetched: Some(Instant::now()),
            ..MapView::default()
        };
        for only in [None, Some("dream".to_string())] {
            view.only = only;
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| show(ui, &mut view, &snapshot, None, Some(1)));
            });
            output.textures_delta.clear();
        }
        let drawn = view.drawn.as_ref().unwrap();
        assert!(drawn.texture.is_some());
        let think = snapshot.mind.activity.iter().filter(|a| a.process == "think" && !a.primed.is_empty());
        assert!(think.clone().count() > 0);
        assert!(think.map(|a| drawn.carrying(a).len()).sum::<usize>() > 0, "activity is seen running along synapses");
    }
}

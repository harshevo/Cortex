//! The shared brain, drawn as it is. Above, one dot per word or event
//! neuron and one line per synapse, laid out by the synapses themselves;
//! a neuron appears on screen in the moment it is created. Below, the
//! spiking cells of the language learner, each with its dendritic
//! branches drawn as long as their synapses are strong; a cell that is
//! born arrives with a ring, and one that dies flashes as it is replaced.
//! Beside them: what the background service is doing, the teacher, what
//! the brain is curious about, what it has been taught, and how it has
//! grown and been pruned.

use std::collections::{HashMap, HashSet};

use cortex_living::{CellView, Id, Kind, NeuronView, Sample, Snapshot};
use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2, vec2,
};

use crate::theme::{self, palette};

const PANEL_WIDTH: f32 = 310.0;

/// Neurons and synapses drawn at most. The network may hold more; the
/// ones in use are the ones worth seeing.
const NEURONS_DRAWN: usize = 300;
const SYNAPSES_DRAWN: usize = 1_600;

/// How long a new neuron takes to arrive on screen, in seconds.
const ARRIVAL: f64 = 0.9;

/// The layout: neurons push each other apart, synapses pull their ends
/// together, and everything leans towards the middle.
const REPULSION: f32 = 2_600.0;
const SPRING: f32 = 0.018;
const SPRING_LENGTH: f32 = 62.0;
const GRAVITY: f32 = 0.012;
const DAMPING: f32 = 0.82;
const FASTEST: f32 = 14.0;

/// A pointer this close to a neuron is on it.
const REACH: f32 = 12.0;

struct Place {
    at: Vec2,
    speed: Vec2,

    /// When it first came on screen.
    arrived: f64,
}

#[derive(Default)]
pub struct LivingView {
    places: HashMap<Id, Place>,
    selected: Option<Id>,

    /// When each spiking cell was born, by its own count, and when it
    /// came on screen; and when a cell last died.
    cells: HashMap<usize, (u64, f64)>,
    deaths: HashMap<usize, f64>,

    /// Something has been drawn once, so what is new arrives on screen
    /// instead of simply being there.
    shown: bool,
}

/// `remembered` is one line on the agent's own remembered tasks.
pub fn show(ui: &mut Ui, view: &mut LivingView, snapshot: &Snapshot, remembered: &str) {
    let now = ui.input(|input| input.time);

    egui::Panel::right(ui.id().with("living panel"))
        .exact_size(PANEL_WIDTH)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(palette::sidebar())
                .inner_margin(egui::Margin::symmetric(16, 12)),
        )
        .show(ui, |ui| {
            ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| panel(ui, view, snapshot, remembered));
        });

    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
    let whole = response.rect;

    // The spiking cells get a band along the bottom.
    let band = if snapshot.cells.is_empty() {
        0.0
    } else {
        (whole.height() * 0.3).clamp(110.0, 240.0)
    };
    let rect = Rect::from_min_max(whole.min, egui::pos2(whole.max.x, whole.max.y - band));

    if band > 0.0 {
        let cells = Rect::from_min_max(egui::pos2(whole.min.x, rect.max.y), whole.max);

        view.draw_cells(&painter, cells, &snapshot.cells, snapshot.language.spikes, now);
    }

    let drawn = choose(snapshot);
    let ids: HashSet<Id> = drawn.iter().map(|neuron| neuron.id).collect();

    let mut synapses: Vec<(Id, Id, f32)> = snapshot
        .synapses
        .iter()
        .filter(|(from, to, _)| ids.contains(from) && ids.contains(to))
        .copied()
        .collect();

    synapses.sort_by(|left, right| right.2.total_cmp(&left.2));
    synapses.truncate(SYNAPSES_DRAWN);

    view.arrange(&drawn, &synapses, now);

    let scale = view.scale(rect);
    let on_screen = |at: Vec2| rect.center() + at * scale;

    let by_id: HashMap<Id, &NeuronView> = drawn.iter().map(|neuron| (neuron.id, *neuron)).collect();

    for (from, to, weight) in &synapses {
        let (Some(source), Some(target)) = (view.places.get(from), view.places.get(to)) else {
            continue;
        };

        let live = by_id[from].glow > 0.5 && by_id[to].glow > 0.5;

        let stroke = if live {
            Stroke::new(1.0 + weight, palette::accent().gamma_multiply(0.75))
        } else {
            Stroke::new(
                0.6 + weight * 0.8,
                palette::text_muted().gamma_multiply(0.12 + weight * 0.4),
            )
        };

        painter.line_segment([on_screen(source.at), on_screen(target.at)], stroke);
    }

    let pointer = response.hover_pos();
    let mut hovered: Option<(Id, f32)> = None;

    for neuron in &drawn {
        let Some(place) = view.places.get(&neuron.id) else {
            continue;
        };

        let centre = on_screen(place.at);
        let age = ((now - place.arrived) / ARRIVAL).clamp(0.0, 1.0) as f32;

        let radius = (3.0 + (1.0 + neuron.fired as f32).ln().min(6.0)) * ease(age);
        let colour = colour_of(neuron.kind).gamma_multiply(0.4 + 0.6 * neuron.glow);

        // A ring spreads from a neuron as it is created.
        if age < 1.0 {
            painter.circle_stroke(
                centre,
                radius + 22.0 * age,
                Stroke::new(1.5, colour_of(neuron.kind).gamma_multiply(1.0 - age)),
            );
        }

        painter.circle_filled(centre, radius, colour);

        if neuron.in_workspace {
            painter.circle_stroke(centre, radius + 4.5, Stroke::new(1.4, palette::text()));

            label(
                &painter,
                rect,
                centre,
                radius + 8.0,
                &neuron.label,
                palette::text(),
            );
        }

        if view.selected == Some(neuron.id) {
            painter.circle_stroke(centre, radius + 8.0, Stroke::new(1.2, palette::accent()));
        }

        if let Some(pointer) = pointer {
            let distance = pointer.distance(centre);

            if distance <= REACH.max(radius)
                && hovered.is_none_or(|(_, nearest)| distance < nearest)
            {
                hovered = Some((neuron.id, distance));
            }
        }
    }

    if let Some((id, _)) = hovered {
        let neuron = by_id[&id];

        if !neuron.in_workspace {
            let centre = on_screen(view.places[&id].at);

            label(
                &painter,
                rect,
                centre,
                12.0,
                &neuron.label,
                palette::text_secondary(),
            );
        }

        if response.clicked() {
            view.selected = Some(id);
        }
    } else if response.clicked() {
        view.selected = None;
    }

    if drawn.len() < snapshot.neurons.len() {
        painter.text(
            rect.left_bottom() + vec2(14.0, -12.0),
            Align2::LEFT_BOTTOM,
            format!(
                "Showing the {} neurons most in use, of {}",
                drawn.len(),
                snapshot.neurons.len()
            ),
            theme::small(),
            palette::text_muted(),
        );
    }

    view.shown = true;

    // The layout keeps settling, and rest changes the network with
    // nothing else happening.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}

/// The neurons worth drawing: what the workspace holds, what fired
/// lately, then what has fired most.
fn choose(snapshot: &Snapshot) -> Vec<&NeuronView> {
    let mut neurons: Vec<&NeuronView> = snapshot.neurons.iter().collect();

    let worth = |neuron: &NeuronView| {
        f32::from(u8::from(neuron.in_workspace)) * 1_000.0
            + neuron.glow * 100.0
            + (1.0 + neuron.fired as f32).ln()
    };

    neurons.sort_by(|left, right| {
        worth(right)
            .total_cmp(&worth(left))
            .then(left.id.cmp(&right.id))
    });
    neurons.truncate(NEURONS_DRAWN);

    neurons
}

impl LivingView {
    /// Gives new neurons a place and moves every one a step towards
    /// where the synapses would have it.
    fn arrange(&mut self, drawn: &[&NeuronView], synapses: &[(Id, Id, f32)], now: f64) {
        let ids: HashSet<Id> = drawn.iter().map(|neuron| neuron.id).collect();

        self.places.retain(|id, _| ids.contains(id));

        for neuron in drawn {
            if self.places.contains_key(&neuron.id) {
                continue;
            }

            // Beside what it is wired to, when that is already placed.
            let beside = synapses
                .iter()
                .filter_map(
                    |(from, to, _)| match (*from == neuron.id, *to == neuron.id) {
                        (true, _) => self.places.get(to),
                        (_, true) => self.places.get(from),
                        _ => None,
                    },
                )
                .map(|place| place.at)
                .next();

            let angle = (neuron.id as f32 * 2.399_963).rem_euclid(std::f32::consts::TAU);
            let off = vec2(angle.cos(), angle.sin());

            let at = match beside {
                Some(beside) => beside + off * 26.0,
                None => off * (60.0 + (neuron.id % 7) as f32 * 22.0),
            };

            self.places.insert(
                neuron.id,
                Place {
                    at,
                    speed: Vec2::ZERO,
                    // Everything there at the first drawing is simply there.
                    arrived: if self.shown { now } else { f64::NEG_INFINITY },
                },
            );
        }

        let ids: Vec<Id> = self.places.keys().copied().collect();
        let mut push: HashMap<Id, Vec2> = ids.iter().map(|id| (*id, Vec2::ZERO)).collect();

        for (position, one) in ids.iter().enumerate() {
            let here = self.places[one].at;

            if let Some(push) = push.get_mut(one) {
                *push -= here * GRAVITY;
            }

            for other in &ids[position + 1..] {
                let apart = here - self.places[other].at;
                let distance = apart.length().max(4.0);
                let force = apart / distance * (REPULSION / (distance * distance));

                if let Some(push) = push.get_mut(one) {
                    *push += force;
                }

                if let Some(push) = push.get_mut(other) {
                    *push -= force;
                }
            }
        }

        for (from, to, weight) in synapses {
            let (Some(source), Some(target)) = (self.places.get(from), self.places.get(to)) else {
                continue;
            };

            let apart = target.at - source.at;
            let distance = apart.length().max(1.0);
            let pull = apart / distance * (distance - SPRING_LENGTH) * SPRING * (0.4 + weight);

            if let Some(push) = push.get_mut(from) {
                *push += pull;
            }

            if let Some(push) = push.get_mut(to) {
                *push -= pull;
            }
        }

        for (id, place) in &mut self.places {
            place.speed = (place.speed + push[id]) * DAMPING;

            let speed = place.speed.length();

            if speed > FASTEST {
                place.speed *= FASTEST / speed;
            }

            place.at += place.speed;
        }
    }

    /// How much to shrink the layout so that all of it is in `rect`.
    fn scale(&self, rect: Rect) -> f32 {
        let furthest = self
            .places
            .values()
            .map(|place| place.at.length())
            .fold(1.0_f32, f32::max);

        let room = (rect.width().min(rect.height()) / 2.0 - 40.0).max(40.0);

        (room / furthest).min(1.4)
    }
}

impl LivingView {
    /// The spiking cells in a grid, each a soma with its four dendritic
    /// branches. A branch is drawn as long and as bright as its synapses
    /// are strong; a cell glows when it spiked in the last few words.
    fn draw_cells(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        cells: &[CellView],
        spikes: u64,
        now: f64,
    ) {
        let first = self.cells.is_empty();

        for cell in cells {
            match self.cells.get(&cell.id) {
                Some((born, _)) if *born == cell.born => {}
                Some(_) => {
                    // Its place was taken by a newly wired cell.
                    self.deaths.insert(cell.id, now);
                    self.cells.insert(cell.id, (cell.born, now));
                }
                None => {
                    let arrived = if first { f64::NEG_INFINITY } else { now };
                    self.cells.insert(cell.id, (cell.born, arrived));
                }
            }
        }
        self.deaths.retain(|_, when| now - *when < 3.0);

        painter.rect_filled(rect.shrink(6.0), egui::CornerRadius::same(8), palette::sunken());

        let inhibitory = cells.iter().filter(|cell| cell.inhibitory).count();
        painter.text(
            rect.left_top() + vec2(16.0, 12.0),
            Align2::LEFT_TOP,
            format!(
                "Spiking cells: {}  ({} excitatory, {} inhibitory)  ·  {} dendritic branches  ·  {} spikes",
                cells.len(),
                cells.len() - inhibitory,
                inhibitory,
                cells.iter().map(|cell| cell.branches.len()).sum::<usize>(),
                spikes
            ),
            theme::small(),
            palette::text_secondary(),
        );

        let inner = Rect::from_min_max(
            rect.left_top() + vec2(16.0, 34.0),
            rect.right_bottom() - vec2(16.0, 12.0),
        );
        let count = cells.len().max(1) as f32;
        let columns = (count * inner.width() / inner.height().max(1.0)).sqrt().ceil().max(1.0);
        let rows = (count / columns).ceil().max(1.0);
        let step = vec2(inner.width() / columns, inner.height() / rows);
        let reach = step.x.min(step.y);

        for (at, cell) in cells.iter().enumerate() {
            let column = (at as f32 % columns).floor();
            let row = (at as f32 / columns).floor();
            let soma = inner.left_top()
                + vec2((column + 0.5) * step.x, (row + 0.5) * step.y + reach * 0.22);

            let glow = (-(cell.silent as f32) / 40.0).exp();
            let colour = if cell.inhibitory {
                palette::warning()
            } else {
                palette::accent()
            };

            for (branch, weight) in cell.branches.iter().enumerate() {
                let spread = (branch as f32 - (cell.branches.len() as f32 - 1.0) / 2.0) * 0.55;
                let length = reach * (0.12 + 0.42 * weight.clamp(0.0, 1.0));
                let tip = soma + vec2(spread.sin(), -spread.cos()) * length;

                painter.line_segment(
                    [soma, tip],
                    Stroke::new(
                        0.8 + weight,
                        colour.gamma_multiply(0.18 + 0.5 * weight + 0.3 * glow),
                    ),
                );
            }

            let radius = (reach * 0.11).clamp(1.6, 4.5);

            painter.circle_filled(soma, radius, colour.gamma_multiply(0.3 + 0.7 * glow));

            if let Some((_, arrived)) = self.cells.get(&cell.id) {
                let age = ((now - arrived) / ARRIVAL).clamp(0.0, 1.0) as f32;

                if age < 1.0 {
                    painter.circle_stroke(
                        soma,
                        radius + reach * 0.5 * age,
                        Stroke::new(1.5, palette::good().gamma_multiply(1.0 - age)),
                    );
                }
            }

            if let Some(died) = self.deaths.get(&cell.id) {
                let fading = (1.0 - (now - died) / 3.0).clamp(0.0, 1.0) as f32;

                painter.circle_stroke(
                    soma,
                    radius + 3.0,
                    Stroke::new(2.0, palette::bad().gamma_multiply(fading)),
                );
            }
        }
    }
}

/// A label beside a neuron, kept inside the picture.
fn label(painter: &egui::Painter, rect: Rect, centre: Pos2, off: f32, text: &str, colour: Color32) {
    let (anchor, at) = if centre.x > rect.right() - 190.0 {
        (Align2::RIGHT_CENTER, centre - vec2(off, 0.0))
    } else {
        (Align2::LEFT_CENTER, centre + vec2(off, 0.0))
    };

    painter.text(
        at,
        anchor,
        shorten(text, 44),
        FontId::proportional(11.5),
        colour,
    );
}

fn colour_of(kind: Kind) -> Color32 {
    match kind {
        Kind::Sense => palette::accent(),
        Kind::Assembly => palette::warning(),
        Kind::Own => palette::model(),
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Sense => "sense: one thing met in the world",
        Kind::Assembly => "assembly: grown for things that kept firing together",
        Kind::Own => "own: one of the network's own states",
    }
}

fn ease(share: f32) -> f32 {
    1.0 - (1.0 - share).powi(3)
}

fn shorten(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }

    let kept: String = text.chars().take(most - 1).collect();

    format!("{kept}…")
}

fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.0)
}

fn panel(ui: &mut Ui, view: &mut LivingView, snapshot: &Snapshot, remembered: &str) {
    ui.set_width(PANEL_WIDTH - 32.0);

    ui.label(RichText::new("Brain").font(theme::strong(15.0)));

    muted(
        ui,
        "One spiking and dendritic network shared by chat, code and trading. It learns \
         from what is said and done, keeps learning in the background after Cortex closes, \
         and is taught by an LLM teacher. These are measurements of learning mechanisms; \
         they do not show awareness.",
    );

    ui.add_space(8.0);

    let background = &snapshot.background;
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
        let colour = if background.running {
            palette::good()
        } else {
            palette::bad()
        };
        ui.painter().circle_filled(dot.center(), 4.0, colour);
        ui.label(
            RichText::new(if background.running {
                format!("Alive in the background (PID {})", background.pid)
            } else {
                "Background service not reached".to_string()
            })
            .font(theme::small())
            .color(palette::text()),
        );
    });
    if !background.phase.is_empty() {
        muted(ui, &background.phase);
    }
    if background.pending > 0 {
        row(ui, "Experiences waiting", &background.pending.to_string());
    }
    if let Some(problem) = &background.problem {
        muted(ui, problem);
    }

    teacher(ui, &snapshot.teaching);

    heading(ui, "Curious about");
    listed(
        ui,
        &snapshot.curious,
        "Nothing yet. Questions it cannot answer and words it keeps meeting land here.",
    );

    memories(ui, snapshot);
    if !remembered.is_empty() {
        muted(ui, &format!("The coding agent's own memory: {remembered}."));
    }
    growth(ui, snapshot);

    let language = &snapshot.language;
    heading(ui, "Language");
    row(ui, "Vocabulary", &language.vocabulary.to_string());
    row(
        ui,
        "Messages / words heard",
        &format!("{} / {}", language.messages, language.tokens),
    );
    row(
        ui,
        "Quizzes right / wrong",
        &format!("{} / {}", language.quiz_correct, language.quiz_wrong),
    );
    if language.scored > 0 {
        row(
            ui,
            "Next word right",
            &format!(
                "{:.1}%",
                100.0 * language.correct as f64 / language.scored as f64
            ),
        );
        row(
            ui,
            "…by guessing the commonest",
            &format!(
                "{:.1}%",
                100.0 * language.baseline_correct as f64 / language.scored as f64
            ),
        );
    }
    if !language.recent_words.is_empty() {
        muted(
            ui,
            &format!(
                "Last heard: {}",
                language.recent_words.iter().rev().cloned().collect::<Vec<_>>().join(" ")
            ),
        );
    }
    muted(
        ui,
        "It answers from what it was taught and continues text from word associations; it is \
         not yet a fluent language model.",
    );

    heading(ui, "Agent events");

    let state = if snapshot.resting {
        format!(
            "Resting: going over recent moments ({} lived)",
            snapshot.moment
        )
    } else {
        format!("Moment {}", snapshot.moment)
    };

    ui.label(
        RichText::new(state)
            .font(theme::small())
            .color(palette::text_secondary()),
    );

    heading(ui, "Held in the workspace");
    listed(
        ui,
        &snapshot.workspace,
        "Nothing yet. It fills as runs happen.",
    );

    heading(ui, "Expecting next");
    listed(ui, &snapshot.expecting, "Nothing in particular.");

    if let Some(surprise) = snapshot.expected_surprise {
        muted(
            ui,
            &format!("Expects to be {} surprised.", percent(surprise)),
        );
    }

    heading(ui, "Measured");

    let meters = &snapshot.meters;

    row(
        ui,
        "Neurons",
        &format!(
            "{}  (+{} −{})",
            meters.neurons, meters.grown, meters.removed
        ),
    );
    row(
        ui,
        "Synapses",
        &format!(
            "{}  (+{} −{})",
            meters.synapses, meters.connected, meters.disconnected
        ),
    );
    row(ui, "Surprise", &percent(meters.surprise));
    row(ui, "Expected right", &percent(meters.expected_right));
    row(
        ui,
        "…by guessing the common",
        &percent(meters.baseline_right),
    );
    row(ui, "Own surprise foreseen", &percent(meters.self_accuracy));
    row(
        ui,
        "Workspace reaches",
        &format!("{} of the network", percent(meters.reach)),
    );
    row(
        ui,
        "Moments",
        &format!("{} lived, {} replayed", meters.moments, meters.replays),
    );

    if snapshot.history.len() > 1 {
        heading(ui, "Neurons over time");
        spark(
            ui,
            &snapshot.history,
            |sample| sample.neurons as f32,
            palette::accent(),
        );

        heading(ui, "Surprise over time");
        spark(
            ui,
            &snapshot.history,
            |sample| sample.surprise,
            palette::warning(),
        );
    }

    if let Some(neuron) = view
        .selected
        .and_then(|id| snapshot.neurons.iter().find(|neuron| neuron.id == id))
    {
        heading(ui, "Selected neuron");
        chosen(ui, neuron, snapshot);
    } else {
        muted(
            ui,
            "Click a neuron to see what made it and what it is wired to.",
        );
    }

    heading(ui, "Kinds");

    for kind in [Kind::Sense, Kind::Assembly, Kind::Own] {
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());

            ui.painter()
                .circle_filled(dot.center(), 4.0, colour_of(kind));

            muted(ui, kind_name(kind));
        });
    }

    heading(ui, "What changed");

    if snapshot.log.is_empty() {
        muted(ui, "Nothing yet.");
    }

    for line in snapshot.log.iter().rev().take(10) {
        muted(ui, line);
    }
}

/// What the LLM teacher is doing, with its latest rounds.
fn teacher(ui: &mut Ui, teaching: &serde_json::Value) {
    heading(ui, "Teacher");

    if teaching.is_null() {
        muted(ui, "Not reached.");
        return;
    }

    let active = teaching["active"].as_bool().unwrap_or(false);
    let phase = teaching["phase"].as_str().unwrap_or_default();

    if active {
        row(ui, "Teaching", phase);
    } else {
        muted(
            ui,
            "Not teaching. Type /teach snn+dann [topic] in a chat to start; /teach stop ends it; \
             /brain <question> asks the brain itself.",
        );
    }

    if let Some(model) = teaching["model"].as_str() {
        row(ui, "Teacher model", &shorten(model, 30));
    }

    if let Some(topic) = teaching["topic"].as_str() {
        muted(ui, &format!("Topic: {topic}"));
    }

    let rounds = teaching["rounds"].as_u64().unwrap_or(0);

    if rounds > 0 || active {
        row(
            ui,
            "Rounds · right · wrong",
            &format!(
                "{rounds} · {} · {}",
                teaching["correct"].as_u64().unwrap_or(0),
                teaching["wrong"].as_u64().unwrap_or(0)
            ),
        );
    }

    if let Some(problem) = teaching["problem"].as_str() {
        muted(ui, problem);
    }

    let events = teaching["events"].as_array().cloned().unwrap_or_default();

    for event in events.iter().rev().take(6) {
        let kind = event["kind"].as_str().unwrap_or_default();
        let name = match kind {
            "lesson" => "Teacher",
            "quiz" => "Quiz",
            "brain_answer" => "Brain",
            "feedback" => "Judged",
            "curious" => "Brain asks",
            "error" => "Problem",
            other => other,
        };
        let colour = match (kind, event["correct"].as_bool()) {
            ("feedback", Some(true)) => palette::good(),
            ("feedback", Some(false)) | ("error", _) => palette::bad(),
            ("brain_answer" | "curious", _) => palette::accent(),
            _ => palette::text_secondary(),
        };

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).font(theme::strong(11.5)).color(colour));
            if let Some(time) = event["time_ms"].as_u64() {
                ui.label(
                    RichText::new(crate::format::ago(time))
                        .font(theme::small())
                        .color(palette::text_muted()),
                );
            }
        });
        ui.add(
            egui::Label::new(
                RichText::new(shorten(event["text"].as_str().unwrap_or_default(), 220))
                    .font(theme::small())
                    .color(palette::text()),
            )
            .wrap(),
        );
    }
}

/// What it has been taught, each with how strong it is and how it has
/// done in quizzes, and what the agent remembers of its tasks.
fn memories(ui: &mut Ui, snapshot: &Snapshot) {
    heading(ui, "Memories");

    if snapshot.memories.is_empty() {
        muted(ui, "Nothing taught yet.");
    }

    for memory in snapshot.memories.iter().take(12) {
        ui.add_space(3.0);
        ui.label(
            RichText::new(shorten(&memory.question, 60))
                .font(theme::small())
                .color(palette::text()),
        );
        ui.label(
            RichText::new(format!("→ {}", shorten(&memory.answer, 80)))
                .font(theme::small())
                .color(palette::text_secondary()),
        );

        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
        let share = (memory.strength / 4.0).clamp(0.0, 1.0);

        ui.painter()
            .rect_filled(bar, egui::CornerRadius::same(2), palette::sunken());
        ui.painter().rect_filled(
            Rect::from_min_size(bar.min, vec2(bar.width() * share, bar.height())),
            egui::CornerRadius::same(2),
            if memory.misses > memory.hits {
                palette::warning()
            } else {
                palette::accent()
            },
        );

        if memory.hits + memory.misses > 0 {
            muted(
                ui,
                &format!("quizzed right {} · wrong {}", memory.hits, memory.misses),
            );
        }
    }

    if snapshot.memories.len() > 12 {
        muted(ui, &format!("…and {} more.", snapshot.memories.len() - 12));
    }
}

/// How the brain has grown, rewired and been pruned.
fn growth(ui: &mut Ui, snapshot: &Snapshot) {
    let language = &snapshot.language;
    let meters = &snapshot.meters;

    heading(ui, "Growth and plasticity");

    row(
        ui,
        "Spiking cells",
        &format!(
            "{}  (+{} born, −{} died)",
            snapshot.cells.len(),
            language.cells_grown,
            language.cells_died
        ),
    );
    row(
        ui,
        "Spiking synapses",
        &format!(
            "{}  ({} regrown)",
            language.synapses, language.synapses_retracted
        ),
    );
    row(
        ui,
        "Word and event neurons",
        &format!(
            "{}  (+{} −{})",
            meters.neurons, meters.grown, meters.removed
        ),
    );
    row(
        ui,
        "Their synapses",
        &format!(
            "{}  (+{} −{})",
            meters.synapses, meters.connected, meters.disconnected
        ),
    );
    row(
        ui,
        "Dendritic branches",
        &format!(
            "{}  (+{} −{})",
            meters.segments, meters.branched, meters.pruned
        ),
    );
    row(
        ui,
        "Spikes · weight changes",
        &format!("{} · {}", language.spikes, language.updates),
    );
    row(
        ui,
        "Replays · dreams",
        &format!("{} · {}", language.replays + meters.replays, language.dreams),
    );
    row(
        ui,
        "Associations faded",
        &language.associations_faded.to_string(),
    );
    row(ui, "Words forgotten", &language.words_forgotten.to_string());
}

/// What made the selected neuron, and its strongest synapses each way.
fn chosen(ui: &mut Ui, neuron: &NeuronView, snapshot: &Snapshot) {
    ui.label(
        RichText::new(&neuron.label)
            .font(theme::mono())
            .color(palette::text()),
    );

    muted(ui, kind_name(neuron.kind));
    muted(
        ui,
        &format!("Created in moment {}: {}.", neuron.born, neuron.reason),
    );
    muted(
        ui,
        &format!(
            "Fired {} times, last in moment {}.",
            neuron.fired, neuron.last_fired
        ),
    );
    muted(
        ui,
        &format!(
            "After it held the workspace, the next moment was {} surprising on average.",
            percent(neuron.expects_surprise)
        ),
    );

    let name = |id: Id| {
        snapshot
            .neurons
            .iter()
            .find(|other| other.id == id)
            .map_or_else(|| "?".to_string(), |other| shorten(&other.label, 34))
    };

    let mut leads: Vec<(Id, f32)> = snapshot
        .synapses
        .iter()
        .filter(|(from, _, _)| *from == neuron.id)
        .map(|(_, to, weight)| (*to, *weight))
        .collect();

    let mut follows: Vec<(Id, f32)> = snapshot
        .synapses
        .iter()
        .filter(|(_, to, _)| *to == neuron.id)
        .map(|(from, _, weight)| (*from, *weight))
        .collect();

    for (title, synapses) in [("Leads to", &mut leads), ("Follows", &mut follows)] {
        if synapses.is_empty() {
            continue;
        }

        synapses.sort_by(|left, right| right.1.total_cmp(&left.1));

        ui.add_space(4.0);
        ui.label(
            RichText::new(title)
                .font(theme::small())
                .color(palette::text_secondary()),
        );

        for (id, weight) in synapses.iter().take(6) {
            row(ui, &name(*id), &format!("{weight:.2}"));
        }
    }
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).font(theme::strong(12.5)));
    ui.add_space(2.0);
}

fn muted(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .font(theme::small())
                .color(palette::text_muted()),
        )
        .wrap(),
    );
}

fn listed(ui: &mut Ui, labels: &[String], nothing: &str) {
    if labels.is_empty() {
        muted(ui, nothing);
    }

    for label in labels {
        ui.label(
            RichText::new(shorten(label, 40))
                .font(theme::mono())
                .color(palette::text()),
        );
    }
}

fn row(ui: &mut Ui, name: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(name)
                .font(theme::small())
                .color(palette::text_secondary()),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .font(theme::small())
                    .color(palette::text()),
            );
        });
    });
}

/// A small line chart of one meter.
fn spark(ui: &mut Ui, history: &[Sample], reading: impl Fn(&Sample) -> f32, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());

    let readings: Vec<f32> = history.iter().map(reading).collect();

    let lowest = readings.iter().copied().fold(f32::INFINITY, f32::min);
    let highest = readings.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let span = (highest - lowest).max(1e-3);

    let points: Vec<Pos2> = readings
        .iter()
        .enumerate()
        .map(|(position, reading)| {
            let across = position as f32 / (readings.len() - 1).max(1) as f32;
            let up = (reading - lowest) / span;

            egui::pos2(
                rect.left() + across * rect.width(),
                rect.bottom() - 3.0 - up * (rect.height() - 6.0),
            )
        })
        .collect();

    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(4), palette::sunken());

    for pair in points.windows(2) {
        ui.painter()
            .line_segment([pair[0], pair[1]], Stroke::new(1.3, colour));
    }
}

#[cfg(test)]
mod tests {
    use cortex_living::Network;

    use super::*;

    fn lived() -> Network {
        let mut network = Network::new();

        for round in 0..12 {
            for percepts in [
                vec![
                    "does: edit_file".to_string(),
                    format!("file: f{}.rs", round % 3),
                ],
                vec!["does: shell".to_string(), "runs: cargo test".to_string()],
                vec!["result: worked".to_string()],
            ] {
                network.live(&percepts);
            }
        }

        network
    }

    fn draw(context: &egui::Context, view: &mut LivingView, snapshot: &Snapshot) {
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| show(ui, view, snapshot, ""));
        });

        output.textures_delta.clear();
    }

    #[test]
    fn every_neuron_gets_a_place_and_a_new_one_arrives_later() {
        let context = egui::Context::default();
        theme::apply(&context);
        let mut view = LivingView::default();
        let mut network = lived();

        let before = network.snapshot();

        draw(&context, &mut view, &before);
        draw(&context, &mut view, &before);

        assert_eq!(view.places.len(), before.neurons.len());
        assert!(view.places.values().all(|place| place.at.is_finite()));
        assert!(
            view.places
                .values()
                .all(|place| place.arrived == f64::NEG_INFINITY)
        );

        network.live(&["person: rejects".to_string()]);

        let after = network.snapshot();
        let new = after
            .neurons
            .iter()
            .find(|neuron| neuron.label == "person: rejects")
            .unwrap()
            .id;

        draw(&context, &mut view, &after);

        // It was created while the picture was up, so it arrives.
        assert!(view.places[&new].arrived.is_finite());
    }

    #[test]
    fn an_empty_network_draws() {
        let context = egui::Context::default();
        theme::apply(&context);
        let mut view = LivingView::default();

        draw(&context, &mut view, &Snapshot::default());
        draw(&context, &mut view, &Network::new().snapshot());
    }
}

//! The living network, drawn as it is: one dot per neuron it holds and
//! one line per synapse, laid out by the synapses themselves. A neuron
//! appears on screen in the moment it is created.

use std::collections::{HashMap, HashSet};

use cortex_living::{Id, Kind, NeuronView, Sample, Snapshot};
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

    /// Something has been drawn once, so what is new arrives on screen
    /// instead of simply being there.
    shown: bool,
}

pub fn show(ui: &mut Ui, view: &mut LivingView, snapshot: &Snapshot) {
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
                .show(ui, |ui| panel(ui, view, snapshot));
        });

    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
    let rect = response.rect;

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

fn panel(ui: &mut Ui, view: &mut LivingView, snapshot: &Snapshot) {
    ui.set_width(PANEL_WIDTH - 32.0);

    ui.label(RichText::new("Living network").font(theme::strong(15.0)));

    muted(
        ui,
        "Experimental. It watches every run, grows and rewires as it goes, and never advises \
         the model. The figures are measurements of the mechanisms named; none of them is \
         evidence of experience.",
    );

    ui.add_space(8.0);

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
            egui::CentralPanel::default().show(ui, |ui| show(ui, view, snapshot));
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

use cortex_brain::intuition::Picture;
use cortex_brain::spiking::NeuralTrace;
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use serde_json::Value;

#[derive(Default)]
pub struct ActivityView {
    pub inspection: Vec<Value>,
    pub request: Option<Value>,
    query_id: u32,
    query_offset: usize,
    pub trace: NeuralTrace,
    pub performance: String,
    pub intuition: Picture,
    pub active: Vec<Value>,
    pub hunches: Vec<Value>,
    pub values: Vec<f32>,
    pub runs: u32,
    pub step: u32,
    pub confident: bool,
    pub action: String,
    frame: usize,
    selected: usize,
    started: f64,
    playing: bool,
    pub revision: u64,
}

impl ActivityView {
    pub fn observe(&mut self, source: &str, step: u32, detail: &Value, now: f64) {
        if source == "fly-brain" {
            if let Ok(trace) = serde_json::from_value::<NeuralTrace>(detail["trace"].clone())
                && valid_trace(&trace)
            {
                self.trace = trace;
                self.values = serde_json::from_value(detail["values"].clone()).unwrap_or_default();
                self.runs = detail["runs"].as_u64().unwrap_or_default() as u32;
                self.confident = detail["confident"].as_bool().unwrap_or(false);
                self.action = detail["action"].as_str().unwrap_or_default().to_string();
                self.step = step;
                self.frame = 0;
                self.started = now;
                self.playing = true;
                self.revision += 1;
            }
        } else if source == "intuition"
            && let Ok(picture) = serde_json::from_value::<Picture>(detail["picture"].clone())
            && picture.neurons.len() <= 60
            && picture.synapses.len() <= 2880
            && picture.synapses.iter().all(|(a, b, w)| {
                *a < picture.neurons.len() && *b < picture.neurons.len() && w.is_finite()
            })
        {
            self.intuition = picture;
            self.active = detail["active"]
                .as_array()
                .map(|active| active.iter().take(96).cloned().collect())
                .unwrap_or_default();
            self.hunches = detail["hunches"]
                .as_array()
                .map(|hunches| hunches.iter().take(3).cloned().collect())
                .unwrap_or_default();
            self.revision += 1;
        }
    }

    pub fn advance(&mut self, now: f64) -> bool {
        if !self.playing || self.trace.frames.is_empty() {
            return false;
        }
        let next = ((now - self.started).max(0.0) * 12.0) as usize;
        let next = next.min(self.trace.frames.len() - 1);
        let changed = self.frame != next;
        self.frame = next;
        if next + 1 == self.trace.frames.len() {
            self.playing = false;
        }
        changed
    }

    pub fn pause(&mut self) {
        self.playing = false;
        self.frame = self.trace.frames.len().saturating_sub(1);
    }

    pub fn frame(&self) -> usize {
        self.frame
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("Neural activity");
        if !self.performance.is_empty() {
            ui.small(&self.performance);
        }
        if self.trace.frames.is_empty() {
            ui.label("Enable Brain under Capabilities to inspect voltage and spikes.");
        } else {
            ui.label(format!(
                "Step {} · {} neurons · {} connection entries",
                self.step, self.trace.total_neurons, self.trace.total_connections
            ));
            ui.small(format!(
                "{} sampled neurons · {} spikes across the full network · {} ms",
                self.trace.neuron_ids.len(),
                self.trace.total_spikes,
                self.trace.encoding_ms
            ));
            ui.small("Recorded simulation steps; playback is slowed for inspection.");
            ui.horizontal(|ui| {
                if ui
                    .button(if self.playing { "Pause" } else { "Replay" })
                    .clicked()
                {
                    self.playing = !self.playing;
                    self.started = ui.input(|i| i.time)
                        - if self.playing {
                            0.0
                        } else {
                            self.frame as f64 / 12.0
                        };
                    if self.playing {
                        self.frame = 0;
                    }
                }
                let last = self.trace.frames.len() - 1;
                if ui
                    .add(egui::Slider::new(&mut self.frame, 0..=last).text("tick"))
                    .changed()
                {
                    self.playing = false;
                }
            });
            self.network(ui);
            self.voltage(ui);
            egui::CollapsingHeader::new("Situation inputs").show(ui, |ui| {
                for (name, value) in cortex_brain::situation::FEATURE_NAMES
                    .iter()
                    .zip(&self.trace.features)
                {
                    ui.small(format!("{name}: {value:.3}"));
                }
            });
            ui.label(format!(
                "Readout active: {:.1}% · mean spikes/neuron/tick: {:.4}",
                self.trace.readout_active * 100.0,
                self.trace.mean_rate
            ));
            ui.label(format!(
                "{} checked runs · {}",
                self.runs,
                if self.confident {
                    format!("advice: {}", self.action)
                } else {
                    "collecting evidence; no confident advice".into()
                }
            ));
            ui.horizontal_wrapped(|ui| {
                for (action, value) in ["inspect", "edit", "run", "revert", "finish"]
                    .iter()
                    .zip(&self.values)
                {
                    ui.small(format!("{action} {value:.3}"));
                }
            });
        }
        ui.separator();
        if self.trace.total_neurons > 0 {
            ui.small("Inspection reads the current engine; playback shows the recorded ticks.");
            ui.horizontal(|ui| {
                ui.label("Any neuron ID");
                ui.add(
                    egui::DragValue::new(&mut self.query_id)
                        .range(0..=self.trace.total_neurons.saturating_sub(1) as u32),
                );
                if ui.button("Inspect").clicked() {
                    self.query_offset = 0;
                    self.request =
                        Some(serde_json::json!({"neuron_ids":[self.query_id],"offset":0}));
                }
            });
            for state in &self.inspection {
                if let Some(neurons) = state["neurons"].as_array() {
                    for neuron in neurons {
                        ui.small(format!(
                            "Neuron {} · voltage {} · refractory {} · {} outgoing",
                            neuron["id"],
                            neuron["voltage"],
                            neuron["refractory"],
                            neuron["outgoing_total"]
                        ));
                        egui::CollapsingHeader::new("Outgoing synapses (64 per page)").show(
                            ui,
                            |ui| {
                                if let Some(synapses) = neuron["synapses"].as_array() {
                                    for synapse in synapses {
                                        ui.small(format!(
                                            "→ {} · weight {}",
                                            synapse["target"], synapse["weight"]
                                        ));
                                    }
                                }
                            },
                        );
                        ui.horizontal(|ui| {
                            if ui.add_enabled(self.query_offset > 0, egui::Button::new("Previous")).clicked() {
                                self.query_offset = self.query_offset.saturating_sub(64);
                                self.request = Some(serde_json::json!({"neuron_ids":[self.query_id],"offset":self.query_offset}));
                            }
                            if ui.add_enabled(self.query_offset + 64 < neuron["outgoing_total"].as_u64().unwrap_or_default() as usize, egui::Button::new("Next")).clicked() {
                                self.query_offset += 64;
                                self.request = Some(serde_json::json!({"neuron_ids":[self.query_id],"offset":self.query_offset}));
                            }
                        });
                    }
                }
            }
        }
        ui.label(format!(
            "Intuition: {} concepts · {} synapses · {} runs",
            self.intuition.total_neurons, self.intuition.total_synapses, self.intuition.runs
        ));
        ui.small(format!(
            "{} active concepts · {} hunches",
            self.active.len(),
            self.hunches.len()
        ));
        for hunch in self.hunches.iter().take(3) {
            ui.label(format!(
                "{} ({:.2})",
                hunch["text"].as_str().unwrap_or("association"),
                hunch["strength"].as_f64().unwrap_or_default()
            ));
        }
    }

    fn network(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 185.0), Sense::click());
        let painter = ui.painter_at(rect);
        let points: Vec<Pos2> = (0..self.trace.neuron_ids.len())
            .map(|index| {
                let column = match self.trace.populations[index].as_str() {
                    "input" => 0,
                    "readout" => 1,
                    _ => 2,
                };
                let peers: Vec<_> = self
                    .trace
                    .populations
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| **p == self.trace.populations[index])
                    .map(|(i, _)| i)
                    .collect();
                let row = peers.iter().position(|i| *i == index).unwrap_or(0);
                Pos2::new(
                    rect.left() + 25.0 + column as f32 * (rect.width() - 50.0) / 2.0,
                    rect.top()
                        + 8.0
                        + (row as f32 + 0.5) * (rect.height() - 16.0) / peers.len().max(1) as f32,
                )
            })
            .collect();
        let frame = &self.trace.frames[self.frame];
        for &(from, to, strength) in &self.trace.synapses {
            if from >= points.len() || to >= points.len() {
                continue;
            }
            let color = if strength < 0 {
                Color32::from_rgb(160, 65, 90)
            } else {
                Color32::from_rgb(65, 130, 150)
            };
            let width = if frame.spikes[from] { 1.6 } else { 0.4 };
            painter.arrow(
                points[from],
                points[to] - points[from],
                Stroke::new(width, color),
            );
        }
        let mut hovered = None;
        if let Some(pointer) = response.hover_pos() {
            hovered = points
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.distance(pointer).total_cmp(&b.distance(pointer)))
                .filter(|(_, p)| p.distance(pointer) < 9.0)
                .map(|(i, _)| i);
        }
        if response.clicked()
            && let Some(index) = hovered
        {
            self.selected = index;
        }
        for (index, point) in points.iter().enumerate() {
            let color = if frame.spikes[index] {
                Color32::YELLOW
            } else {
                Color32::from_rgb(
                    70,
                    (100.0 + frame.voltage[index].clamp(0.0, 1.0) * 150.0) as u8,
                    220,
                )
            };
            painter.circle_filled(
                *point,
                if self.selected == index { 4.5 } else { 2.5 },
                color,
            );
        }
        if let Some(index) = hovered {
            response.on_hover_text(format!(
                "Neuron {} · {}\nvoltage {:.4} · spike {}",
                self.trace.neuron_ids[index],
                self.trace.populations[index],
                frame.voltage[index],
                frame.spikes[index]
            ));
        }
        ui.small("Input → readout / central · blue excites · red inhibits · yellow fires");
    }

    fn voltage(&mut self, ui: &mut egui::Ui) {
        self.selected = self
            .selected
            .min(self.trace.neuron_ids.len().saturating_sub(1));
        if self.trace.neuron_ids.is_empty() {
            return;
        }
        ui.label(format!(
            "Neuron {} · voltage after reset · threshold {:.1}",
            self.trace.neuron_ids[self.selected], self.trace.threshold
        ));
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 65.0), Sense::hover());
        let point = |tick: usize, value: f32| {
            Pos2::new(
                rect.left()
                    + tick as f32 * rect.width() / (self.trace.frames.len() - 1).max(1) as f32,
                rect.bottom() - ((value + 1.0) / 2.2) * rect.height(),
            )
        };
        let painter = ui.painter_at(rect);
        painter.line_segment(
            [
                point(0, self.trace.threshold),
                point(self.trace.frames.len() - 1, self.trace.threshold),
            ],
            Stroke::new(0.5, Color32::GRAY),
        );
        for tick in 1..self.trace.frames.len() {
            painter.line_segment(
                [
                    point(tick - 1, self.trace.frames[tick - 1].voltage[self.selected]),
                    point(tick, self.trace.frames[tick].voltage[self.selected]),
                ],
                Stroke::new(1.5, Color32::LIGHT_BLUE),
            );
        }
        for (tick, frame) in self.trace.frames.iter().enumerate() {
            if frame.spikes[self.selected] {
                painter.circle_filled(point(tick, self.trace.threshold), 2.5, Color32::YELLOW);
            }
        }
        let x = point(self.frame, 0.0).x;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0, Color32::WHITE),
        );
    }
}

fn valid_trace(trace: &NeuralTrace) -> bool {
    let size = trace.neuron_ids.len();
    size <= 96
        && trace.frames.len() <= 128
        && trace.populations.len() == size
        && trace
            .neuron_ids
            .iter()
            .all(|id| (*id as usize) < trace.total_neurons)
        && trace.frames.iter().all(|frame| {
            frame.voltage.len() == size
                && frame.spikes.len() == size
                && frame.voltage.iter().all(|value| value.is_finite())
        })
        && trace
            .synapses
            .iter()
            .all(|(from, to, _)| *from < size && *to < size)
        && trace.threshold.is_finite()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_brain::{Brain, BrainConfig};

    #[test]
    fn recorded_activity_can_be_replayed_and_scrubbed_without_a_model() {
        let mut brain = Brain::generate(BrainConfig::tiny());
        brain.respond(&[1.0; cortex_brain::situation::FEATURES]);
        let detail = serde_json::json!({"trace":brain.trace(), "runs":2,"values":[0.1,0.2,0.3,0.0,0.4],"action":"run","confident":false});
        let serialized = serde_json::to_string(&detail).unwrap();
        let detail = serde_json::from_str(&serialized).unwrap();
        let mut view = ActivityView::default();
        view.observe("fly-brain", 3, &detail, 10.0);
        assert_eq!(view.step, 3);
        assert_eq!(view.trace.frames.len(), 24);
        assert!(view.advance(11.0));
        assert_eq!(view.frame(), 12);
        view.pause();
        assert_eq!(view.frame(), 23);
        assert!(!view.advance(30.0));
    }

    #[test]
    fn malformed_logged_traces_are_ignored_instead_of_crashing_the_renderer() {
        let mut view = ActivityView::default();
        let trace = serde_json::json!({"trace": {"total_neurons":2,"total_connections":1,"neuron_ids":[0],"populations":[],"synapses":[[0,99,1]],"frames":[{"voltage":[],"spikes":[]}],"total_spikes":0,"mean_rate":0.0,"readout_active":0.0,"features":[],"threshold":1.0,"encoding_ms":0}});
        view.observe("fly-brain", 1, &trace, 0.0);
        assert!(view.trace.frames.is_empty());
    }
}

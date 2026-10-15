//! The brain view: the spiking circuit as it runs, and every number behind
//! it.
//!
//! The brain tab draws every neuron as a dot, grouped by population and
//! brighter the nearer its membrane is to firing. The steps the circuit
//! takes are read from its service as they happen and played back in
//! order, so a neuron flashes when it spikes and the chart below runs with
//! the spikes of each step. Background activity continues between requests
//! until paused through the native controls.
//!
//! Each tab is laid out for a wide desktop window: generous margins, a
//! wide side panel, headline numbers as tiles, and long explanations kept
//! in hover text rather than on the page.

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, RichText, ScrollArea, Sense, Stroke, Ui, vec2,
};

use crate::circuit::{Ask, Circuit, CircuitState, Pulse, Reply};
use crate::theme::{self, palette, size, space};

/// The margin around each tab's page.
const PAGE: f32 = 32.0;

/// The padding inside each section's card.
const CARD_PADDING: f32 = 20.0;

/// Space left under scrolled content, so its end can be scrolled clear
/// of the composer that floats over the bottom of the window.
const COMPOSER_ROOM: f32 = 120.0;

/// The gap between sections.
const GAP: f32 = 20.0;

/// The side panel takes about a third of the window, within these bounds.
const PANEL_MIN: f32 = 400.0;
const PANEL_MAX: f32 = 540.0;

/// Steps played back each second. Each simulated step is a millisecond,
/// so this shows the circuit 25 times slower than it runs, slow enough to
/// see spikes travel. A backlog is played faster.
const PLAY_RATE: f64 = 40.0;

/// How long a spike's flash takes to fade, in seconds.
const FLASH: f64 = 0.5;

/// How many steps the activity chart spans.
const HISTORY: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Brain,
    Metrics,
}

#[derive(Default)]
pub struct BrainView {
    pub tab: Tab,

    /// The brain's service, and what was said to it.
    circuit: Circuit,
    text: String,
    asking: Option<Receiver<Result<Reply, String>>>,
    reply: Option<Result<Reply, String>>,
    lesson: String,
    program: String,

    /// The playback: the latest step played, when each neuron last
    /// spiked, the spikes of each recent step by population, and steps
    /// owed to the playback since the last frame.
    played: u64,
    flashed: Vec<f64>,
    history: VecDeque<Vec<u16>>,
    owed: f64,
    drawn_at: f64,
}

impl BrainView {
    /// A view that starts on `tab`.
    pub fn on(tab: Tab) -> Self {
        Self {
            tab,
            ..Self::default()
        }
    }

    fn ask(&mut self, ask: Ask) {
        self.reply = None;
        self.asking = Some(self.circuit.ask(ask));
    }

    /// Plays the steps the circuit has taken since the last frame, as many
    /// as the time since allows. Returns how many are still waiting.
    fn play(&mut self, now: f64, pulse: &Pulse) -> usize {
        let elapsed = (now - self.drawn_at).clamp(0.0, 0.1);
        self.drawn_at = now;
        let neurons = pulse.voltages.len();
        if self.flashed.len() != neurons {
            self.flashed = vec![f64::NEG_INFINITY; neurons];
        }

        if pulse.frame < self.played {
            self.played = 0;
            self.history.clear();
            self.flashed.fill(f64::NEG_INFINITY);
            self.owed = 0.0;
        }
        let frames = self.circuit.frames_after(self.played);

        // What happened before the view opened is not replayed.
        if self.played == 0 {
            self.played = frames.last().map_or(pulse.frame, |f| f.0);
            return 0;
        }

        let waiting = frames.len();
        if waiting == 0 {
            self.owed = 0.0;
            return 0;
        }
        self.owed += elapsed * PLAY_RATE * (1.0 + waiting as f64 / 200.0);
        let count = (self.owed.floor() as usize).min(waiting);
        self.owed -= count as f64;

        let groups = pulse.populations.len().max(1);
        for (number, spikes) in frames.into_iter().take(count) {
            let mut counts = vec![0u16; groups];
            for &neuron in &spikes {
                let neuron = neuron as usize;
                if let Some(at) = self.flashed.get_mut(neuron) {
                    *at = now;
                }
                if let Some(&group) = pulse.population.get(neuron) {
                    counts[(group as usize).min(groups - 1)] += 1;
                }
            }
            self.history.push_back(counts);
            if self.history.len() > HISTORY {
                self.history.pop_front();
            }
            self.played = number;
        }

        waiting - count
    }
}

// ---------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------

/// Rounder controls for the brain's pages: fields and buttons as soft
/// shapes, to match the cards around them.
fn soften(ui: &mut Ui) {
    let radius = CornerRadius::same(theme::radius::XL);
    let widgets = &mut ui.visuals_mut().widgets;
    for widget in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.corner_radius = radius;
    }
    ui.spacing_mut().button_padding = vec2(space::L, space::S - 1.0);
    ui.spacing_mut().item_spacing = vec2(space::S, space::S);
}

/// The tab strip: a segmented control on a soft track. Returns the tab to
/// show.
pub fn tabs(ui: &mut Ui, view: &mut BrainView) -> Tab {
    ui.add_space(space::XL);
    ui.horizontal(|ui| {
        ui.add_space(PAGE);
        let names = [(Tab::Brain, "Brain"), (Tab::Metrics, "Metrics")];
        let item = vec2(96.0, 32.0);
        let (track, _) = ui.allocate_exact_size(
            vec2(item.x * names.len() as f32 + 8.0, item.y + 8.0),
            Sense::hover(),
        );
        ui.painter().add(theme::squircle(
            track,
            16.0,
            palette::surface(),
            Stroke::NONE,
        ));
        for (at, (tab, name)) in names.into_iter().enumerate() {
            let rect = Rect::from_min_size(track.min + vec2(4.0 + at as f32 * item.x, 4.0), item);
            let response = ui.interact(rect, ui.id().with(("brain tab", at)), Sense::click());
            let chosen = view.tab == tab;
            if chosen {
                ui.painter()
                    .add(theme::squircle(rect, 12.0, palette::raised(), Stroke::NONE));
            } else if response.hovered() {
                ui.painter().add(theme::squircle(
                    rect,
                    12.0,
                    palette::raised().gamma_multiply(0.5),
                    Stroke::NONE,
                ));
            }
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                name,
                if chosen {
                    theme::strong(size::UI)
                } else {
                    theme::ui()
                },
                if chosen {
                    palette::text()
                } else {
                    palette::text_secondary()
                },
            );
            if response.clicked() {
                view.tab = tab;
            }
        }
    });
    ui.add_space(space::L);
    view.tab
}

/// A section: a soft card with a title, and the title's hover text when
/// there is more to say than the content shows.
fn section<R>(
    ui: &mut Ui,
    title: &str,
    hint: Option<&str>,
    content: impl FnOnce(&mut Ui) -> R,
) -> R {
    let inner = theme::soft_card(ui, palette::surface(), CARD_PADDING, |ui| {
        let heading = ui.label(RichText::new(title).font(theme::strong(size::TITLE)));
        if let Some(hint) = hint {
            heading.on_hover_text(hint);
        }
        ui.add_space(space::S);
        content(ui)
    })
    .inner;
    ui.add_space(GAP);
    inner
}

/// A headline number: a large figure over a short label.
fn tile(ui: &mut Ui, label: &str, value: &str) {
    theme::soft_card(ui, palette::surface(), CARD_PADDING, |ui| {
        ui.label(
            RichText::new(value)
                .font(theme::strong(size::DISPLAY))
                .color(palette::text()),
        );
        ui.add_space(space::XXS);
        ui.label(
            RichText::new(label)
                .font(theme::small())
                .color(palette::text_muted()),
        );
    });
}

/// A row of headline numbers, as many to a line as the width allows.
fn tiles(ui: &mut Ui, items: &[(&str, String)]) {
    let per_line = ((ui.available_width() / 170.0) as usize).clamp(1, items.len().max(1));
    for line in items.chunks(per_line) {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = space::L;
            ui.columns(per_line, |columns| {
                for (column, (label, value)) in columns.iter_mut().zip(line) {
                    tile(column, label, value);
                }
            });
        });
        ui.add_space(space::L);
    }
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

fn plain(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).font(theme::ui()).color(palette::text())).wrap());
}

/// A name and its value on one line, the value to the right.
fn row(ui: &mut Ui, name: &str, value: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.label(
            RichText::new(name)
                .font(theme::ui())
                .color(palette::text_secondary()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .font(theme::ui())
                    .color(palette::text()),
            );
        });
    })
    .response
}

/// A small coloured word, like a tag.
fn tag(ui: &mut Ui, text: &str, colour: Color32) {
    ui.label(
        RichText::new(text)
            .font(theme::strong(size::CAPTION))
            .color(colour),
    );
}

/// A coloured dot and its name, for legends.
fn key(ui: &mut Ui, colour: Color32, name: &str) {
    dot(ui, colour);
    ui.label(
        RichText::new(name)
            .font(theme::small())
            .color(palette::text_secondary()),
    );
    ui.add_space(space::S);
}

fn dot(ui: &mut Ui, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 18.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, colour);
}

fn primary(label: &str) -> egui::Button<'static> {
    theme::primary_button(label)
        .corner_radius(CornerRadius::same(theme::radius::XL))
        .min_size(vec2(0.0, theme::control::LARGE))
}

fn secondary(label: &str) -> egui::Button<'static> {
    theme::secondary_button(label)
        .corner_radius(CornerRadius::same(theme::radius::XL))
        .min_size(vec2(0.0, theme::control::LARGE))
}

fn field<'a>(text: &'a mut String, hint: &str) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .hint_text(hint)
        .font(theme::ui())
        .margin(egui::Margin::symmetric(12, 8))
        .desired_width(f32::INFINITY)
}

fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.0)
}

pub(crate) fn count(number: impl Into<u64>) -> String {
    let number = number.into();
    let digits = number.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A large count, shortened: 100,352 as 100k.
fn short(number: u64) -> String {
    match number {
        n if n >= 10_000_000 => format!("{:.0}M", n as f64 / 1e6),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 10_000 => format!("{:.0}k", n as f64 / 1e3),
        n => count(n),
    }
}

/// The colour of each neuron population.
fn population_colour(name: &str) -> Color32 {
    match name {
        "Context" => Color32::from_rgb(34, 211, 238),
        "Association" => Color32::from_rgb(59, 130, 246),
        "Episodic" => Color32::from_rgb(168, 85, 247),
        "Value" => Color32::from_rgb(234, 179, 8),
        "Action" => Color32::from_rgb(34, 197, 94),
        "Inhibitory" => Color32::from_rgb(239, 68, 68),
        _ => Color32::from_rgb(148, 163, 184),
    }
}

/// The right-hand panel of a tab, scrolled on its own.
fn side_panel(ui: &mut Ui, id: &str, content: impl FnOnce(&mut Ui)) {
    let width = (ui.available_width() * 0.36).clamp(PANEL_MIN, PANEL_MAX);
    egui::Panel::right(ui.id().with(id))
        .exact_size(width)
        .show_separator_line(false)
        .frame(egui::Frame::new().inner_margin(egui::Margin {
            left: 0,
            right: PAGE as i8,
            top: 0,
            bottom: 0,
        }))
        .show(ui, |ui| {
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                soften(ui);
                content(ui);
                ui.add_space(COMPOSER_ROOM);
            });
        });
}

/// Takes in a reply to a question once it has come.
fn receive(view: &mut BrainView) {
    if let Some(asking) = &view.asking
        && let Ok(reply) = asking.try_recv()
    {
        view.reply = Some(reply);
        view.asking = None;
    }
}

/// Shown until the brain's service answers.
fn unreachable(ui: &mut Ui, view: &BrainView, problem: Option<&str>) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(PAGE as i8, 0))
        .show(ui, |ui| {
            soften(ui);
            if !view.circuit.has_read() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Reaching the brain…").color(palette::text_secondary()));
                });
                return;
            }
            ui.allocate_ui(vec2(ui.available_width().min(620.0), 0.0), |ui| {
                section(ui, "Brain not running", None, |ui| {
                    muted(ui, problem.unwrap_or("Its service does not answer."));
                    ui.add_space(space::S);
                    theme::soft_card(ui, palette::sunken(), 14.0, |ui| {
                        ui.label(
                            RichText::new(
                                "launchctl kickstart gui/$(id -u)/dev.cortex.brain-circuit",
                            )
                            .font(theme::mono())
                            .color(palette::text()),
                        );
                    });
                });
            });
        });
}

// ---------------------------------------------------------------------
// The brain tab
// ---------------------------------------------------------------------

/// The brain tab: the circuit as it runs.
pub fn show_brain(ui: &mut Ui, view: &mut BrainView) {
    view.circuit.start(ui.ctx());
    receive(view);

    let (state, pulse, problem) = view.circuit.state();
    let Some(pulse) = pulse else {
        unreachable(ui, view, problem.as_deref());
        return;
    };

    let now = ui.input(|input| input.time);
    let waiting = view.play(now, &pulse);
    let running =
        waiting > 0 || view.asking.is_some() || view.flashed.iter().any(|t| now - t < FLASH);

    side_panel(ui, "brain panel", |ui| {
        brain_panel(ui, view, state.as_ref())
    });

    egui::Frame::new()
        .inner_margin(egui::Margin { left: PAGE as i8, right: GAP as i8, top: 0, bottom: PAGE as i8 })
        .show(ui, |ui| {
            soften(ui);
            let recent: Vec<u64> =
                view.history.iter().rev().take(50).map(|c| c.iter().map(|&n| n as u64).sum()).collect();
            let per_step = recent.iter().sum::<u64>() as f32 / recent.len().max(1) as f32;
            tiles(
                ui,
                &[
                    ("Neurons", short(pulse.voltages.len() as u64)),
                    ("Synapses", state.as_ref().map_or("–".into(), |s| short(s.synapses))),
                    ("Dendrites", short(pulse.activity.dendrites)),
                    ("Learned params", state.as_ref().map_or("–".into(), |s| short(s.learned_decoder_parameters))),
                ],
            );

            theme::soft_card(ui, palette::surface(), CARD_PADDING, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Neurons").font(theme::strong(size::TITLE)));
                    ui.add_space(space::S);
                    let (colour, text) = match (&problem, running) {
                        (Some(_), _) => (palette::warning(), "Unreachable"),
                        (None, _) if pulse.background.enabled => (palette::good(), "Active"),
                        (None, true) => (palette::good(), "Working"),
                        (None, false) => (palette::text_muted(), "Paused"),
                    };
                    dot(ui, colour);
                    let label = ui.label(RichText::new(text).font(theme::small()).color(palette::text_secondary()));
                    if let Some(problem) = &problem {
                        label.on_hover_text(problem);
                    } else if !running {
                        label.on_hover_text("Background activity is paused. Conversation and learning remain available.");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("step {}", count(view.played)))
                                .font(theme::mono())
                                .color(palette::text_muted()),
                        );
                    });
                });
                ui.add_space(space::M);
                let chart = 96.0;
                let height = (ui.available_height() - chart - 170.0).max(120.0);
                neuron_field(ui, view, &pulse, now, height);
                ui.add_space(space::L);
                activity_chart(ui, view, &pulse, chart);
                ui.add_space(space::S);
                ui.label(RichText::new(format!("{per_step:.1} spikes/step · {} active dendritic plateaus", count(pulse.activity.active_branches)))
                    .font(theme::small()).color(palette::text_secondary()));
                ui.add_space(space::S);
                dendrites(ui, &pulse);
                ui.add_space(space::M);
                ui.horizontal_wrapped(|ui| {
                    for name in &pulse.populations {
                        key(ui, population_colour(name), name);
                    }
                });
            });
        });

    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(if running {
            16
        } else {
            100
        }));
}

fn dendrites(ui: &mut Ui, pulse: &Pulse) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Dendrites").font(theme::strong(size::UI)))
            .on_hover_text("Variable simulated branches per excitatory neuron. Replay can grow and prune branches. Values summarize each slot across cells that have that branch.");
        ui.label(RichText::new("mean voltage · active plateaus").font(theme::small()).color(palette::text_muted()));
    });
    for start in (0..pulse.activity.branch_voltage.len()).step_by(4) {
        ui.columns(
            (pulse.activity.branch_voltage.len() - start).min(4),
            |columns| {
                for (offset, column) in columns.iter_mut().enumerate() {
                    let index = start + offset;
                    let voltage = pulse.activity.branch_voltage[index];
                    column.label(
                        RichText::new(format!("Branch {}", index + 1))
                            .font(theme::small())
                            .color(palette::text_secondary()),
                    );
                    column.label(
                        RichText::new(format!(
                            "{voltage:.2} · {}",
                            count(pulse.activity.branch_plateaus[index])
                        ))
                        .font(theme::mono()),
                    );
                }
            },
        );
    }
}

/// Every neuron as a dot, grouped by population: dim at rest, brighter
/// nearer firing, and flashing when it spikes.
fn neuron_field(ui: &mut Ui, view: &BrainView, pulse: &Pulse, now: f64, height: f32) {
    let (response, painter) =
        ui.allocate_painter(vec2(ui.available_width(), height), Sense::hover());
    let area = response.rect;
    let total = pulse.voltages.len().max(1);

    // The pitch that fits every neuron in the area.
    let pitch = ((area.width() * area.height()) / total as f32)
        .sqrt()
        .clamp(3.0, 28.0);
    let across = ((area.width() / pitch) as usize).max(1);
    let pitch = pitch.min(area.width() / across as f32);
    let colours: Vec<Color32> = pulse
        .populations
        .iter()
        .map(|p| population_colour(p))
        .collect();

    let mut order: Vec<usize> = (0..pulse.voltages.len()).collect();
    order.sort_by_key(|&i| pulse.population.get(i).copied().unwrap_or(0));

    let pointer = response.hover_pos();
    let mut hovered = None;
    for (at, &index) in order.iter().enumerate() {
        let group = pulse.population.get(index).copied().unwrap_or(0) as usize;
        let voltage = pulse.voltages[index];
        let centre = area.left_top()
            + vec2(
                (at % across) as f32 * pitch + pitch / 2.0,
                (at / across) as f32 * pitch + pitch / 2.0,
            );
        if centre.y > area.bottom() {
            break;
        }
        // Rest is near zero, threshold near one.
        let level = ((voltage + 0.3) / 1.3).clamp(0.0, 1.0);
        let flash = view
            .flashed
            .get(index)
            .map_or(0.0, |t| (-(now - t) / FLASH * 3.0).exp() as f32);
        let colour = colours.get(group).copied().unwrap_or(palette::text_muted());
        let resting = colour.gamma_multiply(0.10 + 0.45 * level);
        let lit = Color32::WHITE.lerp_to_gamma(colour, 0.35);
        painter.circle_filled(
            centre,
            pitch * (0.30 + 0.14 * flash),
            resting.lerp_to_gamma(lit, flash),
        );
        if pointer.is_some_and(|p| p.distance(centre) < pitch / 2.0) {
            hovered = Some((index, group, voltage));
        }
    }

    if let Some((index, group, voltage)) = hovered {
        let name = pulse
            .populations
            .get(group)
            .map(String::as_str)
            .unwrap_or("");
        response.on_hover_text(format!("Neuron {index} · {name} · voltage {voltage:.2}"));
    }
}

/// Spikes on each recent step, stacked by population: the circuit's
/// activity running from left to right.
fn activity_chart(ui: &mut Ui, view: &BrainView, pulse: &Pulse, height: f32) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        CornerRadius::same(theme::radius::XL),
        palette::sunken(),
    );
    let inner = rect.shrink(8.0);
    let highest = view
        .history
        .iter()
        .map(|c| c.iter().map(|&n| n as u32).sum::<u32>())
        .max()
        .unwrap_or(0)
        .max(8) as f32;
    let slot = inner.width() / HISTORY as f32;
    let start = HISTORY - view.history.len();
    for (at, counts) in view.history.iter().enumerate() {
        let x = inner.left() + (start + at) as f32 * slot;
        let mut bottom = inner.bottom();
        for (group, &n) in counts.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let tall = inner.height() * n as f32 / highest;
            let colour = pulse
                .populations
                .get(group)
                .map_or(palette::text_muted(), |p| population_colour(p));
            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(x, bottom - tall),
                    egui::pos2(x + slot.max(1.0), bottom),
                ),
                CornerRadius::ZERO,
                colour.gamma_multiply(0.85),
            );
            bottom -= tall;
        }
    }
    if view.history.is_empty() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Spikes per step appear here",
            theme::small(),
            palette::text_muted(),
        );
    }
    response.on_hover_text(format!(
        "Spikes on each of the last {HISTORY} steps, by population."
    ));
}

fn brain_panel(ui: &mut Ui, view: &mut BrainView, state: Option<&CircuitState>) {
    let busy = view.asking.is_some();
    let english = state.is_some_and(|s| s.language_tokens.is_some());

    let (_, pulse, problem) = view.circuit.state();
    section(
        ui,
        "Live brain",
        Some("Activity from the running circuit"),
        |ui| {
            if let Some(pulse) = pulse {
                ui.horizontal(|ui| {
                    let enabled = pulse.background.enabled;
                    dot(
                        ui,
                        if enabled {
                            palette::good()
                        } else {
                            palette::text_muted()
                        },
                    );
                    ui.label(if enabled { "Running" } else { "Paused" });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !busy && problem.is_none(),
                                secondary(if enabled { "Pause" } else { "Resume" }),
                            )
                            .clicked()
                        {
                            view.ask(Ask::Activity(!enabled));
                        }
                    });
                });
                row(ui, "Phase", &pulse.background.phase);
                row(ui, "Background cycles", &count(pulse.background.cycles));
                if let Some(state) = state {
                    row(ui, "Structure", &state.topology.structure);
                    row(
                        ui,
                        "Growth accepted",
                        &count(state.adaptation["accepted"].as_u64().unwrap_or(0)),
                    );
                    row(
                        ui,
                        "Neurons added",
                        &count(state.adaptation["neurons_added"].as_u64().unwrap_or(0)),
                    );
                    row(
                        ui,
                        "Core previously trained",
                        if state.core_trained {
                            "Yes"
                        } else {
                            "Not recorded"
                        },
                    );
                }
                if let Some(error) = pulse.background.error {
                    muted(ui, &error);
                }
            }
            if let Some(problem) = problem {
                muted(ui, &problem);
            }
        },
    );

    section(
        ui,
        "Talk",
        Some("Send generates learned tokens. Math tool uses an explicit arithmetic executor."),
        |ui| {
            let edit = ui.add(field(&mut view.text, "Message the brain…"));
            ui.add_space(space::XS);
            let ready = !busy && !view.text.trim().is_empty();
            ui.horizontal(|ui| {
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.add_enabled(ready, primary("Send")).clicked() || enter) && ready {
                    let text = view.text.trim().to_string();
                    view.ask(if english {
                        Ask::English(text)
                    } else {
                        Ask::Chat(text)
                    });
                }
                if english && ui.add_enabled(ready, secondary("Math tool")).clicked() {
                    let text = view.text.trim().to_string();
                    view.ask(Ask::Chat(text));
                }
                if busy {
                    ui.spinner();
                }
            });

            if let Some(reply) = &view.reply {
                ui.add_space(space::S);
                theme::soft_card(ui, palette::raised(), 14.0, |ui| match reply {
                    Ok(Reply::Answer(answer)) => {
                        ui.horizontal_wrapped(|ui| {
                            tag(ui, &answer.program, palette::accent());
                            tag(
                                ui,
                                if answer.abstained {
                                    "Abstained"
                                } else {
                                    "Answered"
                                },
                                if answer.abstained {
                                    palette::warning()
                                } else {
                                    palette::good()
                                },
                            );
                            ui.label(
                                RichText::new(percent(answer.confidence))
                                    .font(theme::small())
                                    .color(palette::text_muted()),
                            );
                        });
                        plain(ui, &answer.text);
                        if !answer.evidence.is_empty() {
                            egui::CollapsingHeader::new(
                                RichText::new("Evidence").font(theme::small()),
                            )
                            .id_salt("brain evidence")
                            .show(ui, |ui| {
                                for line in &answer.evidence {
                                    muted(ui, line);
                                }
                                if answer.candidates.len() > 1 {
                                    for (program, score) in &answer.candidates {
                                        row(ui, program, &percent(*score));
                                    }
                                }
                            });
                        }
                    }
                    Ok(Reply::English {
                        text, note, method, ..
                    }) => {
                        tag(
                            ui,
                            if method == "explicit_instruction_executor" {
                                "Instruction"
                            } else if method.contains("self_monitor") {
                                "Self-monitor"
                            } else {
                                "Neural English"
                            },
                            palette::accent(),
                        );
                        plain(ui, text);
                        ui.label(
                            RichText::new(if method.contains("self_monitor") {
                                "Experimental state estimate"
                            } else if method == "explicit_instruction_executor" {
                                "Executed instruction"
                            } else {
                                "Not checked for meaning"
                            })
                            .font(theme::small())
                            .color(palette::text_muted()),
                        )
                        .on_hover_text(note);
                    }
                    Ok(Reply::Done(text)) => plain(ui, text),
                    Err(error) => muted(ui, error),
                });
                if matches!(
                    reply,
                    Ok(Reply::Answer(_))
                        | Ok(Reply::English {
                            feedback_supported: true,
                            ..
                        })
                ) {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!busy, secondary("Helpful")).clicked() {
                            view.ask(Ask::Feedback(1.0));
                        }
                        if ui.add_enabled(!busy, secondary("Incorrect")).clicked() {
                            view.ask(Ask::Feedback(-1.0));
                        }
                    });
                }
            }
        },
    );

    let Some(state) = state else {
        return;
    };

    section(
        ui,
        "Teach",
        Some("Pairs a question with the operation that answers it, then saves the brain."),
        |ui| {
            ui.add(field(&mut view.lesson, "Question"));
            if view.program.is_empty() || !state.programs.contains(&view.program) {
                view.program = state.programs.first().cloned().unwrap_or_default();
            }
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("brain program")
                    .selected_text(RichText::new(&view.program).font(theme::ui()))
                    .width((ui.available_width() - 190.0).max(120.0))
                    .show_ui(ui, |ui| {
                        for program in &state.programs {
                            ui.selectable_value(&mut view.program, program.clone(), program);
                        }
                    });
                let ready = !busy && !view.lesson.trim().is_empty() && !view.program.is_empty();
                if ui.add_enabled(ready, primary("Teach")).clicked() {
                    let text = std::mem::take(&mut view.lesson);
                    let program = view.program.clone();
                    view.ask(Ask::Teach { text, program });
                }
                if ui
                    .add_enabled(!busy, secondary("Replay"))
                    .on_hover_text("Replay the questions it was taught")
                    .clicked()
                {
                    view.ask(Ask::Replay);
                }
            });
        },
    );

    if !state.curiosity.is_empty() {
        section(
            ui,
            "Couldn't answer",
            Some("Recent questions it abstained on: candidates to teach."),
            |ui| {
                for question in &state.curiosity {
                    let response = ui
                        .add(
                            egui::Label::new(
                                RichText::new(crate::format::one_line(question, 80))
                                    .font(theme::ui()),
                            )
                            .sense(Sense::click())
                            .wrap(),
                        )
                        .on_hover_text("Click to teach");
                    if response.clicked() {
                        view.lesson = question.clone();
                    }
                    ui.add_space(space::XXS);
                }
            },
        );
    }

    if !state.observations.is_empty() {
        section(ui, "Recent questions", None, |ui| {
            observations(ui, state, 8)
        });
    }
}

/// The latest questions and how it answered them.
fn observations(ui: &mut Ui, state: &CircuitState, most: usize) {
    for observation in state.observations.iter().rev().take(most) {
        muted(ui, &crate::format::one_line(&observation.question, 90));
        ui.horizontal_wrapped(|ui| {
            let answer = &observation.response;
            tag(
                ui,
                if answer.abstained {
                    "—"
                } else {
                    &answer.program
                },
                if answer.abstained {
                    palette::text_muted()
                } else {
                    palette::accent()
                },
            );
            ui.label(
                RichText::new(crate::format::one_line(&answer.text, 80))
                    .font(theme::ui())
                    .color(palette::text()),
            );
        });
        ui.add_space(space::S);
    }
}

// ---------------------------------------------------------------------
// The metrics tab
// ---------------------------------------------------------------------

/// The metrics tab: headline numbers, then every counter by group.
pub fn show_metrics(ui: &mut Ui, view: &mut BrainView) {
    view.circuit.start(ui.ctx());
    receive(view);

    let (state, pulse, problem) = view.circuit.state();
    let (Some(state), Some(pulse)) = (state, pulse) else {
        unreachable(ui, view, problem.as_deref());
        return;
    };

    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: PAGE as i8,
                right: PAGE as i8,
                top: 0,
                bottom: PAGE as i8,
            })
            .show(ui, |ui| {
                soften(ui);
                metrics(ui, &state, &pulse);
                ui.add_space(COMPOSER_ROOM);
            });
    });
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(500));
}

fn metrics(ui: &mut Ui, state: &CircuitState, pulse: &Pulse) {
    let activity = &pulse.activity;
    tiles(
        ui,
        &[
            ("Neurons", short(pulse.voltages.len() as u64)),
            ("Synapses", short(state.synapses)),
            ("Dendrites", short(pulse.activity.dendrites)),
            ("Updates", short(state.updates)),
            ("Programs", short(state.programs.len() as u64)),
            ("Steps run", short(pulse.frame)),
        ],
    );
    ui.add_space(space::S);

    let answered = state
        .observations
        .iter()
        .filter(|o| !o.response.abstained)
        .count();
    let mut groups: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "Activity",
            vec![
                ("Active neurons".into(), count(activity.active_neurons)),
                (
                    "Active dendritic plateaus".into(),
                    count(activity.active_branches),
                ),
                ("Spikes, last step".into(), count(activity.spikes)),
                (
                    "Mean · max voltage".into(),
                    format!("{:.2} · {:.2}", activity.mean_voltage, activity.max_voltage),
                ),
                ("Pending events".into(), count(activity.pending_events)),
                ("Dropped events".into(), count(activity.dropped_events)),
            ],
        ),
        (
            "Learning",
            vec![
                ("Updates".into(), count(state.updates)),
                ("Readout weights".into(), count(state.plastic_weights)),
                (
                    "Learned decoder parameters".into(),
                    count(state.learned_decoder_parameters),
                ),
                (
                    "Core previously trained".into(),
                    if state.core_trained {
                        "Yes"
                    } else {
                        "Not recorded"
                    }
                    .into(),
                ),
                (
                    "Checkpoint training steps".into(),
                    count(state.training_steps),
                ),
                (
                    "Online learning".into(),
                    state.topology.online_learning.clone(),
                ),
                (
                    "Acceptance threshold".into(),
                    format!("{:.2}", state.threshold),
                ),
                (
                    "Language tokens".into(),
                    state.language_tokens.map_or("–".into(), count),
                ),
                (
                    "Last training phase".into(),
                    state.method.clone().unwrap_or("–".into()),
                ),
                ("Device".into(), state.device.clone().unwrap_or("–".into())),
            ],
        ),
        (
            "Questions",
            vec![
                ("Asked".into(), count(state.observations.len() as u64)),
                ("Answered".into(), count(answered as u64)),
                (
                    "Abstained".into(),
                    count((state.observations.len() - answered) as u64),
                ),
                (
                    "Waiting to be taught".into(),
                    count(state.curiosity.len() as u64),
                ),
            ],
        ),
    ];

    groups.insert(
        0,
        (
            "Structure",
            vec![
                ("Loaded neurons".into(), count(state.topology.neurons)),
                (
                    "Recurrent synapses".into(),
                    count(state.topology.recurrent_synapses),
                ),
                (
                    "Input synapses".into(),
                    count(state.topology.input_synapses),
                ),
                ("Dendritic branches".into(), count(state.topology.dendrites)),
                (
                    "Branches / excitatory cell".into(),
                    format!(
                        "{}–{}",
                        state.topology.branches_min, state.topology.branches_max
                    ),
                ),
                (
                    "Incoming recurrent / input".into(),
                    format!(
                        "{} / {}",
                        state.topology.recurrent_fan_in, state.topology.input_fan_in
                    ),
                ),
                ("Structure".into(), state.topology.structure.clone()),
                ("Growth".into(), state.topology.growth.clone()),
            ],
        ),
    );
    groups.insert(
        1,
        (
            "Dendrites",
            (0..activity.branch_voltage.len())
                .flat_map(|index| {
                    [
                        (
                            format!("Branch {} mean voltage", index + 1),
                            format!("{:.2}", activity.branch_voltage[index]),
                        ),
                        (
                            format!("Branch {} plateaus", index + 1),
                            count(activity.branch_plateaus[index]),
                        ),
                        (
                            format!("Branch {} mean gain", index + 1),
                            format!("{:.2}", activity.branch_gain[index]),
                        ),
                    ]
                })
                .collect(),
        ),
    );

    groups.insert(
        1,
        (
            "Self-monitor",
            vec![
                (
                    "Trained detector installed".into(),
                    if state.self_monitor["installed"].as_bool() == Some(true) {
                        "Yes"
                    } else {
                        "No"
                    }
                    .into(),
                ),
                (
                    "Learned token reports".into(),
                    if state.self_monitor["learned_reporting"].as_bool() == Some(true) {
                        "Yes"
                    } else {
                        "No"
                    }
                    .into(),
                ),
                (
                    "Learned replay accepted / tried".into(),
                    format!(
                        "{} / {}",
                        state.adaptation["sequence_replay_accepted"]
                            .as_u64()
                            .unwrap_or(0),
                        state.adaptation["sequence_replay_attempts"]
                            .as_u64()
                            .unwrap_or(0)
                    ),
                ),
                (
                    "Core calibration".into(),
                    if state.self_monitor["calibration_matches_core"].as_bool() == Some(true) {
                        "Yes"
                    } else {
                        "Needs revalidation"
                    }
                    .into(),
                ),
                ("Live state validation".into(), "Not established".into()),
            ],
        ),
    );
    let adapt = &state.adaptation;
    groups.push((
        "Adaptive replay",
        vec![
            (
                "Branches grown / pruned".into(),
                format!(
                    "{} / {}",
                    adapt["dendrites_grown"].as_u64().unwrap_or(0),
                    adapt["dendrites_pruned"].as_u64().unwrap_or(0)
                ),
            ),
            (
                "Connection updates accepted / attempted".into(),
                format!(
                    "{} / {}",
                    adapt["connection_accepted"].as_u64().unwrap_or(0),
                    adapt["connection_attempts"].as_u64().unwrap_or(0)
                ),
            ),
            (
                "Feedback events".into(),
                count(adapt["feedback_events"].as_u64().unwrap_or(0)),
            ),
            (
                "Candidates / accepted".into(),
                format!(
                    "{} / {}",
                    adapt["attempts"].as_u64().unwrap_or(0),
                    adapt["accepted"].as_u64().unwrap_or(0)
                ),
            ),
            (
                "Rejected".into(),
                count(adapt["rejected"].as_u64().unwrap_or(0)),
            ),
            (
                "Neurons / synapses added".into(),
                format!(
                    "{} / {}",
                    adapt["neurons_added"].as_u64().unwrap_or(0),
                    adapt["synapses_added"].as_u64().unwrap_or(0)
                ),
            ),
            (
                "Stronger / weaker".into(),
                format!(
                    "{} / {}",
                    adapt["strengthened"].as_u64().unwrap_or(0),
                    adapt["weakened"].as_u64().unwrap_or(0)
                ),
            ),
            (
                "Plasticity rate".into(),
                format!("{:.5}", adapt["rate"].as_f64().unwrap_or(0.0)),
            ),
            (
                "Dream rounds".into(),
                count(adapt["dreams"].as_u64().unwrap_or(0)),
            ),
        ],
    ));
    // Each population's size and how near its neurons are to firing.
    let mut populations = Vec::new();
    for (at, name) in pulse.populations.iter().enumerate() {
        let voltages: Vec<f32> = pulse
            .population
            .iter()
            .zip(&pulse.voltages)
            .filter(|(g, _)| **g as usize == at)
            .map(|(_, v)| *v)
            .collect();
        let mean = voltages.iter().sum::<f32>() / voltages.len().max(1) as f32;
        populations.push((
            name.clone(),
            format!("{} · {mean:.2} V", count(voltages.len() as u64)),
        ));
    }
    groups.insert(2, ("Populations", populations));

    if !state.sources.is_empty() {
        groups.push((
            "Training sources",
            state
                .sources
                .iter()
                .map(|(s, n)| (s.clone(), count(*n)))
                .collect(),
        ));
    }

    let per_line = ((ui.available_width() / 360.0) as usize).clamp(1, 3);
    for line in groups.chunks(per_line) {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = space::L;
            ui.columns(per_line, |columns| {
                for (column, (title, rows)) in columns.iter_mut().zip(line) {
                    section(column, title, None, |ui| {
                        for (name, value) in rows {
                            if *title == "Populations" {
                                ui.horizontal(|ui| {
                                    dot(ui, population_colour(name));
                                    row(ui, name, value);
                                });
                            } else {
                                row(ui, name, value);
                            }
                        }
                    });
                }
            });
        });
    }

    section(
        ui,
        "Replay status",
        Some(
            "Candidates use observed lessons, with a small retention check. Generated dreams are not factual training labels.",
        ),
        |ui| {
            muted(
                ui,
                state.adaptation["last"]
                    .as_str()
                    .unwrap_or("No adaptive replay yet"),
            );
            if let Some(dreams) = state.adaptation["dream_candidates"].as_array() {
                for dream in dreams {
                    plain(ui, dream.as_str().unwrap_or_default());
                }
            }
        },
    );

    if !state.observations.is_empty() {
        section(ui, "Recent questions", None, |ui| {
            observations(ui, state, 20)
        });
    }

    section(
        ui,
        "Programs",
        Some("The operations its readout can choose between."),
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for program in &state.programs {
                    egui::Frame::new()
                        .fill(palette::raised())
                        .corner_radius(8.0)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(program)
                                    .font(theme::mono())
                                    .color(palette::text_secondary()),
                            );
                        });
                }
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pulse() -> Pulse {
        Pulse {
            frame: 40,
            voltages: (0..300).map(|i| (i as f32 / 300.0) - 0.2).collect(),
            population: (0..300).map(|i| (i % 2) as u8).collect(),
            populations: vec!["Context".into(), "Value".into()],
            ..Default::default()
        }
    }

    #[test]
    fn the_brain_panels_draw_with_and_without_a_state() {
        let context = egui::Context::default();
        theme::apply(&context);
        let state = CircuitState {
            programs: vec!["(a+b)".into(), "unknown".into()],
            curiosity: vec!["what is your name".into()],
            language_tokens: Some(512),
            ..Default::default()
        };
        let pulse = pulse();
        let mut view = BrainView::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                brain_panel(ui, &mut view, Some(&state));
                brain_panel(ui, &mut view, None);
                neuron_field(ui, &view, &pulse, 1.0, 200.0);
                activity_chart(ui, &view, &pulse, 80.0);
                metrics(ui, &state, &pulse);
            });
        });
        output.textures_delta.clear();
    }

    #[test]
    fn a_squircle_is_a_closed_soft_outline() {
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(100.0, 60.0));
        let egui::Shape::Path(path) = theme::squircle(rect, 20.0, Color32::WHITE, Stroke::NONE)
        else {
            panic!("a squircle is one path");
        };
        assert!(path.closed);
        assert!(path.points.iter().all(|p| rect.expand(0.01).contains(*p)));
        // It touches each side, and rounds every corner off.
        assert!(path.points.iter().any(|p| (p.x - 100.0).abs() < 0.01));
        assert!(
            !path
                .points
                .iter()
                .any(|p| p.distance(rect.right_top()) < 4.0)
        );
    }
}

//! The quiet row under the composer: on the left one line of numbers
//! (speed, cache, step) that opens into the detail; on the right the
//! model, how hard it thinks, and whether it asks before changing things.

use eframe::egui::containers::menu::{MenuButton, MenuConfig};
use eframe::egui::{
    self, Color32, CornerRadius, PopupCloseBehavior, Pos2, RichText, Sense, Stroke, Ui, vec2,
};

use crate::format;
use crate::session::{Cell, Stats};
use crate::settings::{Effort, Settings};
use crate::theme::{self, palette};

const ROW_HEIGHT: f32 = 14.0;
const SPARKLINE_WIDTH: f32 = 40.0;
const CACHE_BAR_WIDTH: f32 = 40.0;
const CELL_SIZE: f32 = 7.0;
const CELL_GAP: f32 = 2.0;

/// Squares of the loop strip shown; a long run shows its latest ones.
const MAX_CELLS: usize = 24;

/// Below this width the line of numbers is cut down to the step, so that
/// it cannot run into the selectors on the right.
const ROOMY_WIDTH: f32 = 600.0;

const DETAIL_WIDTH: f32 = 270.0;

/// Longest model name shown before it is cut.
const MAX_MODEL_CHARS: usize = 26;

/// A choice made in the row.
pub enum Change {
    Model(String),
    Effort(Effort),

    /// Whether to edit and run without asking.
    ApproveEverything(bool),

    /// Whether the model's reasoning is shown.
    ShowThinking(bool),
}

/// What the row shows about the session.
pub struct Reading<'a> {
    pub stats: &'a Stats,
    pub max_steps: u32,

    /// An estimate for the response being written right now.
    pub live_speed: Option<f32>,

    /// The model's name when it is not one of the settings' choices: the
    /// demo's stand-in. Shown as it is, with nothing to select.
    pub stand_in: Option<&'a str>,
}

/// `can_change` is false while a run is in progress or a past session is
/// on screen: the selectors are shown but do nothing.
pub fn show(
    ui: &mut Ui,
    reading: &Reading<'_>,
    settings: &Settings,
    can_change: bool,
) -> Option<Change> {
    let mut change = None;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;

        summary(ui, reading);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;

            if let Some(model) = reading.stand_in {
                quiet(ui, model);

                change = thinking_toggle(ui, settings);

                return;
            }

            ui.add_enabled_ui(can_change, |ui| {
                change = effort_selector(ui, settings)
                    .or_else(|| model_selector(ui, settings))
                    .or_else(|| approval_toggle(ui, settings));
            });

            // What is shown can be changed at any time, mid-run included.
            if let Some(shown) = thinking_toggle(ui, settings) {
                change = Some(shown);
            }
        });
    });

    change
}

/// The numbers in one line. Clicking it opens the detail.
fn summary(ui: &mut Ui, reading: &Reading<'_>) {
    let stats = reading.stats;

    let roomy = ui.available_width() >= ROOMY_WIDTH;

    // The square of whatever the loop did last; it pulses while the
    // model is working.
    if let Some(cell) = stats.cells.last() {
        // As tall as the button beside it, so the two share a centre line.
        let height = ui.spacing().interact_size.y;

        let (rect, _) = ui.allocate_exact_size(vec2(CELL_SIZE, height), Sense::hover());

        let square = egui::Rect::from_center_size(rect.center(), vec2(CELL_SIZE, CELL_SIZE));

        ui.painter()
            .rect_filled(square, CornerRadius::same(2), cell_color(*cell, pulse(ui)));
    }

    let mut parts = Vec::new();

    if roomy {
        match (reading.live_speed, stats.speed()) {
            (Some(speed), _) => parts.push(format!("~{speed:.0} tok/s")),
            (None, Some(speed)) => parts.push(format!("{speed:.0} tok/s")),
            (None, None) => {}
        }

        if let Some(ratio) = stats.cache_hit_ratio() {
            parts.push(format!("{} cached", format::percent(ratio)));
        }
    }

    parts.push(format!("step {}/{}", stats.step, reading.max_steps));

    let stay_open = MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside);

    selector(&parts.join("  ·  "))
        .config(stay_open)
        .ui(ui, |ui| {
            ui.set_width(DETAIL_WIDTH);
            ui.spacing_mut().item_spacing.y = 7.0;

            detail(ui, "Speed", |ui| speed(ui, reading));
            detail(ui, "Cache", |ui| cache(ui, stats));
            detail(ui, "Loop", |ui| agent_loop(ui, reading));

            detail(ui, "Tokens", |ui| {
                value(
                    ui,
                    &format!(
                        "{} in, {} out",
                        format::count(stats.usage.total_input()),
                        format::count(stats.usage.output_tokens),
                    ),
                );
            });

            if let Some(savings) = stats.savings() {
                ui.label(
                    RichText::new(format!("Spared: {savings}."))
                        .font(theme::small())
                        .color(palette::TEXT_MUTED),
                );
            }
        });
}

/// One named line of the detail.
fn detail(ui: &mut Ui, name: &str, content: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;

        let (rect, _) = ui.allocate_exact_size(vec2(52.0, ROW_HEIGHT), Sense::hover());

        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            name,
            theme::small(),
            palette::TEXT_MUTED,
        );

        content(ui);
    });
}

fn pulse(ui: &Ui) -> f32 {
    0.55 + 0.45 * (ui.input(|input| input.time) * 5.0).sin() as f32
}

fn speed(ui: &mut Ui, reading: &Reading<'_>) {
    let text = match (reading.live_speed, reading.stats.speed()) {
        (Some(speed), _) => format!("~{speed:.0} tok/s"),
        (None, Some(speed)) => format!("{speed:.0} tok/s"),
        (None, None) => "– tok/s".to_string(),
    };

    value(ui, &text).on_hover_text(
        "Output tokens per second. While a response is being written this \
         is an estimate from its length (marked ~); once it is complete, it \
         is the provider's own token count over the whole call.",
    );

    let speeds = reading.stats.recent_speeds();

    if speeds.len() < 2 {
        return;
    }

    let (rect, _) = ui.allocate_exact_size(vec2(SPARKLINE_WIDTH, ROW_HEIGHT), Sense::hover());

    let fastest = speeds.iter().copied().fold(1.0, f32::max);
    let step = rect.width() / (speeds.len() - 1) as f32;

    let points: Vec<Pos2> = speeds
        .iter()
        .enumerate()
        .map(|(index, speed)| {
            let height = speed / fastest * (rect.height() - 3.0);

            Pos2::new(
                rect.left() + index as f32 * step,
                rect.bottom() - 1.5 - height,
            )
        })
        .collect();

    ui.painter().line(points, Stroke::new(1.2, palette::GOOD));
}

fn cache(ui: &mut Ui, stats: &Stats) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(CACHE_BAR_WIDTH, ROW_HEIGHT), Sense::hover());

    let bar = egui::Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    let radius = CornerRadius::same(2);

    ui.painter().rect_filled(bar, radius, palette::RAISED);

    let ratio = stats.cache_hit_ratio();

    if let Some(ratio) = ratio {
        let mut hit = bar;

        hit.set_width(bar.width() * ratio);

        ui.painter().rect_filled(hit, radius, palette::GOOD);
    }

    let text = match ratio {
        Some(ratio) => format!("{} hit", format::percent(ratio)),
        None => "–".to_string(),
    };

    let explanation = format!(
        "{} input tokens were read from the provider's cache (hits); {} were not (misses).",
        format::count(stats.usage.cache_read_tokens),
        format::count(stats.usage.total_input() - stats.usage.cache_read_tokens),
    );

    response.on_hover_text(&explanation);
    value(ui, &text).on_hover_text(&explanation);
}

fn agent_loop(ui: &mut Ui, reading: &Reading<'_>) {
    let stats = reading.stats;

    let start = stats.cells.len().saturating_sub(MAX_CELLS);
    let cells = &stats.cells[start..];

    if !cells.is_empty() {
        let width = cells.len() as f32 * (CELL_SIZE + CELL_GAP) - CELL_GAP;

        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());

        let pulse = pulse(ui);

        for (index, cell) in cells.iter().enumerate() {
            let left = rect.left() + index as f32 * (CELL_SIZE + CELL_GAP);

            let square = egui::Rect::from_min_size(
                Pos2::new(left, rect.center().y - CELL_SIZE / 2.0),
                vec2(CELL_SIZE, CELL_SIZE),
            );

            ui.painter()
                .rect_filled(square, CornerRadius::same(2), cell_color(*cell, pulse));
        }
    }

    let mut explanation = format!(
        "The loop: one square per model response and per observation that went back to it. \
         {} observations so far in this run.\n\nThis session: {} tokens in, {} out.",
        stats.observations(),
        format::count(stats.usage.total_input()),
        format::count(stats.usage.output_tokens),
    );

    if let Some(savings) = stats.savings() {
        explanation.push_str(&format!("\n\nSpared so far: {savings}."));
    }

    value(ui, &format!("step {}/{}", stats.step, reading.max_steps)).on_hover_text(explanation);
}

fn cell_color(cell: Cell, pulse: f32) -> Color32 {
    match cell {
        Cell::Thinking => palette::MODEL.gamma_multiply(pulse),
        Cell::Model => palette::MODEL,
        Cell::Observation => palette::GOOD,
        Cell::FailedObservation => palette::BAD,
        Cell::Check { passed: true } => palette::TEXT,
        Cell::Check { passed: false } => palette::WARNING,
    }
}

/// A quiet button that opens a menu.
fn selector<'a>(label: &str) -> MenuButton<'a> {
    let button = egui::Button::new(
        RichText::new(label.to_string())
            .font(theme::small())
            .color(palette::TEXT_SECONDARY),
    )
    .fill(Color32::TRANSPARENT)
    .stroke(Stroke::NONE);

    MenuButton::from_button(button)
}

fn model_selector(ui: &mut Ui, settings: &Settings) -> Option<Change> {
    let current = settings.current();

    let label = if current.model.is_empty() {
        "Choose a model".to_string()
    } else {
        format::one_line(&current.model, MAX_MODEL_CHARS)
    };

    let mut change = None;

    selector(&label).ui(ui, |ui| {
        let choices = current.choices();

        if choices.is_empty() {
            ui.label(RichText::new("Add models in Settings.").color(palette::TEXT_MUTED));
        }

        for model in choices {
            let selected = model == current.model;

            if ui
                .selectable_label(selected, RichText::new(&model).font(theme::mono()))
                .clicked()
                && !selected
            {
                change = Some(Change::Model(model));
            }
        }
    });

    change
}

/// A slider in a popup. The choice is made when the slider is let go, so
/// dragging across it does not rebuild the agent at every stop.
fn effort_selector(ui: &mut Ui, settings: &Settings) -> Option<Change> {
    if !settings.provider.takes_effort() {
        return None;
    }

    let mut change = None;

    let stay_open = MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside);

    selector(settings.effort.label())
        .config(stay_open)
        .ui(ui, |ui| {
            ui.set_width(190.0);

            // The slider's position between frames, while it is being dragged.
            let id = ui.id().with("effort");

            let mut position = ui
                .data(|data| data.get_temp::<usize>(id))
                .unwrap_or(settings.effort.position());

            ui.label(RichText::new("How hard the model thinks").font(theme::small()));

            let slider = ui.add(
                egui::Slider::new(&mut position, 0..=Effort::ALL.len() - 1)
                    .show_value(false)
                    .step_by(1.0),
            );

            ui.label(RichText::new(Effort::ALL[position].label()).color(palette::TEXT));

            ui.data_mut(|data| data.insert_temp(id, position));

            let settled = slider.drag_stopped() || (slider.changed() && !slider.dragged());

            if settled && Effort::ALL[position] != settings.effort {
                change = Some(Change::Effort(Effort::ALL[position]));
            }
        });

    change
}

fn approval_toggle(ui: &mut Ui, settings: &Settings) -> Option<Change> {
    let (label, color) = if settings.approve_everything {
        ("Bypass permissions", palette::WARNING)
    } else {
        ("Ask before changes", palette::TEXT_SECONDARY)
    };

    let button = egui::Button::new(RichText::new(label).font(theme::small()).color(color))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE);

    let response = ui.add(button).on_hover_text(
        "Whether Cortex asks before it edits a file or runs a command. Click to switch.",
    );

    response
        .clicked()
        .then_some(Change::ApproveEverything(!settings.approve_everything))
}

fn thinking_toggle(ui: &mut Ui, settings: &Settings) -> Option<Change> {
    let (label, color) = if settings.show_thinking {
        ("Thinking shown", palette::TEXT_SECONDARY)
    } else {
        ("Thinking hidden", palette::TEXT_MUTED)
    };

    let button = egui::Button::new(RichText::new(label).font(theme::small()).color(color))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE);

    let response = ui.add(button).on_hover_text(
        "Whether the model's reasoning is shown as it arrives, for models and providers \
         that send it. Click to switch.",
    );

    response
        .clicked()
        .then_some(Change::ShowThinking(!settings.show_thinking))
}

fn value(ui: &mut Ui, text: &str) -> egui::Response {
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::TEXT_SECONDARY),
    )
}

fn quiet(ui: &mut Ui, text: &str) -> egui::Response {
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::TEXT_MUTED),
    )
}

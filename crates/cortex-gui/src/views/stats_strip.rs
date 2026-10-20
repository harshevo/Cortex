//! The quiet row under the composer: on the left one line of numbers
//! (cache, speed, step) that opens upward into the detail; on the right
//! whether it asks before changing things, the model, and how hard it
//! thinks.

use eframe::egui::{self, Color32, CornerRadius, Pos2, RichText, Sense, Stroke, Ui, vec2};

use super::menu::{self, Opens};

use crate::format;
use crate::session::{Cell, Stats};
use crate::settings::{Effort, Provider, Settings};
use crate::theme::{self, palette};

const ROW_HEIGHT: f32 = 14.0;
const SPARKLINE_WIDTH: f32 = 40.0;
const CACHE_BAR_WIDTH: f32 = 40.0;
const CELL_SIZE: f32 = 7.0;
const CELL_GAP: f32 = 2.0;

/// Squares of the loop strip shown; a long run shows its latest ones.
const MAX_CELLS: usize = 12;

/// Below this width the line of numbers is cut down to the step, so that
/// it cannot run into the selectors on the right.
const ROOMY_WIDTH: f32 = 600.0;

const DETAIL_WIDTH: f32 = 320.0;

/// Longest model name shown before it is cut.
const MAX_MODEL_CHARS: usize = 26;

const MENU_ROW_HEIGHT: f32 = crate::theme::control::MEDIUM;
const MODEL_MENU_WIDTH: f32 = 230.0;
const EFFORT_MENU_WIDTH: f32 = 170.0;

/// A choice made in the row.
pub enum Change {
    /// A model, and the provider it belongs to.
    Model(Provider, String),
    Effort(Effort),

    /// Whether to edit and run without asking.
    ApproveEverything(bool),

    /// Replace the older part of the conversation with a summary.
    Compact,
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
        ui.spacing_mut().item_spacing.x = crate::theme::space::XS;

        change = summary(ui, reading, can_change);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = crate::theme::space::XS;

            if let Some(model) = reading.stand_in {
                quiet(ui, model);
            } else {
                ui.add_enabled_ui(can_change, |ui| {
                    if let Some(chosen) = effort_selector(ui, settings)
                        .or_else(|| model_selector(ui, settings))
                        .or_else(|| approval_toggle(ui, settings))
                    {
                        change = Some(chosen);
                    }
                });
            }
        });
    });

    change
}

/// The numbers in one line. Clicking it opens the detail, where the
/// conversation can be compacted between runs.
fn summary(ui: &mut Ui, reading: &Reading<'_>, can_change: bool) -> Option<Change> {
    let stats = reading.stats;

    let mut change = None;

    let roomy = ui.available_width() >= ROOMY_WIDTH;

    // The square of whatever the loop did last; it pulses while the
    // model is working.
    if let Some(cell) = stats.cells.last() {
        // As tall as the button beside it, so the two share a centre line.
        let height = ui.spacing().interact_size.y;

        let (rect, _) = ui.allocate_exact_size(vec2(CELL_SIZE, height), Sense::hover());

        let square = egui::Rect::from_center_size(rect.center(), vec2(CELL_SIZE, CELL_SIZE));

        ui.painter().rect_filled(
            square,
            CornerRadius::same(crate::theme::radius::XS),
            cell_color(*cell, pulse(ui)),
        );
    }

    let mut parts = Vec::new();

    if roomy && let Some(ratio) = stats.cache_hit_ratio() {
        parts.push(format!("{} cached", format::percent(ratio)));
    }

    if roomy {
        match (reading.live_speed, stats.speed()) {
            (Some(speed), _) => parts.push(format!("~{speed:.0} tok/s")),
            (None, Some(speed)) => parts.push(format!("{speed:.0} tok/s")),
            (None, None) => {}
        }
    }

    // Only while there are steps to count.
    if stats.step > 0 {
        parts.push(format!("step {}/{}", stats.step, reading.max_steps));
    }

    if let Some(context) = &stats.context {
        parts.push(match context.share {
            Some(share) => format!("context {}", format::percent(share)),
            None => format!("context {}", format::count(context.tokens)),
        });
    }

    let button = ui.add(selector(&parts.join("  ·  ")));

    menu::attach_sticky(&button, Opens::Up, |ui| {
        ui.set_width(DETAIL_WIDTH);
        ui.spacing_mut().item_spacing.y = theme::space::M;
        menu::title(ui, "Session");

        detail(ui, "Speed", |ui| speed(ui, reading));
        detail(ui, "Cache", |ui| cache(ui, stats));
        detail(ui, "Loop", |ui| agent_loop(ui, reading));

        if let Some(context) = &stats.context {
            detail(ui, "Context", |ui| {
                value(ui, &context_line(context));
            });

            ui.collapsing("Context breakdown", |ui| parts_of(ui, context));
        }

        if let Some(quota) = &stats.quota {
            detail(ui, "Plan", |ui| {
                value(ui, &quota_line(quota));
            });
        }

        if !stats.held_out.is_empty() {
            ui.label(
                RichText::new(format!(
                    "Held out of this run, to measure whether they help: {}.",
                    stats.held_out.join(", ")
                ))
                .font(theme::small())
                .color(palette::text_muted()),
            );
        }

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

        ui.horizontal(|ui| {
            let button = ui.add_enabled(can_change, egui::Button::new("Compact"));

            let button = button.on_hover_text(
                "Replace the older part of the conversation with a summary the model \
                     writes. One extra model call; the next call reads less.",
            );

            if button.clicked() {
                change = Some(Change::Compact);
            }

            if stats.compactions > 0 {
                ui.label(
                    RichText::new(format!("{} compactions", stats.compactions))
                        .font(theme::small())
                        .color(palette::text_muted()),
                );
            }
        });

        if let Some(savings) = stats.savings() {
            ui.label(
                RichText::new(format!("Spared: {savings}."))
                    .font(theme::small())
                    .color(palette::text_muted()),
            );
        }
    });

    change
}

/// How full the window is: "42% of 1M", or the tokens alone when the
/// model's window is not known.
fn context_line(context: &cortex_core::window::ContextUse) -> String {
    let tokens = format::count(context.tokens);
    let about = if context.estimated { "~" } else { "" };

    match (context.window, context.share) {
        (Some(window), Some(share)) => format!(
            "{about}{tokens} of {} ({})",
            format::count(window),
            format::percent(share)
        ),

        _ => format!("{about}{tokens} tokens · limit unknown"),
    }
}

/// What the context is made of, largest first, as estimated tokens.
fn parts_of(ui: &mut Ui, context: &cortex_core::window::ContextUse) {
    let parts = &context.estimated_tokens;

    let mut named = [
        ("system prompt", parts.system),
        ("tool descriptions", parts.tool_specs),
        ("briefings and advice", parts.briefings),
        ("your messages", parts.user),
        ("the model's replies", parts.assistant),
        ("tool calls", parts.tool_calls),
        ("tool results", parts.tool_results),
        ("summary", parts.summary),
        ("other", parts.other),
    ];

    named.sort_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));

    for (name, tokens) in named.into_iter().filter(|(_, tokens)| *tokens > 0) {
        ui.horizontal(|ui| {
            quiet(ui, name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                value(ui, &format!("~{}", format::count(tokens)));
            });
        });
    }
}

/// A subscription's allowance, as the provider last reported it.
fn quota_line(quota: &cortex_core::window::PlanQuota) -> String {
    let window = |name: &str, window: &Option<cortex_core::window::QuotaWindow>| {
        window.map(|window| format!("{name} {:.0}% used", window.used_percent))
    };

    let parts: Vec<String> = [
        window("Session", &quota.primary),
        window("week", &quota.secondary),
    ]
    .into_iter()
    .flatten()
    .collect();

    if parts.is_empty() {
        "not reported".to_string()
    } else {
        parts.join(" · ")
    }
}

/// One named line of the detail.
fn detail(ui: &mut Ui, name: &str, content: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;

        let (rect, _) = ui.allocate_exact_size(vec2(60.0, ROW_HEIGHT), Sense::hover());

        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            name,
            theme::small(),
            palette::text_muted(),
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

    ui.painter().line(points, Stroke::new(1.2, palette::good()));
}

fn cache(ui: &mut Ui, stats: &Stats) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(CACHE_BAR_WIDTH, ROW_HEIGHT), Sense::hover());

    let bar = egui::Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    let radius = CornerRadius::same(crate::theme::radius::XS);

    ui.painter().rect_filled(bar, radius, palette::raised());

    let ratio = stats.cache_hit_ratio();

    if let Some(ratio) = ratio {
        let mut hit = bar;

        hit.set_width(bar.width() * ratio);

        ui.painter().rect_filled(hit, radius, palette::good());
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

            ui.painter().rect_filled(
                square,
                CornerRadius::same(crate::theme::radius::XS),
                cell_color(*cell, pulse),
            );
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

    if stats.step > 0 {
        value(ui, &format!("step {}/{}", stats.step, reading.max_steps)).on_hover_text(explanation);
    }
}

fn cell_color(cell: Cell, pulse: f32) -> Color32 {
    match cell {
        Cell::Thinking => palette::model().gamma_multiply(pulse),
        Cell::Model => palette::model(),
        Cell::Observation => palette::good(),
        Cell::FailedObservation => palette::bad(),
        Cell::Check { passed: true } => palette::text(),
        Cell::Check { passed: false } => palette::warning(),
    }
}

/// A quiet button that opens the detail.
fn selector(label: &str) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(label.to_string())
            .font(theme::small())
            .color(palette::text_secondary()),
    )
    .fill(Color32::TRANSPARENT)
    .stroke(Stroke::NONE)
}

/// A selector that opens a menu upward: the model and the effort.
fn dropdown<R>(ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    menu::dropdown(ui, None, label, true, Opens::UpLeft, content).1
}

/// One choice in a menu. Returns its rectangle, for whatever the caller
/// draws at its right-hand end.
fn menu_row(
    ui: &mut Ui,
    label: &str,
    font: egui::FontId,
    selected: bool,
) -> (egui::Rect, egui::Response) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), MENU_ROW_HEIGHT), Sense::click());

    if selected || response.hovered() {
        let fill = if selected {
            palette::raised()
        } else {
            palette::surface()
        };

        ui.painter()
            .rect_filled(rect, CornerRadius::same(crate::theme::radius::M), fill);
    }

    ui.painter().text(
        rect.left_center() + vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        if selected {
            palette::text()
        } else {
            palette::text_secondary()
        },
    );

    (rect, response)
}

fn menu_title(ui: &mut Ui, text: &str) {
    menu::title(ui, text);
}

/// Every provider's models in one menu, under the provider's name.
fn model_selector(ui: &mut Ui, settings: &Settings) -> Option<Change> {
    let current = settings.current();

    let label = if current.model.is_empty() {
        "Choose a model".to_string()
    } else {
        format::one_line(&current.model, MAX_MODEL_CHARS)
    };

    let mut change = None;

    dropdown(ui, &label, |ui| {
        ui.set_width(MODEL_MENU_WIDTH);
        ui.spacing_mut().item_spacing.y = 1.0;

        let mut any = false;

        for provider in Provider::ALL {
            let choices = settings.of(provider).choices();

            if choices.is_empty() {
                continue;
            }

            if any {
                ui.add_space(6.0);
            }

            any = true;

            menu_title(ui, provider.label());

            for model in choices {
                let selected = provider == settings.provider && model == current.model;

                let (rect, response) = menu_row(
                    ui,
                    &format::one_line(&model, MAX_MODEL_CHARS),
                    theme::mono(),
                    selected,
                );

                if selected {
                    let centre = rect.right_center() - vec2(14.0, 0.0);

                    ui.painter().line(
                        vec![
                            centre + vec2(-4.0, 0.0),
                            centre + vec2(-1.0, 3.0),
                            centre + vec2(4.0, -3.0),
                        ],
                        Stroke::new(1.4, palette::accent()),
                    );
                }

                if response.clicked() && !selected {
                    change = Some(Change::Model(provider, model));
                }
            }
        }

        if !any {
            ui.label(RichText::new("Add models in Settings.").color(palette::text_muted()));
        } else if Provider::ALL.len() > 1 {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                quiet(ui, "Another provider starts a new chat.");
            });
        }
    });

    change
}

/// The levels as a short list, each with as many bars lit as it is deep.
fn effort_selector(ui: &mut Ui, settings: &Settings) -> Option<Change> {
    if !settings.provider.takes_effort() {
        return None;
    }

    let mut change = None;

    dropdown(ui, settings.effort.label(), |ui| {
        ui.set_width(EFFORT_MENU_WIDTH);
        ui.spacing_mut().item_spacing.y = 1.0;

        menu_title(ui, "Reasoning effort");

        for effort in Effort::ALL {
            let selected = effort == settings.effort;

            let (rect, response) = menu_row(
                ui,
                effort.label(),
                egui::FontId::proportional(12.5),
                selected,
            );

            let lit = effort.position() + 1;
            let total = Effort::ALL.len();
            let right = rect.right() - 10.0;

            for bar in 0..total {
                let height = 4.0 + bar as f32 * 2.0;
                let left = right - (total - bar) as f32 * 5.0;

                let shape = egui::Rect::from_min_max(
                    Pos2::new(left, rect.center().y + 6.0 - height),
                    Pos2::new(left + 3.0, rect.center().y + 6.0),
                );

                let color = match (bar < lit, selected) {
                    (true, true) => palette::accent(),
                    (true, false) => palette::text_secondary(),
                    (false, _) => palette::border(),
                };

                ui.painter().rect_filled(
                    shape,
                    CornerRadius::same(crate::theme::radius::XS),
                    color,
                );
            }

            if response.clicked() && !selected {
                change = Some(Change::Effort(effort));
            }
        }
    });

    change
}

fn approval_toggle(ui: &mut Ui, settings: &Settings) -> Option<Change> {
    let (label, color) = if settings.approve_everything {
        ("Bypass permissions", palette::warning())
    } else {
        ("Ask before changes", palette::text_secondary())
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

fn value(ui: &mut Ui, text: &str) -> egui::Response {
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::text_secondary()),
    )
}

fn quiet(ui: &mut Ui, text: &str) -> egui::Response {
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::text_muted()),
    )
}

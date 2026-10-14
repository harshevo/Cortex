//! The tabs that are tables: the forecast with its plan, how live
//! forecasts came out, and how every network measures up.

use cortex_liquid::{NetMetrics, StepScore};
use cortex_market::market::time_label;
use cortex_market::{Brain, Market, Snapshot, Source, Tally, Tone, clock};
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Ui, vec2};

use super::format;
use super::panel::tone_colour;
use crate::theme::{self, palette, space};

const ROW: f32 = 26.0;

/// One cell: its text and colour.
struct Cell {
    text: String,
    colour: Color32,
    strong: bool,
}

fn cell(text: impl Into<String>) -> Cell {
    Cell {
        text: text.into(),
        colour: palette::text(),
        strong: false,
    }
}

fn tinted(text: impl Into<String>, colour: Color32) -> Cell {
    Cell {
        text: text.into(),
        colour,
        strong: false,
    }
}

fn strong(text: impl Into<String>, colour: Color32) -> Cell {
    Cell {
        text: text.into(),
        colour,
        strong: true,
    }
}

/// A table whose columns share the width by the given weights. The first
/// column is aligned left, the rest right, as numbers are.
fn table(ui: &mut Ui, weights: &[f32], header: &[&str], rows: Vec<Vec<Cell>>) {
    table_with_text(ui, weights, header, rows, 1);
}

/// A table whose first `words` columns hold words, aligned left.
fn table_with_text(
    ui: &mut Ui,
    weights: &[f32],
    header: &[&str],
    rows: Vec<Vec<Cell>>,
    words: usize,
) {
    let width = ui.available_width();
    let total: f32 = weights.iter().sum();
    let widths: Vec<f32> = weights
        .iter()
        .map(|weight| weight / total * (width - 16.0))
        .collect();

    let draw = |ui: &mut Ui, cells: &[Cell], header: bool, stripe: bool| {
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());

        if stripe {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(theme::radius::S),
                palette::surface().gamma_multiply(0.7),
            );
        }

        let mut left = rect.left() + 8.0;

        for (at, cell) in cells.iter().enumerate() {
            let column = widths.get(at).copied().unwrap_or(60.0);
            let font = if header {
                theme::strong(11.5)
            } else if cell.strong {
                theme::strong(12.5)
            } else {
                egui::FontId::proportional(12.5)
            };

            let colour = if header {
                palette::text_muted()
            } else {
                cell.colour
            };
            let galley = ui
                .painter()
                .layout(cell.text.clone(), font, colour, column - 8.0);

            let x = if at < words {
                left
            } else {
                left + column - galley.size().x - 4.0
            };

            ui.painter().galley(
                egui::pos2(x, rect.center().y - galley.size().y / 2.0),
                galley,
                colour,
            );

            left += column;
        }
    };

    let header_cells: Vec<Cell> = header.iter().map(|name| cell(*name)).collect();

    draw(ui, &header_cells, true, false);

    for (at, row) in rows.iter().enumerate() {
        draw(ui, row, false, at % 2 == 0);
    }
}

fn title(ui: &mut Ui, text: &str, detail: &str) {
    ui.add_space(space::M);

    ui.horizontal(|ui| {
        ui.label(RichText::new(text).font(theme::strong(15.0)));

        if !detail.is_empty() {
            ui.label(
                RichText::new(detail)
                    .font(theme::small())
                    .color(palette::text_muted()),
            );
        }
    });

    ui.add_space(space::XS);
}

fn up_down(change: f64) -> Color32 {
    if change > 0.0 {
        palette::good()
    } else if change < 0.0 {
        palette::bad()
    } else {
        palette::text_secondary()
    }
}

pub fn forecast(ui: &mut Ui, snapshot: &Snapshot) {
    let intraday = snapshot.interval.is_intraday();
    let brain = snapshot.brain;

    let using = match brain {
        Brain::Llm => format!(
            "LLM · {}",
            snapshot.llm.model.as_deref().unwrap_or("not set up")
        ),
        _ => format!("{} · {} network", brain.short(), brain.name()),
    };

    title(ui, "Next 10 candles", &using);

    if snapshot.rows.is_empty() {
        ui.label(
            RichText::new(match brain {
                Brain::Llm => "Ask the model from the Chart tab.",
                _ => "The networks are training, or there are too few candles.",
            })
            .color(palette::text_muted()),
        );
    } else {
        let has_volume = snapshot.rows.iter().any(|row| row.volume > 0.0);

        let rows = snapshot
            .rows
            .iter()
            .map(|row| {
                let colour = if row.close >= row.open {
                    palette::good()
                } else {
                    palette::bad()
                };

                vec![
                    strong(
                        format!("+{}  {}", row.step, time_label(row.time_ms, intraday)),
                        palette::text(),
                    ),
                    cell(format::price(row.open)),
                    cell(format::price(row.high)),
                    cell(format::price(row.low)),
                    strong(format::price(row.close), colour),
                    tinted(
                        format!(
                            "{} – {}",
                            format::price(row.close_low),
                            format::price(row.close_high)
                        ),
                        palette::text_secondary(),
                    ),
                    tinted(format::percent(row.change_pct), up_down(row.change_pct)),
                    cell(if has_volume {
                        format::quantity(row.volume)
                    } else {
                        "—".to_string()
                    }),
                    cell(if has_volume {
                        format::quantity(row.turnover)
                    } else {
                        "—".to_string()
                    }),
                    cell(format::maybe(row.rsi, |value| format!("{value:.1}"))),
                    cell(format::maybe(row.vwap, format::price)),
                ]
            })
            .collect();

        table(
            ui,
            &[1.3, 1.0, 1.0, 1.0, 1.0, 1.6, 0.8, 0.9, 0.9, 0.6, 1.0],
            &[
                "Candle",
                "Open",
                "High",
                "Low",
                "Close",
                "Close 80%",
                "Change",
                "Volume",
                "Turnover",
                "RSI",
                "VWAP",
            ],
            rows,
        );
    }

    let Some(plan) = &snapshot.plan else {
        return;
    };

    title(ui, "Plan", "");

    ui.horizontal(|ui| {
        egui::Frame::new()
            .fill(tone_colour(plan.bias.tone).gamma_multiply(0.14))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(&plan.bias.label)
                        .font(theme::strong(13.0))
                        .color(tone_colour(plan.bias.tone)),
                );
            });

        ui.label(
            RichText::new(plan.bias.reasons.join(" · "))
                .font(theme::small())
                .color(palette::text_secondary()),
        );
    });

    ui.add_space(space::XS);

    let rows = plan
        .scenarios
        .iter()
        .map(|scenario| {
            let colour = tone_colour(scenario.tone);
            let mark = match scenario.tone {
                Tone::Up => "▲ ",
                Tone::Down => "▼ ",
                Tone::Neutral => "■ ",
            };

            vec![
                strong(format!("{mark}{}", scenario.case), colour),
                tinted(&scenario.expect, palette::text_secondary()),
                cell(&scenario.action),
                cell(format::maybe(scenario.entry, format::price)),
                tinted(
                    format::maybe(scenario.target, format::price),
                    palette::good(),
                ),
                tinted(format::maybe(scenario.stop, format::price), palette::bad()),
            ]
        })
        .collect();

    table_with_text(
        ui,
        &[2.2, 2.6, 2.6, 0.9, 0.9, 0.9],
        &["If", "Expect", "Do", "At", "Target", "Stop"],
        rows,
        3,
    );

    ui.add_space(space::S);
    ui.label(
        RichText::new("Rules applied to the forecast and levels. Not advice; see Outcomes for how forecasts have done.")
            .font(theme::small())
            .color(palette::text_muted()),
    );
}

/// The share right as text, with how many.
fn record_text(tally: Option<&Tally>) -> (String, Color32) {
    match tally.and_then(|tally| Some((tally.accuracy()?, tally.judged))) {
        Some((accuracy, judged)) => (
            format!("{} of {judged}", format::share(accuracy)),
            if accuracy > 0.55 {
                palette::good()
            } else if accuracy < 0.45 {
                palette::bad()
            } else {
                palette::text()
            },
        ),
        None => ("—".to_string(), palette::text_muted()),
    }
}

pub fn outcomes(ui: &mut Ui, market: &Market, snapshot: &Snapshot) {
    let live = snapshot.source.as_ref().is_some_and(Source::is_live);

    title(ui, "How the forecasts came out", "");

    if !live {
        ui.label(
            RichText::new("Outcomes are recorded for live Dhan instruments: each closed candle's forecasts are checked when their candles close.")
                .color(palette::text_muted()),
        );

        return;
    }

    // One row per network: right at each horizon, inside the band.
    let mut rows = Vec::new();

    for brain in Brain::ALL {
        let of = |step: usize| {
            snapshot
                .tallies
                .iter()
                .find(|tally| tally.source == brain.short() && tally.step == step)
        };

        let made: usize = snapshot
            .tallies
            .iter()
            .filter(|tally| tally.source == brain.short())
            .map(|tally| tally.made)
            .sum();

        if made == 0 {
            continue;
        }

        let pending: usize = snapshot
            .tallies
            .iter()
            .filter(|tally| tally.source == brain.short())
            .map(|tally| tally.made - tally.resolved)
            .sum();

        let (one, one_colour) = record_text(of(1));
        let (five, five_colour) = record_text(of(5));
        let (ten, ten_colour) = record_text(of(10));

        let inside = of(1).and_then(Tally::inside_rate);
        let error = of(1)
            .filter(|tally| tally.resolved > 0)
            .map(|tally| tally.mean_error_pct);

        rows.push(vec![
            strong(
                format!("{} · {}", brain.short(), brain.name()),
                if brain == snapshot.brain {
                    palette::accent()
                } else {
                    palette::text()
                },
            ),
            tinted(one, one_colour),
            tinted(five, five_colour),
            tinted(ten, ten_colour),
            cell(format::maybe(inside, format::share)),
            cell(format::maybe(error, |value| format!("{value:.2}%"))),
            tinted(pending.to_string(), palette::text_muted()),
        ]);
    }

    if rows.is_empty() {
        ui.label(
            RichText::new("Nothing recorded yet: forecasts are written down as candles close.")
                .color(palette::text_muted()),
        );

        return;
    }

    table(
        ui,
        &[1.8, 1.0, 1.0, 1.0, 1.0, 1.0, 0.8],
        &[
            "Network",
            "Right +1",
            "Right +5",
            "Right +10",
            "In 80% band",
            "Error +1",
            "Waiting",
        ],
        rows,
    );

    title(ui, "Each forecast", "newest first");

    let intraday = snapshot.interval.is_intraday();

    let rows = snapshot
        .outcomes
        .iter()
        .take(200)
        .map(|prediction| {
            let (result, colour) = match prediction.outcome {
                None => ("waiting".to_string(), palette::text_muted()),
                Some(outcome) => match outcome.right {
                    Some(true) => ("✓ right".to_string(), palette::good()),
                    Some(false) => ("✗ wrong".to_string(), palette::bad()),
                    None => ("flat".to_string(), palette::text_secondary()),
                },
            };

            let direction = match prediction.up() {
                Some(true) => "▲",
                Some(false) => "▼",
                None => "■",
            };

            vec![
                cell(time_label(prediction.made_ms, intraday)),
                strong(&prediction.source, palette::text()),
                cell(format!(
                    "+{} → {}",
                    prediction.step,
                    time_label(prediction.due_ms, intraday)
                )),
                cell(format::price(prediction.from)),
                tinted(
                    format!("{direction} {}", format::price(prediction.close)),
                    up_down(prediction.close - prediction.from),
                ),
                cell(
                    prediction
                        .outcome
                        .map_or("—".to_string(), |outcome| format::price(outcome.actual)),
                ),
                cell(prediction.outcome.map_or("—".to_string(), |outcome| {
                    format!("{:.2}%", outcome.error_pct)
                })),
                tinted(
                    prediction
                        .outcome
                        .and_then(|outcome| outcome.inside)
                        .map_or("—", |inside| if inside { "in" } else { "out" }),
                    palette::text_secondary(),
                ),
                strong(result, colour),
            ]
        })
        .collect();

    table(
        ui,
        &[0.9, 0.7, 1.2, 1.0, 1.2, 1.0, 0.8, 0.6, 0.9],
        &[
            "Made", "By", "For", "From", "Forecast", "Actual", "Error", "Band", "Result",
        ],
        rows,
    );

    ui.add_space(space::M);

    if ui.small_button("Clear this instrument's record").clicked() {
        market.clear_outcomes(false);
    }

    let _ = clock::now_ms;
}

pub fn metrics(ui: &mut Ui, market: &Market, snapshot: &Snapshot) {
    let Some(report) = &snapshot.report else {
        title(ui, "Metrics", "");
        ui.label(
            RichText::new("The networks have not been trained on this series yet.")
                .color(palette::text_muted()),
        );

        return;
    };

    title(
        ui,
        "Held-out metrics",
        &format!(
            "trained on {} · {} candles, the last {} held out · {:.0} ms",
            snapshot.trained_on,
            report.bars,
            report.bars - report.train_bars,
            report.total_ms
        ),
    );

    // Direction called right on held-out candles, per network and horizon.
    let score_text = |score: Option<&StepScore>| -> (String, Color32) {
        match score.and_then(|score| Some((score.accuracy?, score.low?, score.high?))) {
            Some((accuracy, low, high)) => (
                format!(
                    "{}  ({}–{})",
                    format::share(accuracy),
                    format::share(low),
                    format::share(high)
                ),
                if low > 0.5 {
                    palette::good()
                } else if high < 0.5 {
                    palette::bad()
                } else {
                    palette::text()
                },
            ),
            None => ("—".to_string(), palette::text_muted()),
        }
    };

    let mut rows = Vec::new();

    for metrics in &report.metrics {
        let brain = Brain::of(metrics.net);
        let (one, one_colour) = score_text(metrics.at(1));
        let (five, five_colour) = score_text(metrics.at(5));
        let (ten, ten_colour) = score_text(metrics.at(10));

        rows.push(vec![
            strong(
                format!("{} · {}", brain.short(), brain.name()),
                if brain == snapshot.brain {
                    palette::accent()
                } else {
                    palette::text()
                },
            ),
            tinted(one, one_colour),
            tinted(five, five_colour),
            tinted(ten, ten_colour),
            cell(format::maybe(
                metrics.at(1).map(|score| score.error_pct),
                |value| format!("{value:.3}%"),
            )),
            tinted(
                format::maybe(metrics.at(1).map(|score| score.skill), |value| {
                    format!("{value:+.3}")
                }),
                up_down(metrics.at(1).map_or(0.0, |score| score.skill)),
            ),
            cell(format::maybe(metrics.band_hit, format::share)),
            cell(format::maybe(metrics.volume_error, format::share)),
        ]);
    }

    let (one, _) = score_text(report.baseline.iter().find(|score| score.step == 1));
    let (five, _) = score_text(report.baseline.iter().find(|score| score.step == 5));
    let (ten, _) = score_text(report.baseline.iter().find(|score| score.step == 10));

    rows.push(vec![
        tinted("Baseline · same as last", palette::text_muted()),
        tinted(one, palette::text_muted()),
        tinted(five, palette::text_muted()),
        tinted(ten, palette::text_muted()),
        tinted("—", palette::text_muted()),
        tinted("0", palette::text_muted()),
        tinted("—", palette::text_muted()),
        tinted("—", palette::text_muted()),
    ]);

    table(
        ui,
        &[1.8, 1.4, 1.4, 1.4, 0.9, 0.8, 0.9, 0.9],
        &[
            "Network",
            "Direction +1",
            "Direction +5",
            "Direction +10",
            "Error +1",
            "Skill",
            "In 80% band",
            "Volume error",
        ],
        rows,
    );

    ui.label(
        RichText::new("Direction: share called right, with its 99% range — green only when the whole range is above 50%. Skill above 0 beats predicting no change. A calibrated band holds about 80%.")
            .font(theme::small())
            .color(palette::text_muted()),
    );

    title(ui, "Each network", "");

    let width = ui.available_width();
    let columns = if width > 1100.0 { 4 } else { 2 };
    let card_width = (width - space::M * (columns - 1) as f32) / columns as f32;

    let cards: Vec<&NetMetrics> = report.metrics.iter().collect();

    for chunk in cards.chunks(columns) {
        ui.horizontal_top(|ui| {
            for metrics in chunk {
                ui.allocate_ui_with_layout(
                    vec2(card_width, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        network_card(ui, metrics, snapshot);
                    },
                );
            }
        });

        ui.add_space(space::M);
    }

    // The language model has no held-out record: only live outcomes.
    let llm = &snapshot.llm;

    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());

        ui.horizontal(|ui| {
            ui.label(RichText::new("LLM").font(theme::strong(13.0)).color(palette::accent()));
            ui.label(RichText::new(llm.model.as_deref().unwrap_or("No model set up")).font(theme::strong(13.0)));
        });

        let mean = if llm.calls > 0 { llm.total_ms / f64::from(llm.calls) } else { 0.0 };

        super::panel::row(ui, "Asked", &format!("{} times, {} failed", llm.calls, llm.failures), palette::text());

        if llm.calls > 0 {
            super::panel::row(ui, "Mean answer time", &format!("{:.1} s", mean / 1_000.0), palette::text());
        }

        let all = market.all_tallies();
        let mine: Vec<&Tally> = all.iter().filter(|tally| tally.source == "LLM").collect();
        let judged: usize = mine.iter().map(|tally| tally.judged).sum();
        let right: usize = mine.iter().map(|tally| tally.right).sum();

        super::panel::row(
            ui,
            "Live, all instruments",
            &if judged > 0 {
                format!("{right} of {judged} right ({})", format::share(right as f64 / judged as f64))
            } else {
                "nothing checked yet".to_string()
            },
            palette::text(),
        );

        ui.label(
            RichText::new("Too slow and costly to score on thousands of held-out candles; judged on live outcomes only.")
                .font(theme::small())
                .color(palette::text_muted()),
        );
    });
}

fn network_card(ui: &mut Ui, metrics: &NetMetrics, snapshot: &Snapshot) {
    let brain = Brain::of(metrics.net);

    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(brain.short())
                    .font(theme::strong(13.0))
                    .color(palette::accent()),
            );
            ui.label(RichText::new(brain.name()).font(theme::strong(13.0)));
        });

        ui.label(
            RichText::new(brain.about())
                .font(theme::small())
                .color(palette::text_muted()),
        );
        ui.add_space(space::XS);

        // Direction right at each horizon, as a bar from 30% to 70%.
        for score in &metrics.steps {
            accuracy_bar(ui, score);
        }

        ui.add_space(space::XS);

        let row = super::panel::row;

        if metrics.parameters > 0 {
            row(
                ui,
                "Trained weights",
                &format::quantity(metrics.parameters as f64),
                palette::text(),
            );
        }

        if metrics.neurons > 0 {
            row(ui, "Neurons", &metrics.neurons.to_string(), palette::text());
        }

        if metrics.net != cortex_liquid::Net::Ensemble {
            row(
                ui,
                "Ensemble weight",
                &format::share(f64::from(metrics.weight)),
                palette::text(),
            );
            row(
                ui,
                "Training",
                &format!("{:.0} ms", metrics.train_ms),
                palette::text(),
            );
        }

        for (name, value) in &metrics.facts {
            row(ui, name, value, palette::text());
        }

        if metrics.net == cortex_liquid::Net::Spiking && snapshot.spikes > 0 {
            row(
                ui,
                "Spikes, last candle",
                &snapshot.spikes.to_string(),
                palette::text(),
            );
        }

        if metrics.losses.len() > 1 {
            ui.add_space(space::XS);
            loss_curve(ui, &metrics.losses);
        }
    });
}

fn accuracy_bar(ui: &mut Ui, score: &StepScore) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("+{}", score.step))
                .font(theme::small())
                .color(palette::text_secondary()),
        );

        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width() - 44.0, 8.0), Sense::hover());
        let painter = ui.painter_at(rect.expand(3.0));
        let at =
            |share: f64| rect.left() + ((share - 0.3) / 0.4).clamp(0.0, 1.0) as f32 * rect.width();

        painter.rect_filled(rect, CornerRadius::same(4), palette::sunken());

        if let (Some(low), Some(high)) = (score.low, score.high) {
            painter.rect_filled(
                egui::Rect::from_x_y_ranges(at(low)..=at(high), rect.y_range()),
                CornerRadius::same(4),
                palette::accent().gamma_multiply(0.3),
            );
        }

        painter.line_segment(
            [
                egui::pos2(at(0.5), rect.top() - 2.0),
                egui::pos2(at(0.5), rect.bottom() + 2.0),
            ],
            egui::Stroke::new(1.0, palette::text_muted()),
        );

        if let Some(accuracy) = score.accuracy {
            painter.circle_filled(
                egui::pos2(at(accuracy), rect.center().y),
                4.0,
                palette::accent(),
            );
        }

        ui.label(
            RichText::new(format::maybe(score.accuracy, format::share))
                .font(theme::small())
                .color(palette::text()),
        );
    });
}

fn loss_curve(ui: &mut Ui, losses: &[cortex_liquid::EpochLoss]) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, CornerRadius::same(4), palette::sunken());

    let values = losses.iter().flat_map(|loss| [loss.fit, loss.check]);
    let (low, high) = values.fold((f32::MAX, f32::MIN), |(low, high), value| {
        (low.min(value), high.max(value))
    });
    let span = (high - low).max(1e-6);

    let point = |epoch: usize, value: f32| {
        egui::pos2(
            rect.left() + 4.0 + epoch as f32 / (losses.len() - 1) as f32 * (rect.width() - 8.0),
            rect.bottom() - 4.0 - (value - low) / span * (rect.height() - 8.0),
        )
    };

    let picks: [(fn(&cortex_liquid::EpochLoss) -> f32, Color32); 2] = [
        (|loss| loss.check, palette::accent().gamma_multiply(0.4)),
        (|loss| loss.fit, palette::accent()),
    ];

    for (pick, colour) in picks {
        let points: Vec<egui::Pos2> = losses
            .iter()
            .enumerate()
            .map(|(epoch, loss)| point(epoch, pick(loss)))
            .collect();

        painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, colour)));
    }

    painter.text(
        rect.right_top() + vec2(-4.0, 3.0),
        egui::Align2::RIGHT_TOP,
        "loss: fit · held back",
        theme::small(),
        palette::text_muted(),
    );
}

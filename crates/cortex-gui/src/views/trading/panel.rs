//! Beside the chart: the network in use and its record, the lean, the
//! next candle, and the numbers for right now.

use cortex_market::{Asking, Brain, Market, Snapshot, Source, Tone};
use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Ui, vec2};

use super::{PictureScale, format};
use crate::theme::{self, palette, space};

pub fn tone_colour(tone: Tone) -> Color32 {
    match tone {
        Tone::Up => palette::good(),
        Tone::Down => palette::bad(),
        Tone::Neutral => palette::text_secondary(),
    }
}

pub fn show(ui: &mut Ui, market: &Market, snapshot: &Snapshot, picture: &mut PictureScale) {
    network(ui, market, snapshot);

    if let Some(plan) = &snapshot.plan {
        ui.add_space(space::S);

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&plan.bias.label)
                        .font(theme::strong(16.0))
                        .color(tone_colour(plan.bias.tone)),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{:+}", plan.bias.score))
                            .font(theme::mono())
                            .color(palette::text_muted()),
                    );
                });
            });

            ui.label(
                RichText::new(plan.bias.reasons.join(" · "))
                    .font(theme::small())
                    .color(palette::text_secondary()),
            );

            ui.add_space(space::XS);

            row(ui, "Support", &format::price(plan.support), palette::good());
            row(
                ui,
                "Resistance",
                &format::price(plan.resistance),
                palette::bad(),
            );
        });
    }

    if let (Some(next), Some(last)) = (snapshot.rows.first(), snapshot.rows.last()) {
        ui.add_space(space::S);

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());

            heading(ui, "Next candle");

            let colour = if next.close >= next.open {
                palette::good()
            } else {
                palette::bad()
            };

            ui.label(
                RichText::new(format!(
                    "{}  {}",
                    format::price(next.close),
                    format::percent(next.change_pct)
                ))
                .font(theme::strong(15.0))
                .color(colour),
            );

            ui.label(
                RichText::new(format!(
                    "O {}  H {}  L {}",
                    format::price(next.open),
                    format::price(next.high),
                    format::price(next.low)
                ))
                .font(theme::small())
                .color(palette::text_secondary()),
            );

            ui.add_space(space::XS);

            row(
                ui,
                &format!("In {}", last.step),
                &format!(
                    "{}  {}",
                    format::price(last.close),
                    format::percent(last.change_pct)
                ),
                if last.change_pct >= 0.0 {
                    palette::good()
                } else {
                    palette::bad()
                },
            );
            row(
                ui,
                "80% range",
                &format!(
                    "{} – {}",
                    format::price(last.close_low),
                    format::price(last.close_high)
                ),
                palette::text_secondary(),
            );
        });
    }

    ui.add_space(space::S);

    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());

        heading(ui, "Now");

        let now = &snapshot.now;
        let plain = palette::text();

        row(ui, "VWAP", &format::maybe(now.vwap, format::price), plain);
        row(
            ui,
            "RSI 14",
            &format::maybe(now.rsi, |value| format!("{value:.1}")),
            rsi_colour(now.rsi),
        );
        row(
            ui,
            "EMA 9 / 21",
            &format!(
                "{} / {}",
                format::maybe(now.ema_fast, format::price),
                format::maybe(now.ema_slow, format::price)
            ),
            plain,
        );
        row(ui, "ATR 14", &format::maybe(now.atr, format::price), plain);

        if let Some(volume) = now.volume {
            row(ui, "Volume", &format::quantity(volume), plain);
        }

        if let (Some(high), Some(low)) = (now.day_high, now.day_low) {
            row(
                ui,
                "Day range",
                &format!("{} – {}", format::price(low), format::price(high)),
                plain,
            );
        }

        if let (Some(bid), Some(ask)) = (now.bid, now.ask) {
            row(
                ui,
                "Bid / ask",
                &format!("{} / {}", format::price(bid), format::price(ask)),
                plain,
            );
        }

        if let Some(spread) = now.spread {
            row(ui, "Spread", &format!("{spread:.2}"), plain);
        }

        if let Some(share) = now.buy_share {
            ui.add_space(space::XS);

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Buyers")
                        .font(theme::small())
                        .color(palette::text_secondary()),
                );

                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width() - 40.0, 8.0), Sense::hover());

                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(4),
                    palette::bad().gamma_multiply(0.7),
                );
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        rect.min,
                        vec2(rect.width() * share as f32, rect.height()),
                    ),
                    CornerRadius::same(4),
                    palette::good(),
                );

                ui.label(RichText::new(format::share(share)).font(theme::small()));
            });

            if let (Some(buy), Some(sell)) = (now.bid_quantity, now.ask_quantity) {
                row(
                    ui,
                    "Waiting",
                    &format!(
                        "{} buy · {} sell",
                        format::quantity(buy),
                        format::quantity(sell)
                    ),
                    palette::text_secondary(),
                );
            }
        }

        if let Some(oi) = now.open_interest {
            row(ui, "Open interest", &format::quantity(oi), plain);
        }
    });

    if let Some(Source::Picture { name }) = &snapshot.source {
        ui.add_space(space::S);

        picture_scale(ui, market, name, picture);
    }

    if !snapshot.notes.is_empty() {
        ui.add_space(space::S);

        for note in &snapshot.notes {
            ui.label(
                RichText::new(note)
                    .font(theme::small())
                    .color(palette::text_muted()),
            );
        }
    }
}

fn network(ui: &mut Ui, market: &Market, snapshot: &Snapshot) {
    let brain = snapshot.brain;

    egui::Frame::new()
        .fill(palette::accent().gamma_multiply(0.10))
        .stroke(egui::Stroke::new(
            1.0,
            palette::accent().gamma_multiply(0.5),
        ))
        .corner_radius(CornerRadius::same(theme::radius::L))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(brain.short())
                        .font(theme::strong(13.0))
                        .color(palette::accent()),
                );

                let name = match brain {
                    Brain::Llm => snapshot
                        .llm
                        .model
                        .clone()
                        .unwrap_or_else(|| "Language model".to_string()),
                    _ => format!("{} network", brain.name()),
                };

                ui.label(RichText::new(name).font(theme::strong(13.0)));
            });

            ui.label(
                RichText::new(brain.about())
                    .font(theme::small())
                    .color(palette::text_secondary()),
            );

            ui.add_space(space::XS);

            if let Some(metrics) = brain
                .net()
                .and_then(|net| snapshot.report.as_ref()?.of(net).cloned())
            {
                let at = |step: usize| {
                    metrics
                        .at(step)
                        .and_then(|score| score.accuracy)
                        .map_or("—".to_string(), format::share)
                };

                row(
                    ui,
                    "Held out",
                    &format!("+1 {}  ·  +5 {}  ·  +10 {}", at(1), at(5), at(10)),
                    palette::text(),
                );
            }

            let live: Vec<_> = snapshot
                .tallies
                .iter()
                .filter(|tally| tally.source == brain.short() && tally.judged > 0)
                .collect();

            if !live.is_empty() {
                let right: usize = live.iter().map(|tally| tally.right).sum();
                let judged: usize = live.iter().map(|tally| tally.judged).sum();

                row(
                    ui,
                    "Live",
                    &format!(
                        "{right} of {judged} right ({})",
                        format::share(right as f64 / judged as f64)
                    ),
                    palette::text(),
                );
            }

            if brain == Brain::Llm {
                let llm = &snapshot.llm;

                match &llm.asking {
                    Asking::Asking => {
                        ui.label(
                            RichText::new("Asking…")
                                .font(theme::small())
                                .color(palette::text_muted()),
                        );
                    }
                    Asking::Failed(reason) => {
                        ui.label(
                            RichText::new(reason)
                                .font(theme::small())
                                .color(palette::bad()),
                        );
                    }
                    Asking::Idle => {}
                }

                if !llm.reason.is_empty() {
                    ui.label(
                        RichText::new(&llm.reason)
                            .font(theme::small())
                            .color(palette::text()),
                    );
                }

                if ui.small_button("Ask again").clicked() {
                    market.ask_llm(true);
                }
            }
        });
}

fn picture_scale(ui: &mut Ui, market: &Market, name: &str, picture: &mut PictureScale) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());

        heading(ui, "Picture's price scale");

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(picture.top)
                    .hint_text("Highest")
                    .desired_width(80.0),
            );
            ui.add(
                egui::TextEdit::singleline(picture.bottom)
                    .hint_text("Lowest")
                    .desired_width(80.0),
            );

            let top: Option<f64> = picture.top.trim().replace(',', "").parse().ok();
            let bottom: Option<f64> = picture.bottom.trim().replace(',', "").parse().ok();
            let ready =
                matches!((top, bottom), (Some(top), Some(bottom)) if top > bottom && bottom > 0.0);

            if ui.add_enabled(ready, egui::Button::new("Apply")).clicked()
                && let (Some((_, bytes)), Some(top), Some(bottom)) = (picture.picture, top, bottom)
            {
                let _ = market.load_picture(name, bytes.clone(), Some((top, bottom)));
            }
        });
    });
}

fn rsi_colour(rsi: Option<f64>) -> Color32 {
    match rsi {
        Some(value) if value >= 70.0 => palette::bad(),
        Some(value) if value <= 30.0 => palette::good(),
        _ => palette::text(),
    }
}

pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::strong(11.5))
            .color(palette::text_muted()),
    );
}

pub fn row(ui: &mut Ui, name: &str, value: &str, colour: Color32) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(name)
                .font(theme::small())
                .color(palette::text_secondary()),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).font(theme::small()).color(colour));
        });
    });
}

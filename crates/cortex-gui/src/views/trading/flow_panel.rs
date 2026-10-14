//! The order-flow tab: the book's depth, what it shows about pressure,
//! and the ladder itself.

use cortex_market::{Market, Snapshot, dhan::depth::DepthMode};
use eframe::egui::{self, CornerRadius, RichText, Stroke, Ui, vec2};

use super::tables::{self, cell, tinted};
use super::widgets;
use crate::theme::{self, palette};

const DEPTHS: [(DepthMode, &str); 3] = [
    (DepthMode::Five, "5 levels"),
    (DepthMode::Twenty, "20 levels"),
    (DepthMode::TwoHundred, "200 levels"),
];

pub fn show(ui: &mut Ui, market: &Market, snapshot: &Snapshot) {
    let book = &snapshot.depth;
    let now = cortex_market::clock::now_ms();
    let flow = book.features(now);

    ui.add_space(24.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;

        ui.label(RichText::new("Order flow").font(theme::strong(16.0)))
            .on_hover_text(
                "Aggregated displayed liquidity; trade direction and cancellation intent are not observed.",
            );

        ui.label(
            RichText::new(&book.status)
                .font(theme::small())
                .color(palette::text_muted()),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let names: Vec<&str> = DEPTHS.iter().map(|(_, name)| *name).collect();
            let chosen = DEPTHS
                .iter()
                .position(|(mode, _)| *mode == market.depth_mode());

            if let Some(at) = widgets::segmented(ui, &names, chosen) {
                market.set_depth_mode(DEPTHS[at].0);
            }
        });
    });

    ui.add_space(12.0);

    if !flow.available {
        widgets::banner(
            ui,
            "Depth partial, stale or unavailable; flow signals abstain.",
            palette::warning(),
        );
        ui.add_space(12.0);
    }

    // The numbers, as tiles.
    let mut tiles: Vec<(&str, String)> = Vec::new();

    if let Some(spread) = flow.spread {
        tiles.push(("Spread", format!("{spread:.4}")));
    }

    if let Some(imbalance) = flow.imbalance {
        tiles.push(("Imbalance", format!("{:.1}%", imbalance * 100.0)));
    }

    if let Some(microprice) = flow.microprice {
        tiles.push(("Microprice", format!("{microprice:.4}")));
    }

    if book.bid_ms > 0 || book.ask_ms > 0 {
        tiles.push((
            "Bid / ask age",
            format!("{} / {} ms", now - book.bid_ms, now - book.ask_ms),
        ));
    }

    if let Some(latest) = snapshot.flow_history.last() {
        tiles.push((
            "Window volume Δ",
            latest
                .volume_delta
                .map_or("—".into(), |value| format!("{value:.0}")),
        ));
        tiles.push((
            "Liquidity change",
            latest
                .displayed_change
                .map_or("—".into(), |value| format!("{value:+.0}")),
        ));
    }

    if !tiles.is_empty() {
        let width = ui.available_width();
        let columns = ((width + 16.0) / 196.0).floor().clamp(1.0, 6.0) as usize;
        let tile_width = (width - 16.0 * (columns - 1) as f32) / columns as f32;

        for chunk in tiles.chunks(columns) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;

                for (name, value) in chunk {
                    ui.allocate_ui(vec2(tile_width, 0.0), |ui| {
                        widgets::card().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 4.0;
                            widgets::caption(ui, name);
                            ui.label(
                                RichText::new(value)
                                    .font(theme::strong(16.0))
                                    .color(palette::text()),
                            );
                        });
                    });
                }
            });

            ui.add_space(16.0);
        }
    }

    // Imbalance over the recent windows.
    if snapshot.flow_history.len() > 1 {
        widgets::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::caption(ui, "Depth imbalance · recent windows");
            ui.add_space(8.0);

            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 96.0), egui::Sense::hover());

            ui.painter()
                .rect_filled(rect, CornerRadius::same(8), palette::sunken());
            ui.painter().hline(
                rect.x_range(),
                rect.center().y,
                Stroke::new(1.0, palette::border()),
            );

            let count = snapshot.flow_history.len();
            let mut previous = None;

            for (at, sample) in snapshot.flow_history.iter().enumerate() {
                let point = sample.imbalance.map(|imbalance| {
                    egui::pos2(
                        rect.left() + 8.0 + at as f32 / (count - 1) as f32 * (rect.width() - 16.0),
                        rect.center().y - imbalance as f32 * rect.height() * 0.42,
                    )
                });

                if let (Some(before), Some(point)) = (previous, point) {
                    ui.painter()
                        .line_segment([before, point], Stroke::new(1.5, palette::accent()));
                }

                previous = point;
            }
        });

        ui.add_space(16.0);
    }

    // The ladder.
    let rows = (0..book.bids.len().max(book.asks.len()).min(200))
        .map(|at| {
            let mut row = Vec::with_capacity(6);

            match book.bids.get(at) {
                Some(bid) => {
                    row.push(cell(bid.orders.to_string()));
                    row.push(cell(format!("{:.0}", bid.quantity)));
                    row.push(tinted(format!("{:.2}", bid.price), palette::good()));
                }
                None => row.extend((0..3).map(|_| tinted("—", palette::text_muted()))),
            }

            match book.asks.get(at) {
                Some(ask) => {
                    row.push(tinted(format!("{:.2}", ask.price), palette::bad()));
                    row.push(cell(format!("{:.0}", ask.quantity)));
                    row.push(cell(ask.orders.to_string()));
                }
                None => row.extend((0..3).map(|_| tinted("—", palette::text_muted()))),
            }

            row
        })
        .collect();

    tables::table(
        ui,
        &[1.0; 6],
        &[
            "Bid orders",
            "Bid qty",
            "Bid price",
            "Ask price",
            "Ask qty",
            "Ask orders",
        ],
        rows,
    );

    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs(1));
}

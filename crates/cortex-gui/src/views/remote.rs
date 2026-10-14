//! The remote control part of the settings page: the switch, the pairing
//! code and the phones that are let in.

use eframe::egui::{self, CornerRadius, RichText, Stroke, Ui, vec2};
use qrcode::{Color, EcLevel, QrCode};

use super::settings::{caption, section, switch_row};
use crate::remote::{self, Hosting};
use crate::theme::{self, palette};

/// The side of the pairing code's picture.
const CODE_SIZE: f32 = 236.0;

/// White around the code, in modules. Scanners need it to find the code.
const QUIET: f32 = 3.0;

/// Returns the switch's new position when it was flipped.
pub fn show(ui: &mut Ui, mut on: bool) -> Option<bool> {
    let bridge = remote::bridge()?;

    caption(
        ui,
        "Encrypted end to end, no open port. The agent and your keys stay on this computer.",
    );

    let flipped = switch_row(
        ui,
        &mut on,
        RichText::new("Let paired phones connect"),
        "A paired phone can do everything you can here.",
        true,
    );

    if !on {
        return flipped.then_some(on);
    }

    match bridge.hosting() {
        Hosting::Off | Hosting::Starting => {
            ui.add_space(crate::theme::space::S);
            caption(ui, "Starting…");
        }

        Hosting::Failed(problem) => {
            ui.add_space(crate::theme::space::S);
            ui.label(RichText::new(problem).color(palette::bad()));
        }

        Hosting::On(_) => {
            pairing(ui, bridge);
            devices(ui, bridge);
        }
    }

    flipped.then_some(on)
}

fn pairing(ui: &mut Ui, bridge: &'static remote::Bridge) {
    section(ui, "Pair a phone");

    let Some((code, left)) = bridge.code() else {
        caption(
            ui,
            "On the phone, choose \"Pair with a computer\". A code admits one phone, for ten \
             minutes.",
        );

        ui.add_space(crate::theme::space::S);

        if ui
            .add(crate::theme::secondary_button("Show a pairing code"))
            .clicked()
        {
            bridge.show_code();
        }

        return;
    };

    caption(
        ui,
        &format!(
            "Scan with the Cortex app. Works once, for {} more min.",
            left.as_secs() / 60 + 1
        ),
    );

    ui.add_space(crate::theme::space::M);

    draw_code(ui, &code);

    ui.add_space(crate::theme::space::M);

    ui.horizontal(|ui| {
        if ui
            .add(crate::theme::secondary_button("Copy as text"))
            .on_hover_text("For a phone whose camera cannot be used: paste it into the app.")
            .clicked()
        {
            ui.ctx().copy_text(code.clone());
        }

        if ui.add(crate::theme::secondary_button("New code")).clicked() {
            bridge.show_code();
        }

        if ui.add(crate::theme::secondary_button("Hide")).clicked() {
            bridge.hide_code();
        }
    });

    // The minutes left count down while nothing else moves.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs(5));
}

/// Draws the code dark on white whatever the theme: that is what
/// cameras read best.
fn draw_code(ui: &mut Ui, code: &str) {
    let Ok(picture) = QrCode::with_error_correction_level(code.as_bytes(), EcLevel::L) else {
        ui.label(
            RichText::new("The code could not be drawn. Copy it as text instead.")
                .color(palette::bad()),
        );

        return;
    };

    let modules = picture.width();
    let colors = picture.to_colors();

    let (rect, _) = ui.allocate_exact_size(vec2(CODE_SIZE, CODE_SIZE), egui::Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(
        rect,
        CornerRadius::same(crate::theme::radius::L),
        palette::CODE_PAPER,
    );

    let step = CODE_SIZE / (modules as f32 + 2.0 * QUIET);
    let origin = rect.min + vec2(step * QUIET, step * QUIET);

    for (index, color) in colors.iter().enumerate() {
        if *color != Color::Dark {
            continue;
        }

        let column = (index % modules) as f32;
        let row = (index / modules) as f32;

        let module = egui::Rect::from_min_size(
            origin + vec2(column * step, row * step),
            // A hair wider than the step, so no line shows between modules.
            vec2(step + 0.5, step + 0.5),
        );

        painter.rect_filled(module, CornerRadius::ZERO, palette::CODE_INK);
    }
}

fn devices(ui: &mut Ui, bridge: &'static remote::Bridge) {
    section(ui, "Paired phones");

    let devices = bridge.devices();

    if devices.is_empty() {
        caption(ui, "None yet.");

        return;
    }

    for (device, connected) in devices {
        ui.add_space(crate::theme::space::S);

        egui::Frame::new()
            .fill(palette::surface())
            .stroke(Stroke::new(1.0, palette::border()))
            .corner_radius(CornerRadius::same(crate::theme::radius::L))
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());

                    let colour = if connected {
                        palette::good()
                    } else {
                        palette::text_muted()
                    };

                    ui.painter().circle_filled(dot.center(), 4.0, colour);

                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;

                        ui.label(RichText::new(&device.name).color(palette::text()));

                        let state = if connected {
                            "Connected"
                        } else {
                            "Not connected"
                        };

                        ui.label(
                            RichText::new(state)
                                .font(theme::small())
                                .color(palette::text_muted()),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(crate::theme::secondary_button("Forget"))
                            .on_hover_text("Stop letting this phone connect.")
                            .clicked()
                        {
                            bridge.forget(&device.id);
                        }
                    });
                });
            });
    }
}

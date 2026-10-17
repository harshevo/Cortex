//! The task list pinned above the composer: the plan the model keeps
//! with `update_plan`, step by step.

use cortex_core::plan::{Plan, Status};
use eframe::egui::{self, CornerRadius, RichText, Sense, Stroke, Ui};

use super::icons;
use crate::theme::{self, palette, radius, space};

const ROW_HEIGHT: f32 = 24.0;

/// Steps shown before the rest are folded into a count.
const MAX_ROWS: usize = 8;

/// Draws `plan`. `open` says whether every step is listed or only the
/// one in progress; a click on the header changes it.
pub fn show(ui: &mut Ui, plan: &Plan, open: &mut bool, running: bool) {
    egui::Frame::new()
        .fill(palette::surface())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(radius::L))
        .inner_margin(egui::Margin::symmetric(space::M as i8, space::S as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;

            if header(ui, plan, *open).clicked() {
                *open = !*open;
            }

            if *open {
                ui.add_space(space::XS);

                for step in plan.steps.iter().take(MAX_ROWS) {
                    row(ui, &step.step, step.status, running);
                }

                let more = plan.steps.len().saturating_sub(MAX_ROWS);

                if more > 0 {
                    ui.label(
                        RichText::new(format!("{more} more"))
                            .font(theme::small())
                            .color(palette::text_muted()),
                    );
                }
            } else if let Some(step) = plan.current() {
                row(ui, &step.step, step.status, running);
            }
        });
}

fn header(ui: &mut Ui, plan: &Plan, open: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());

    let painter = ui.painter();
    let middle = rect.center().y;

    let title = painter.text(
        egui::pos2(rect.left(), middle),
        egui::Align2::LEFT_CENTER,
        "Tasks",
        theme::strong(theme::size::UI),
        palette::text(),
    );

    painter.text(
        egui::pos2(title.right() + space::S, middle),
        egui::Align2::LEFT_CENTER,
        format!("{}/{}", plan.completed(), plan.steps.len()),
        theme::ui(),
        palette::text_muted(),
    );

    // Pointing down when open, right when folded.
    let chevron = egui::pos2(rect.right() - space::S, middle);

    if open {
        icons::chevron(ui, chevron, palette::text_muted());
    } else {
        let size = 3.5;

        ui.painter().line(
            vec![
                chevron + egui::vec2(-size / 2.0, -size),
                chevron + egui::vec2(size / 2.0, 0.0),
                chevron + egui::vec2(-size / 2.0, size),
            ],
            Stroke::new(1.5, palette::text_muted()),
        );
    }

    let mut response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if !plan.goal.trim().is_empty() || !plan.acceptance.is_empty() {
        response = response.on_hover_ui(|ui| {
            ui.set_max_width(360.0);

            if !plan.goal.trim().is_empty() {
                ui.label(RichText::new(plan.goal.trim()).font(theme::ui()));
            }

            if !plan.acceptance.is_empty() {
                ui.add_space(space::XS);
                ui.label(
                    RichText::new("Done when")
                        .font(theme::small())
                        .color(palette::text_muted()),
                );

                for criterion in &plan.acceptance {
                    ui.label(RichText::new(format!("· {}", criterion.trim())).font(theme::ui()));
                }
            }
        });
    }

    response
}

fn row(ui: &mut Ui, text: &str, status: Status, running: bool) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());

    let middle = rect.center().y;
    let mark = egui::pos2(rect.left() + 6.0, middle);

    let painter = ui.painter();

    let color = match status {
        Status::Pending => {
            painter.circle_stroke(mark, 5.0, Stroke::new(1.25, palette::text_muted()));

            palette::text_secondary()
        }

        Status::InProgress => {
            painter.circle_stroke(mark, 5.0, Stroke::new(1.25, palette::accent()));

            if running {
                icons::activity_dot_sized(ui, mark, 2.5);
            } else {
                painter.circle_filled(mark, 2.5, palette::accent());
            }

            palette::text()
        }

        Status::Completed => {
            painter.circle_filled(mark, 5.5, palette::text_muted());
            painter.line(
                vec![
                    mark + egui::vec2(-2.5, 0.0),
                    mark + egui::vec2(-0.7, 1.8),
                    mark + egui::vec2(2.6, -1.8),
                ],
                Stroke::new(1.4, palette::surface()),
            );

            palette::text_muted()
        }

        Status::Blocked => {
            painter.circle_stroke(mark, 5.0, Stroke::new(1.25, palette::bad()));
            painter.line_segment(
                [mark + egui::vec2(-2.5, -2.5), mark + egui::vec2(2.5, 2.5)],
                Stroke::new(1.25, palette::bad()),
            );

            palette::bad()
        }
    };

    let left = rect.left() + 6.0 + space::M + 2.0;
    let room = rect.right() - left;
    let shown = crate::format::one_line(text.trim(), (room / 6.5).max(4.0) as usize);

    let galley = ui.painter().layout_no_wrap(shown, theme::ui(), color);

    let at = egui::pos2(left, middle - galley.size().y / 2.0);
    let width = galley.size().x;

    ui.painter().galley(at, galley, color);

    if status == Status::Completed {
        ui.painter().line_segment(
            [egui::pos2(left, middle), egui::pos2(left + width, middle)],
            Stroke::new(1.0, palette::text_muted()),
        );
    }
}

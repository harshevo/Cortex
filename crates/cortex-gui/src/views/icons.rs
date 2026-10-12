//! The app's few icons, drawn as lines so they stay crisp at any scale
//! and need no icon font.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, vec2};

use crate::theme::palette;

/// The size of the square an icon is drawn in.
const ICON_SIZE: f32 = 14.0;

/// The size of the square that responds to the pointer.
const BUTTON_SIZE: f32 = 22.0;

const LINE_WIDTH: f32 = 1.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Plus,
    Copy,
    Back,
    Trash,
    Folder,
    FolderPlus,
    Gear,
    Sidebar,
    Chat,
    Code,
}

pub fn activity_dot(ui: &Ui, centre: Pos2) {
    let phase = ui.input(|input| input.time) as f32 * std::f32::consts::TAU / 1.8;
    let alpha = 0.35 + 0.65 * (phase.sin() + 1.0) / 2.0;
    ui.painter()
        .circle_filled(centre, 3.0, palette::ACCENT.gamma_multiply(alpha));
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}

/// A small square button with an icon. `danger` tints it red on hover,
/// for buttons that delete.
pub fn button(ui: &mut Ui, icon: Icon, tooltip: &str, danger: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON_SIZE, BUTTON_SIZE), Sense::click());

    paint_button(ui, rect, icon, &response, danger);

    response.on_hover_text(tooltip)
}

/// The same button at a place the caller has already chosen, for buttons
/// that sit on top of a row.
pub fn button_at(
    ui: &Ui,
    rect: Rect,
    id: egui::Id,
    icon: Icon,
    tooltip: &str,
    danger: bool,
) -> egui::Response {
    let response = ui.interact(rect, id, Sense::click());

    paint_button(ui, rect, icon, &response, danger);

    response.on_hover_text(tooltip)
}

/// The square a button occupies when it is the `position`th from the
/// right-hand end of `row`.
pub fn slot_from_right(row: Rect, position: usize) -> Rect {
    let right = row.right() - 6.0 - position as f32 * (BUTTON_SIZE + 2.0);

    Rect::from_center_size(
        Pos2::new(right - BUTTON_SIZE / 2.0, row.center().y),
        vec2(BUTTON_SIZE, BUTTON_SIZE),
    )
}

fn paint_button(ui: &Ui, rect: Rect, icon: Icon, response: &egui::Response, danger: bool) {
    let color = match (response.hovered(), danger) {
        (true, true) => palette::BAD,
        (true, false) => palette::TEXT,
        (false, _) => palette::TEXT_MUTED,
    };

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), palette::RAISED);
    }

    paint(ui, rect.center(), icon, color);
}

/// Draws `icon` centred on `centre`.
pub fn paint(ui: &Ui, centre: Pos2, icon: Icon, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(LINE_WIDTH, color);

    // Icons are designed on a grid from -1 to 1.
    let at = |x: f32, y: f32| centre + vec2(x, y) * (ICON_SIZE / 2.0);
    let line = |from: Pos2, to: Pos2| {
        painter.line_segment([from, to], stroke);
    };

    match icon {
        Icon::Back => {
            line(at(0.85, 0.0), at(-0.85, 0.0));
            line(at(-0.85, 0.0), at(-0.2, -0.65));
            line(at(-0.85, 0.0), at(-0.2, 0.65));
        }
        Icon::Copy => {
            painter.rect_stroke(
                Rect::from_min_max(at(-0.35, -0.35), at(0.8, 0.85)),
                CornerRadius::same(1),
                stroke,
                egui::StrokeKind::Middle,
            );
            line(at(-0.8, 0.35), at(-0.8, -0.85));
            line(at(-0.8, -0.85), at(0.35, -0.85));
        }
        Icon::Plus => {
            line(at(-0.8, 0.0), at(0.8, 0.0));
            line(at(0.0, -0.8), at(0.0, 0.8));
        }

        Icon::Trash => {
            // Lid and handle.
            line(at(-0.9, -0.55), at(0.9, -0.55));
            line(at(-0.3, -0.55), at(-0.3, -0.9));
            line(at(-0.3, -0.9), at(0.3, -0.9));
            line(at(0.3, -0.9), at(0.3, -0.55));

            // Can.
            line(at(-0.65, -0.55), at(-0.55, 0.9));
            line(at(-0.55, 0.9), at(0.55, 0.9));
            line(at(0.55, 0.9), at(0.65, -0.55));

            line(at(-0.2, -0.2), at(-0.2, 0.55));
            line(at(0.2, -0.2), at(0.2, 0.55));
        }

        Icon::Folder => folder(&line, &at),

        Icon::FolderPlus => {
            folder(&line, &at);

            line(at(-0.35, 0.15), at(0.35, 0.15));
            line(at(0.0, -0.2), at(0.0, 0.5));
        }

        Icon::Gear => {
            painter.circle_stroke(centre, ICON_SIZE * 0.2, stroke);
            painter.circle_stroke(centre, ICON_SIZE * 0.42, stroke);

            for tooth in 0..8 {
                let angle = tooth as f32 * std::f32::consts::TAU / 8.0;
                let direction = vec2(angle.cos(), angle.sin());

                line(
                    centre + direction * ICON_SIZE * 0.42,
                    centre + direction * ICON_SIZE * 0.58,
                );
            }
        }

        // A window with its left-hand column marked off.
        Icon::Sidebar => {
            painter.rect_stroke(
                Rect::from_min_max(at(-0.95, -0.75), at(0.95, 0.75)),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );

            line(at(-0.3, -0.75), at(-0.3, 0.75));
        }

        // A speech bubble with its tail at the bottom left.
        Icon::Chat => {
            let outline = [
                (-0.9, -0.8),
                (0.9, -0.8),
                (0.9, 0.4),
                (-0.1, 0.4),
                (-0.6, 0.9),
                (-0.6, 0.4),
                (-0.9, 0.4),
                (-0.9, -0.8),
            ];

            for pair in outline.windows(2) {
                line(at(pair[0].0, pair[0].1), at(pair[1].0, pair[1].1));
            }
        }

        Icon::Code => {
            line(at(-0.45, -0.5), at(-0.95, 0.0));
            line(at(-0.95, 0.0), at(-0.45, 0.5));

            line(at(0.45, -0.5), at(0.95, 0.0));
            line(at(0.95, 0.0), at(0.45, 0.5));

            line(at(0.2, -0.8), at(-0.2, 0.8));
        }
    }
}

/// The outline of a folder: a body with a tab on its top-left corner.
fn folder(line: &impl Fn(Pos2, Pos2), at: &impl Fn(f32, f32) -> Pos2) {
    let outline = [
        (-0.95, -0.7),
        (-0.3, -0.7),
        (-0.05, -0.4),
        (0.95, -0.4),
        (0.95, 0.8),
        (-0.95, 0.8),
        (-0.95, -0.7),
    ];

    for pair in outline.windows(2) {
        line(at(pair[0].0, pair[0].1), at(pair[1].0, pair[1].1));
    }
}

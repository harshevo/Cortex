//! The trading screen's own controls: one button style in three
//! weights, square icon buttons, a segmented control, and dropdown
//! (or drop-up) menus for wherever more than three choices sit together.
//!
//! Sizes follow an 8px scale: controls are 32 tall, padded 12 across.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Pos2, Rect, RectAlign, Response, Sense, Stroke, Ui, pos2,
    vec2,
};

use crate::theme::{self, palette};
use crate::views::icons::{self, Icon};

/// The height of every control.
pub const HEIGHT: f32 = 32.0;

/// The rounding of every control.
pub const RADIUS: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Filled with the accent: the one thing to do here.
    Primary,
    /// Bordered on the surface.
    Secondary,
    /// No ground until hovered.
    Ghost,
}

/// What a button shows beside its text.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    App(Icon),
    Dots,
    Refresh,
    Send,
    Stop,
    Chevron,
    Import,
    Panel,
    Link,
}

/// Colours of a control in its current state: ground, ink, outline.
fn colours(
    kind: Kind,
    response: &Response,
    enabled: bool,
    active: bool,
) -> (Color32, Color32, Stroke) {
    let hovered = enabled && response.hovered();
    let pressed = enabled && response.is_pointer_button_down_on();

    match kind {
        Kind::Primary => {
            let mut fill = palette::accent();

            if !enabled {
                fill = palette::raised();
            } else if pressed {
                fill = fill.lerp_to_gamma(Color32::BLACK, 0.15);
            } else if hovered {
                fill = fill.lerp_to_gamma(Color32::WHITE, 0.12);
            }

            let ink = if enabled {
                palette::background()
            } else {
                palette::text_muted()
            };

            (fill, ink, Stroke::NONE)
        }
        Kind::Secondary | Kind::Ghost => {
            let fill = if pressed {
                palette::raised().lerp_to_gamma(palette::border(), 0.5)
            } else if hovered || active {
                palette::raised()
            } else if kind == Kind::Secondary {
                palette::surface()
            } else {
                Color32::TRANSPARENT
            };

            let ink = if !enabled {
                palette::text_muted()
            } else if active {
                palette::accent()
            } else if hovered {
                palette::text()
            } else {
                palette::text_secondary()
            };

            let stroke = if kind == Kind::Secondary {
                Stroke::new(1.0, palette::border())
            } else {
                Stroke::NONE
            };

            (fill, ink, stroke)
        }
    }
}

fn finish(ui: &Ui, rect: Rect, response: Response, enabled: bool) -> Response {
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(RADIUS),
            theme::focus_ring(),
            egui::StrokeKind::Outside,
        );
    }

    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// A button with text and, if given, a glyph before it.
pub fn button(ui: &mut Ui, text: &str, glyph: Option<Glyph>, kind: Kind) -> Response {
    button_enabled(ui, true, text, glyph, kind)
}

pub fn button_enabled(
    ui: &mut Ui,
    enabled: bool,
    text: &str,
    glyph: Option<Glyph>,
    kind: Kind,
) -> Response {
    labelled(ui, enabled, text, glyph, None, kind, false)
}

/// The common body of text buttons: a leading glyph, the text, and a
/// trailing glyph (a dropdown's chevron).
fn labelled(
    ui: &mut Ui,
    enabled: bool,
    text: &str,
    leading: Option<Glyph>,
    trailing: Option<Glyph>,
    kind: Kind,
    active: bool,
) -> Response {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), theme::strong(13.0), palette::text());

    let glyph_width = 16.0;
    let gap = 8.0;
    let mut width = 12.0 + galley.size().x + 12.0;

    if leading.is_some() {
        width += glyph_width + gap;
    }

    if trailing.is_some() {
        width += glyph_width + 4.0;
    }

    let (rect, response) = ui.allocate_exact_size(
        vec2(width, HEIGHT),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );

    let (fill, ink, stroke) = colours(kind, &response, enabled, active);

    ui.painter().rect(
        rect,
        CornerRadius::same(RADIUS),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );

    let mut left = rect.left() + 12.0;

    if let Some(glyph) = leading {
        paint(
            ui,
            pos2(left + glyph_width / 2.0, rect.center().y),
            glyph,
            ink,
        );
        left += glyph_width + gap;
    }

    ui.painter().galley(
        pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );

    if let Some(glyph) = trailing {
        paint(
            ui,
            pos2(
                rect.right() - 10.0 - glyph_width / 2.0 + 4.0,
                rect.center().y,
            ),
            glyph,
            ink,
        );
    }

    finish(ui, rect, response, enabled)
}

/// A square button with a glyph; `active` marks a switch that is on.
pub fn icon_button(ui: &mut Ui, glyph: Glyph, tooltip: &str, active: bool) -> Response {
    icon_button_kind(ui, glyph, tooltip, active, Kind::Ghost, true)
}

pub fn icon_button_kind(
    ui: &mut Ui,
    glyph: Glyph,
    tooltip: &str,
    active: bool,
    kind: Kind,
    enabled: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        vec2(HEIGHT, HEIGHT),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );

    let (fill, ink, stroke) = colours(kind, &response, enabled, active);

    ui.painter().rect(
        rect,
        CornerRadius::same(RADIUS),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );

    paint(ui, rect.center(), glyph, ink);

    finish(ui, rect, response, enabled).on_hover_text(tooltip)
}

/// A button that opens a menu below it, or above it when `up`.
pub fn dropdown<R>(
    ui: &mut Ui,
    text: &str,
    leading: Option<Glyph>,
    kind: Kind,
    active: bool,
    up: bool,
    contents: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let response = labelled(ui, true, text, leading, Some(Glyph::Chevron), kind, active);
    let inner = menu(&response, up, contents);

    (response, inner)
}

/// The "⋯" button and its menu, holding whatever does not fit beside it.
pub fn overflow<R>(
    ui: &mut Ui,
    tooltip: &str,
    up: bool,
    contents: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let response = icon_button(ui, Glyph::Dots, tooltip, false);
    let inner = menu(&response, up, contents);

    (response, inner)
}

/// The menu a button opens: a raised panel with roomy rows.
pub fn menu<R>(response: &Response, up: bool, contents: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let aligns: &[RectAlign] = if up {
        &[
            RectAlign::TOP_END,
            RectAlign::TOP_START,
            RectAlign::BOTTOM_END,
        ]
    } else {
        &[
            RectAlign::BOTTOM_END,
            RectAlign::BOTTOM_START,
            RectAlign::TOP_END,
        ]
    };

    egui::Popup::menu(response)
        .align(aligns[0])
        .align_alternatives(&aligns[1..])
        .gap(4.0)
        .frame(
            egui::Frame::new()
                .fill(palette::surface())
                .stroke(Stroke::new(1.0, palette::border()))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(6))
                .shadow(egui::Shadow {
                    offset: [0, 6],
                    blur: 18,
                    spread: 0,
                    color: Color32::from_black_alpha(80),
                }),
        )
        .show(|ui| {
            ui.set_min_width(200.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            contents(ui)
        })
        .map(|inner| inner.inner)
}

/// One row of a menu: a glyph, its text, a muted line under it if
/// given, and a dot when it is the one chosen.
pub fn menu_item(
    ui: &mut Ui,
    text: &str,
    glyph: Option<Glyph>,
    detail: Option<&str>,
    chosen: bool,
) -> Response {
    let width = ui.available_width().max(200.0);
    let detail_galley = detail.map(|detail| {
        ui.painter().layout(
            detail.to_string(),
            theme::small(),
            palette::text_muted(),
            width - 48.0,
        )
    });
    let height = 32.0
        + detail_galley
            .as_ref()
            .map_or(0.0, |galley| galley.size().y + 2.0);

    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), palette::raised());
    }

    let ink = if chosen {
        palette::accent()
    } else if response.hovered() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    let mut left = rect.left() + 10.0;
    let line = rect.top() + 16.0;

    if let Some(glyph) = glyph {
        paint(ui, pos2(left + 7.0, line), glyph, ink);
        left += 24.0;
    }

    ui.painter().text(
        pos2(left, line),
        Align2::LEFT_CENTER,
        text,
        theme::strong(13.0),
        ink,
    );

    if let Some(galley) = detail_galley {
        ui.painter()
            .galley(pos2(left, line + 10.0), galley, palette::text_muted());
    }

    if chosen {
        ui.painter()
            .circle_filled(pos2(rect.right() - 14.0, line), 3.0, palette::accent());
    }

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A thin line between groups of a menu.
pub fn menu_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());

    ui.painter().hline(
        rect.x_range().shrink(6.0),
        rect.center().y,
        Stroke::new(1.0, palette::border()),
    );
}

/// A row of choices on one sunken track, the chosen one raised. Returns
/// the one clicked.
pub fn segmented(ui: &mut Ui, items: &[&str], chosen: Option<usize>) -> Option<usize> {
    let mut clicked = None;

    egui::Frame::new()
        .fill(palette::sunken())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(RADIUS + 2))
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;

            ui.horizontal(|ui| {
                for (at, item) in items.iter().enumerate() {
                    let on = chosen == Some(at);

                    if segment(ui, item, on, None).clicked() {
                        clicked = Some(at);
                    }
                }
            });
        });

    clicked
}

/// One segment of the track. With `menu`, it shows a chevron and is the
/// button for that menu.
pub fn segment(ui: &mut Ui, text: &str, on: bool, trailing: Option<Glyph>) -> Response {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), theme::strong(13.0), palette::text());
    let extra = if trailing.is_some() { 18.0 } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(
        vec2(galley.size().x + 28.0 + extra, HEIGHT - 4.0),
        Sense::click(),
    );

    let (fill, ink) = if on {
        (palette::raised(), palette::text())
    } else if response.hovered() {
        (palette::surface(), palette::text())
    } else {
        (Color32::TRANSPARENT, palette::text_muted())
    };

    ui.painter().rect(
        rect,
        CornerRadius::same(RADIUS),
        fill,
        if on {
            Stroke::new(1.0, palette::border())
        } else {
            Stroke::NONE
        },
        egui::StrokeKind::Inside,
    );

    ui.painter().galley(
        pos2(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );

    if let Some(glyph) = trailing {
        paint(ui, pos2(rect.right() - 16.0, rect.center().y), glyph, ink);
    }

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small rounded label with a coloured dot, for a state.
pub fn status(ui: &mut Ui, text: &str, colour: Color32) -> Response {
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.12))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;

            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());

                ui.painter().circle_filled(dot.center(), 3.5, colour);
                ui.label(
                    egui::RichText::new(text)
                        .font(theme::strong(12.0))
                        .color(colour),
                );
            });
        })
        .response
}

/// A strip for a problem or a warning: tinted, with the text wrapped.
pub fn banner(ui: &mut Ui, text: &str, colour: Color32) {
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, colour.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(text).font(theme::small()).color(colour));
        });
}

/// The small muted heading over a group.
pub fn caption(ui: &mut Ui, text: &str) -> Response {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(theme::strong(11.0))
            .color(palette::text_muted())
            .extra_letter_spacing(0.6),
    )
}

/// Draws `glyph` centred on `centre`.
pub fn paint(ui: &Ui, centre: Pos2, glyph: Glyph, colour: Color32) {
    let painter = ui.painter();
    let line = Stroke::new(1.5, colour);
    let c = centre;

    match glyph {
        Glyph::App(icon) => icons::paint(ui, centre, icon, colour),
        Glyph::Dots => {
            for dx in [-5.0, 0.0, 5.0] {
                painter.circle_filled(c + vec2(dx, 0.0), 1.6, colour);
            }
        }
        Glyph::Refresh => {
            let radius = 5.5;
            let points: Vec<Pos2> = (0..=20)
                .map(|step| {
                    let angle = 0.5 + step as f32 / 20.0 * 4.9;
                    c + vec2(angle.cos(), angle.sin()) * radius
                })
                .collect();
            let end = *points.last().unwrap();

            painter.add(egui::Shape::line(points, line));
            painter.line_segment([end, end + vec2(3.5, -0.5)], line);
            painter.line_segment([end, end + vec2(0.5, 3.5)], line);
        }
        Glyph::Send => {
            painter.line_segment([c + vec2(0.0, 6.0), c + vec2(0.0, -6.0)], line);
            painter.line_segment([c + vec2(-4.5, -1.5), c + vec2(0.0, -6.0)], line);
            painter.line_segment([c + vec2(4.5, -1.5), c + vec2(0.0, -6.0)], line);
        }
        Glyph::Stop => {
            painter.rect_filled(
                Rect::from_center_size(c, vec2(9.0, 9.0)),
                CornerRadius::same(2),
                colour,
            );
        }
        Glyph::Chevron => {
            painter.line_segment([c + vec2(-3.5, -1.5), c + vec2(0.0, 2.0)], line);
            painter.line_segment([c + vec2(3.5, -1.5), c + vec2(0.0, 2.0)], line);
        }
        Glyph::Import => {
            painter.line_segment([c + vec2(0.0, -6.0), c + vec2(0.0, 3.0)], line);
            painter.line_segment([c + vec2(-3.5, -0.5), c + vec2(0.0, 3.0)], line);
            painter.line_segment([c + vec2(3.5, -0.5), c + vec2(0.0, 3.0)], line);
            painter.add(egui::Shape::line(
                vec![
                    c + vec2(-6.0, 2.0),
                    c + vec2(-6.0, 6.0),
                    c + vec2(6.0, 6.0),
                    c + vec2(6.0, 2.0),
                ],
                line,
            ));
        }
        Glyph::Panel => {
            let rect = Rect::from_center_size(c, vec2(14.0, 12.0));

            painter.rect_stroke(rect, CornerRadius::same(2), line, egui::StrokeKind::Middle);
            painter.line_segment(
                [
                    pos2(rect.right() - 5.0, rect.top()),
                    pos2(rect.right() - 5.0, rect.bottom()),
                ],
                line,
            );
        }
        Glyph::Link => {
            painter.circle_stroke(c + vec2(-2.5, 0.0), 3.5, line);
            painter.circle_stroke(c + vec2(2.5, 0.0), 3.5, line);
        }
    }
}

/// The frame of a card on the trading screen.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(palette::surface())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
}

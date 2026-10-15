//! How the app looks: colours, fonts, spacing.

pub mod fonts;
pub mod palette;

use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, Stroke, TextStyle};

/// Corner radius of cards and bubbles.
pub const CARD_RADIUS: u8 = radius::L;

/// The steps every gap and padding is taken from, in points: an 8-point
/// grid, with 2 and 4 for hairlines and the insides of small controls.
/// Keeping to them is what makes rows, cards and columns line up across
/// views.
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Heights of the things that are pressed, so that a row of controls
/// shares one centre line.
pub mod control {
    /// Small icon buttons and the chips in bars.
    pub const SMALL: f32 = 24.0;

    /// Ordinary buttons, fields and menu rows.
    pub const MEDIUM: f32 = 28.0;

    /// The main action of a page.
    pub const LARGE: f32 = 36.0;
}

/// Corner radii, smallest to largest: marks, chips, rows and buttons,
/// cards, and the composer and bubbles.
pub mod radius {
    pub const XS: u8 = 2;
    pub const S: u8 = 4;
    pub const M: u8 = 6;
    pub const L: u8 = 10;
    pub const XL: u8 = 14;
}

/// Type sizes, in points.
pub mod size {
    /// Section names set in capitals-like weight above a list.
    pub const CAPTION: f32 = 11.5;

    /// Help text, meta lines and the stats strip.
    pub const SMALL: f32 = 12.0;

    /// Rows, buttons and everything else the interface says.
    pub const UI: f32 = 13.0;

    /// What is read at length: the conversation.
    pub const BODY: f32 = 14.0;

    pub const TITLE: f32 = 16.0;
    pub const HEADING: f32 = 20.0;
    pub const DISPLAY: f32 = 24.0;
}

/// Height of a line of running text, as a multiple of its size.
pub const LINE_HEIGHT: f32 = 1.5;

/// Longest line of the assistant's prose, in characters of the body
/// font. Past about eighty the eye loses its place on the way back.
pub const MEASURE_CHARS: f32 = 78.0;

/// Widest the conversation column grows; beyond this, long lines are
/// tiring to read.
pub const COLUMN_WIDTH: f32 = 760.0;

/// Sets the app up to draw: its fonts, and the colours of the theme in
/// use.
pub fn apply(context: &egui::Context) {
    fonts::install(context);

    restyle(context);
}

/// Takes over the colours of the theme in use. Called again whenever
/// another theme is chosen.
pub fn restyle(context: &egui::Context) {
    let dark = palette::current().dark;

    context.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });

    context.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, strong(size::HEADING)),
            (TextStyle::Body, FontId::proportional(size::BODY)),
            (TextStyle::Button, FontId::proportional(size::UI)),
            (TextStyle::Small, FontId::proportional(size::SMALL)),
            (TextStyle::Monospace, FontId::monospace(size::SMALL)),
        ]
        .into();

        style.spacing.item_spacing = egui::vec2(space::S, space::S);
        style.spacing.button_padding = egui::vec2(space::M, space::XS + 1.0);
        style.spacing.interact_size.y = control::SMALL;
        style.spacing.menu_margin = Margin::same(space::XS as i8 + 2);
        style.spacing.menu_spacing = space::XS;
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.floating = true;
        style.interaction.selectable_labels = false;

        let visuals = &mut style.visuals;

        visuals.dark_mode = dark;
        visuals.panel_fill = palette::background();
        visuals.window_fill = palette::surface();
        visuals.extreme_bg_color = palette::sunken();
        visuals.faint_bg_color = palette::surface();
        visuals.override_text_color = Some(palette::text());
        visuals.hyperlink_color = palette::accent();
        visuals.selection.bg_fill = palette::accent().gamma_multiply(0.35);
        visuals.selection.stroke = Stroke::new(1.0, palette::accent());
        visuals.window_stroke = Stroke::new(1.0, palette::border());

        let radius = CornerRadius::same(radius::M);

        for (widget, fill) in [
            (&mut visuals.widgets.noninteractive, Color32::TRANSPARENT),
            (&mut visuals.widgets.inactive, palette::surface()),
            (&mut visuals.widgets.hovered, palette::raised()),
            (&mut visuals.widgets.active, palette::raised()),
            (&mut visuals.widgets.open, palette::raised()),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.bg_stroke = Stroke::NONE;
            widget.fg_stroke = Stroke::new(1.0, palette::text());
            widget.corner_radius = radius;
            widget.expansion = 0.0;
        }

        // Buttons and fields get a quiet outline, so they read as things
        // to press and type in against the dark surface.
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette::border());
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette::border());

        // egui draws a widget that holds the keyboard as "active", so
        // this is the focus ring of every button and switch.
        visuals.widgets.active.bg_stroke = focus_ring();

        visuals.window_corner_radius = CornerRadius::same(radius::L);
        visuals.menu_corner_radius = CornerRadius::same(radius::L);

        // Menus and floating windows lift off the page with a soft
        // shadow rather than a hard edge.
        let shadow = egui::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(if dark { 90 } else { 28 }),
        };
        visuals.popup_shadow = shadow;
        visuals.window_shadow = shadow;

        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette::border());
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette::text_secondary());
    });
}

/// The frame of a card: a quiet bordered surface.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(palette::surface())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(space::M as i8, space::S as i8 + 2))
}

/// The page's main action: filled with the accent.
pub fn primary_button(label: &str) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(label.to_string())
            .font(strong(size::UI))
            .color(palette::sunken()),
    )
    .fill(palette::accent())
    .stroke(Stroke::NONE)
    .corner_radius(CornerRadius::same(radius::M))
    .min_size(egui::vec2(0.0, control::MEDIUM))
}

/// A quiet bordered button for everything that is not the main action.
pub fn secondary_button(label: &str) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(label.to_string())
            .font(ui())
            .color(palette::text()),
    )
    .fill(palette::surface())
    .stroke(Stroke::new(1.0, palette::border()))
    .corner_radius(CornerRadius::same(radius::M))
    .min_size(egui::vec2(0.0, control::MEDIUM))
}

/// The outline of whatever holds the keyboard.
pub fn focus_ring() -> Stroke {
    Stroke::new(1.5, palette::accent())
}

pub fn small() -> FontId {
    FontId::proportional(size::SMALL)
}

/// The font of rows, buttons and labels.
pub fn ui() -> FontId {
    FontId::proportional(size::UI)
}

/// The font the conversation is read in.
pub fn body() -> FontId {
    FontId::proportional(size::BODY)
}

pub fn mono() -> FontId {
    FontId::monospace(size::SMALL)
}

/// The serif the assistant speaks in.
pub fn voice(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name(fonts::VOICE.into()))
}

/// The interface font, heavier, for titles.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name(fonts::STRONG.into()))
}

/// A rectangle with squircle corners: the curve eases into each straight
/// side instead of meeting it at a circle's sudden join, so cards and tiles
/// read as one soft shape. `radius` is how far each corner reaches along
/// its sides; it is kept to half the shorter side.
pub fn squircle(rect: egui::Rect, radius: f32, fill: Color32, stroke: Stroke) -> egui::Shape {
    // A quarter superellipse of exponent 4 per corner, sampled finely
    // enough that its edge stays smooth at any size a card is drawn.
    const EXPONENT: f32 = 4.0;
    const SAMPLES: usize = 10;

    let r = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let corners = [
        (egui::pos2(rect.right() - r, rect.top() + r), 0.0_f32),
        (egui::pos2(rect.right() - r, rect.bottom() - r), 0.5),
        (egui::pos2(rect.left() + r, rect.bottom() - r), 1.0),
        (egui::pos2(rect.left() + r, rect.top() + r), 1.5),
    ];
    let mut points = Vec::with_capacity(4 * (SAMPLES + 1));

    for (centre, start) in corners {
        for at in 0..=SAMPLES {
            // From the top of the right side round to the left of the
            // bottom side, and so on clockwise in screen space.
            let angle = (start - 0.5 + 0.5 * at as f32 / SAMPLES as f32) * std::f32::consts::PI;
            let (sin, cos) = angle.sin_cos();
            let x = cos.signum() * cos.abs().powf(2.0 / EXPONENT);
            let y = sin.signum() * sin.abs().powf(2.0 / EXPONENT);
            points.push(centre + egui::vec2(x * r, y * r));
        }
    }

    egui::Shape::convex_polygon(points, fill, stroke)
}

/// Draws `content` on a soft squircle card that fills the width it is
/// given, padded by `padding` on every side.
pub fn soft_card<R>(
    ui: &mut egui::Ui,
    fill: Color32,
    padding: f32,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let behind = ui.painter().add(egui::Shape::Noop);
    let response = egui::Frame::new()
        .inner_margin(Margin::same(padding as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui)
        });
    let rect = response.response.rect;
    ui.painter()
        .set(behind, squircle(rect, SOFT_RADIUS, fill, Stroke::NONE));

    response
}

/// How far a soft card's corners reach.
pub const SOFT_RADIUS: f32 = 22.0;

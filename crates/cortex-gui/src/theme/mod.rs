//! How the app looks: colours, fonts, spacing.

pub mod fonts;
pub mod palette;

use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, Stroke, TextStyle};

/// Corner radius of cards and bubbles.
pub const CARD_RADIUS: u8 = 10;

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
            (TextStyle::Heading, strong(20.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(13.5)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();

        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
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

        let radius = CornerRadius::same(7);

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
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette::text_muted());

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
        .inner_margin(Margin::symmetric(12, 9))
}

pub fn small() -> FontId {
    FontId::proportional(12.0)
}

pub fn mono() -> FontId {
    FontId::monospace(12.0)
}

/// The serif the assistant speaks in.
pub fn voice(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name(fonts::VOICE.into()))
}

/// The interface font, heavier, for titles.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name(fonts::STRONG.into()))
}

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

/// Widest the conversation column grows; wider windows leave equal
/// margins either side, so the chat sits in the middle.
pub const COLUMN_WIDTH: f32 = 800.0;

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

/// The interface font, heavier, for titles.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name(fonts::STRONG.into()))
}

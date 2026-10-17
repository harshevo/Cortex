//! How the app looks: its colour theme, and what the conversation shows.

use eframe::egui::{self, CornerRadius, Rect, Sense, Stroke, Ui, vec2};

use super::rows::{self, Filter};
use super::{Action, Form};
use crate::theme::palette::{self, Theme};
use crate::theme::{self, radius, size, space};

/// The size of a theme's card, and of the picture of the theme in it.
const CARD: egui::Vec2 = vec2(148.0, 104.0);
const PREVIEW_HEIGHT: f32 = 72.0;

pub fn show(ui: &mut Ui, form: &mut Form, filter: &Filter) -> Option<Action> {
    let mut chosen = None;

    for (dark, label) in [(true, "Dark themes"), (false, "Light themes")] {
        // A search for a theme's name shows the half it is in.
        let names: Vec<&str> = palette::THEMES
            .iter()
            .filter(|theme| theme.dark == dark)
            .map(|theme| theme.name)
            .collect();

        let help = format!("Used as soon as it is picked: {}.", names.join(", "));

        rows::block(ui, filter, label, &help, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(space::M, space::M);

                for theme in palette::THEMES.iter().filter(|theme| theme.dark == dark) {
                    if card(ui, theme) {
                        chosen = Some(theme.name);
                    }
                }
            });
        });
    }

    rows::switch(
        ui,
        filter,
        "Show the model's thinking",
        "Shows the model's reasoning in the chat as it arrives, for models and providers \
         that send it.",
        &mut form.draft.show_thinking,
        true,
    );

    let mut fonts_changed = false;

    for (label, help, list, pick) in [
        (
            "Interface font",
            "The typeface of menus, buttons and labels.",
            theme::fonts::TEXT,
            &mut form.draft.interface_font,
        ),
        (
            "Reply font",
            "The typeface of the assistant's words in the chat.",
            theme::fonts::TEXT,
            &mut form.draft.reply_font,
        ),
        (
            "Code font",
            "The typeface of code, commands and their output.",
            theme::fonts::CODE,
            &mut form.draft.code_font,
        ),
    ] {
        rows::row(ui, filter, label, help, |ui| {
            egui::ComboBox::from_id_salt(label)
                .selected_text(pick.as_str())
                .show_ui(ui, |ui| {
                    for choice in list {
                        if ui.selectable_label(*pick == choice.name, choice.name).clicked() {
                            *pick = choice.name.to_string();
                            fonts_changed = true;
                        }
                    }
                });
        });
    }

    if fonts_changed && chosen.is_none() {
        return Some(Action::Fonts(form.draft.font_selection()));
    }

    let name = chosen?;

    // Kept in the draft too, so that applying other settings does not
    // bring the old theme back.
    form.draft.theme = name.to_string();

    Some(Action::Theme(name))
}

/// A theme as a small picture of the app in its colours, with its name
/// under it. Returns whether it was picked and is not the one in use.
fn card(ui: &mut Ui, theme: &Theme) -> bool {
    let selected = theme.name == palette::current().name;

    let (rect, response) = ui.allocate_exact_size(CARD, Sense::click());

    if !ui.is_rect_visible(rect) {
        return false;
    }

    let painter = ui.painter_at(rect);
    let corner = CornerRadius::same(radius::L);

    let preview = Rect::from_min_size(rect.min, vec2(rect.width(), PREVIEW_HEIGHT));

    preview_of(&painter, preview, theme);

    let outline = if selected {
        Stroke::new(2.0, palette::accent())
    } else if response.hovered() || response.has_focus() {
        Stroke::new(1.0, palette::text_muted())
    } else {
        Stroke::new(1.0, palette::border())
    };

    painter.rect_stroke(preview, corner, outline, egui::StrokeKind::Inside);

    painter.text(
        egui::pos2(rect.left() + 2.0, preview.bottom() + 15.0),
        egui::Align2::LEFT_CENTER,
        theme.name,
        theme::ui(),
        if selected {
            palette::text()
        } else {
            palette::text_secondary()
        },
    );

    if selected {
        painter.text(
            egui::pos2(rect.right() - 2.0, preview.bottom() + 15.0),
            egui::Align2::RIGHT_CENTER,
            "In use",
            egui::FontId::proportional(size::CAPTION),
            palette::accent(),
        );
    }

    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
        && !selected
}

/// The app in miniature, in `theme`'s own colours: the sidebar, what
/// was said and what was answered, the message box and its button.
fn preview_of(painter: &egui::Painter, rect: Rect, theme: &Theme) {
    let corner = CornerRadius::same(radius::L);

    painter.rect_filled(rect, corner, theme.background);

    let sidebar = Rect::from_min_size(rect.min, vec2(rect.width() * 0.26, rect.height()));

    painter.rect_filled(
        sidebar,
        CornerRadius {
            nw: radius::L,
            sw: radius::L,
            ne: 0,
            se: 0,
        },
        theme.sidebar,
    );

    let bar = |left: f32, top: f32, width: f32, color| {
        painter.rect_filled(
            Rect::from_min_size(rect.min + vec2(left, top), vec2(width, 4.0)),
            CornerRadius::same(radius::XS),
            color,
        );
    };

    // Sessions in the sidebar, one of them selected.
    let rail = sidebar.width();

    painter.rect_filled(
        Rect::from_min_size(rect.min + vec2(5.0, 19.0), vec2(rail - 10.0, 10.0)),
        CornerRadius::same(radius::XS),
        theme.raised,
    );

    bar(8.0, 10.0, rail - 20.0, theme.text_muted);
    bar(8.0, 22.0, rail - 18.0, theme.text);
    bar(8.0, 36.0, rail - 24.0, theme.text_secondary);

    let left = rail + 10.0;
    let width = rect.width() - left - 10.0;

    // What the person said, in its bubble on the right.
    let bubble = Rect::from_min_size(
        rect.min + vec2(left + width * 0.45, 8.0),
        vec2(width * 0.55, 12.0),
    );

    painter.rect_filled(bubble, CornerRadius::same(radius::S), theme.raised);

    bar(
        left + width * 0.45 + 5.0,
        12.0,
        width * 0.55 - 10.0,
        theme.text,
    );

    // What the agent answered, with a word of code and a result.
    bar(left, 27.0, width * 0.8, theme.text);
    bar(left, 35.0, width * 0.3, theme.text);
    bar(left + width * 0.3 + 4.0, 35.0, width * 0.22, theme.code);
    bar(left, 43.0, width * 0.18, theme.good);
    bar(left + width * 0.18 + 4.0, 43.0, width * 0.14, theme.warning);
    bar(left + width * 0.32 + 8.0, 43.0, width * 0.14, theme.bad);

    // The message box, and the button that sends.
    let composer = Rect::from_min_max(
        rect.min + vec2(left, rect.height() - 19.0),
        rect.max - vec2(10.0, 7.0),
    );

    painter.rect_filled(composer, CornerRadius::same(radius::S), theme.surface);

    painter.rect_stroke(
        composer,
        CornerRadius::same(radius::S),
        Stroke::new(1.0, theme.border),
        egui::StrokeKind::Inside,
    );

    painter.circle_filled(composer.right_center() - vec2(7.0, 0.0), 3.5, theme.accent);
}

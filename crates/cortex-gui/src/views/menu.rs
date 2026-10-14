//! Menus: the one way the app tucks controls away.
//!
//! A row keeps its one to three most used controls in sight; the rest go
//! into a menu: a dropdown, or a drop-up when the row sits near the
//! bottom of the window. Every menu is built from the
//! same rows, so they all look and behave alike.

use eframe::egui::{
    self, Align2, CornerRadius, Popup, PopupCloseBehavior, RectAlign, Response, Sense, Stroke, Ui,
    vec2,
};

use super::icons::{self, Icon};
use crate::theme::{self, control, palette, radius, space};

/// Narrowest a menu is drawn.
pub const MIN_WIDTH: f32 = 180.0;

/// Which way a menu opens from its button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opens {
    /// Below the button, its left edges lined up.
    Down,

    /// Below the button, its right edges lined up: for buttons at the
    /// right-hand end of a bar.
    DownLeft,

    /// Above the button, its left edges lined up: for controls near the
    /// bottom of the window, such as the composer.
    Up,

    /// Above the button, its right edges lined up.
    UpLeft,
}

impl Opens {
    fn align(self) -> RectAlign {
        match self {
            Self::Down => RectAlign::BOTTOM_START,
            Self::DownLeft => RectAlign::BOTTOM_END,
            Self::Up => RectAlign::TOP_START,
            Self::UpLeft => RectAlign::TOP_END,
        }
    }

    /// Where to try next when there is no room the preferred way.
    fn alternatives(self) -> &'static [RectAlign] {
        match self {
            Self::Down | Self::DownLeft => &[
                RectAlign::BOTTOM_START,
                RectAlign::BOTTOM_END,
                RectAlign::TOP_START,
                RectAlign::TOP_END,
            ],
            Self::Up | Self::UpLeft => &[
                RectAlign::TOP_START,
                RectAlign::TOP_END,
                RectAlign::BOTTOM_START,
                RectAlign::BOTTOM_END,
            ],
        }
    }
}

/// Opens a menu from `button` when it is clicked, and draws it while it
/// is open. The menu closes when one of its rows is clicked, or anywhere
/// outside it.
pub fn attach<R>(button: &Response, opens: Opens, content: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    popup(button, opens)
        .show(|ui| {
            ui.set_min_width(MIN_WIDTH);
            ui.spacing_mut().item_spacing.y = space::XXS;

            content(ui)
        })
        .map(|inner| inner.inner)
}

/// As [`attach`], but the menu stays open while it is clicked inside:
/// for menus that show detail and hold controls of their own.
pub fn attach_sticky<R>(
    button: &Response,
    opens: Opens,
    content: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    popup(button, opens)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = space::XS;

            content(ui)
        })
        .map(|inner| inner.inner)
}

fn popup(button: &Response, opens: Opens) -> Popup<'static> {
    Popup::menu(button)
        .align(opens.align())
        .align_alternatives(opens.alternatives())
        .gap(space::XS)
}

/// A button showing `label` and a chevron, that opens a menu. `quiet`
/// leaves out the ground and outline, for selectors that sit in running
/// text such as the row under the composer.
pub fn dropdown<R>(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    quiet: bool,
    opens: Opens,
    content: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let button = dropdown_button(ui, icon, label, quiet);
    let inner = attach(&button, opens, content);

    (button, inner)
}

/// The button of [`dropdown`] alone, for menus that need [`attach_sticky`].
pub fn dropdown_button(ui: &mut Ui, icon: Option<Icon>, label: &str, quiet: bool) -> Response {
    const PADDING: f32 = 10.0;
    const ICON: f32 = 16.0;
    const CHEVRON: f32 = 10.0;

    let enabled = ui.is_enabled();
    let text_color = if enabled {
        palette::text()
    } else {
        palette::text_muted()
    };

    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::small(), text_color);

    let icon_room = if icon.is_some() {
        ICON + space::XS
    } else {
        0.0
    };
    let width = PADDING + icon_room + galley.size().x + space::S + CHEVRON + PADDING - 2.0;

    let (rect, response) = ui.allocate_exact_size(vec2(width, control::SMALL), Sense::click());

    let open = Popup::is_id_open(ui.ctx(), Popup::default_response_id(&response));
    let hovered = response.hovered() && enabled;

    let fill = match (quiet, hovered || open) {
        (_, true) => palette::raised(),
        (false, false) => palette::surface(),
        (true, false) => egui::Color32::TRANSPARENT,
    };

    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::M), fill);

    if !quiet {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(radius::M),
            Stroke::new(1.0, palette::border()),
            egui::StrokeKind::Inside,
        );
    }

    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(radius::M),
            theme::focus_ring(),
            egui::StrokeKind::Inside,
        );
    }

    let mut left = rect.left() + PADDING;

    if let Some(icon) = icon {
        icons::paint(
            ui,
            egui::pos2(left + ICON / 2.0, rect.center().y),
            icon,
            palette::text_secondary(),
        );

        left += icon_room;
    }

    let text_width = galley.size().x;

    ui.painter().galley(
        egui::pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        text_color,
    );

    icons::chevron(
        ui,
        egui::pos2(
            left + text_width + space::S + CHEVRON / 2.0 - 2.0,
            rect.center().y + 0.5,
        ),
        palette::text_muted(),
    );

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One row of a menu: an optional icon, a label, and at the right-hand
/// end either a tick when it is the chosen one or a quiet `hint` (a key
/// to press, say). Clicking it closes the menu.
pub fn item(ui: &mut Ui, icon: Option<Icon>, label: &str, hint: &str, selected: bool) -> Response {
    row(ui, icon, label, hint, selected, false)
}

/// A row that removes or stops something: red when pointed at.
pub fn danger_item(ui: &mut Ui, icon: Option<Icon>, label: &str) -> Response {
    row(ui, icon, label, "", false, true)
}

fn row(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    hint: &str,
    selected: bool,
    danger: bool,
) -> Response {
    let enabled = ui.is_enabled();
    let width = ui.available_width().max(MIN_WIDTH - 12.0);

    let (rect, response) = ui.allocate_exact_size(vec2(width, control::MEDIUM), Sense::click());

    let hovered = response.hovered() && enabled;

    if hovered || selected {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(radius::M),
            if hovered {
                palette::raised()
            } else {
                palette::surface()
            },
        );
    }

    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(radius::M),
            theme::focus_ring(),
            egui::StrokeKind::Inside,
        );
    }

    let color = match (enabled, danger && hovered, selected || hovered) {
        (false, _, _) => palette::text_muted(),
        (true, true, _) => palette::bad(),
        (true, false, true) => palette::text(),
        (true, false, false) => palette::text_secondary(),
    };

    let mut left = rect.left() + space::S + 2.0;

    if let Some(icon) = icon {
        icons::paint(ui, egui::pos2(left + 7.0, rect.center().y), icon, color);
        left += 14.0 + space::S + 2.0;
    }

    ui.painter().text(
        egui::pos2(left, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::ui(),
        color,
    );

    let right = egui::pos2(rect.right() - space::S - 2.0, rect.center().y);

    if selected {
        icons::paint(ui, right - vec2(7.0, 0.0), Icon::Check, palette::accent());
    } else if !hint.is_empty() {
        ui.painter().text(
            right,
            Align2::RIGHT_CENTER,
            hint,
            theme::small(),
            palette::text_muted(),
        );
    }

    if response.clicked() {
        ui.close();
    }

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A quiet name over a group of rows.
pub fn title(ui: &mut Ui, text: &str) {
    ui.add_space(space::XS);

    ui.horizontal(|ui| {
        ui.add_space(space::S + 2.0);
        ui.label(
            egui::RichText::new(text)
                .font(theme::strong(theme::size::CAPTION))
                .color(palette::text_muted()),
        );
    });

    ui.add_space(space::XXS);
}

/// The line between groups of rows.
pub fn separator(ui: &mut Ui) {
    ui.add_space(space::XS);

    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());

    ui.painter().hline(
        rect.x_range().shrink(space::XS),
        rect.center().y,
        Stroke::new(1.0, palette::border()),
    );

    ui.add_space(space::XS);
}

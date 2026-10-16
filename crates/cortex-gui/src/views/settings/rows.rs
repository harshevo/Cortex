//! What every section is made of: a row with a name, a line of help and
//! a control at its right-hand end; and the filter that decides which
//! rows a search shows.

use std::cell::Cell;

use eframe::egui::{self, Align, Layout, RichText, Sense, Stroke, Ui, vec2};

use crate::theme::{self, palette, size, space};

/// Widest a text field in a row grows.
pub const FIELD_WIDTH: f32 = 300.0;

/// Which rows are drawn: all of them, or the ones a search matches.
pub struct Filter {
    /// What is searched for, in lower case. Empty when nothing is.
    query: String,

    /// The section whose rows follow. Its name is drawn above the first
    /// of them that matches, so a section with no match shows nothing.
    heading: Cell<Option<&'static str>>,

    /// How many rows have been drawn.
    shown: Cell<usize>,
}

impl Filter {
    pub fn everything() -> Self {
        Self::matching("")
    }

    /// `query` is in lower case.
    pub fn matching(query: &str) -> Self {
        Self {
            query: query.to_string(),
            heading: Cell::new(None),
            shown: Cell::new(0),
        }
    }

    pub fn searching(&self) -> bool {
        !self.query.is_empty()
    }

    /// Names the section whose rows come next.
    pub fn under(&self, title: &'static str) {
        self.heading.set(Some(title));
    }

    pub fn shown(&self) -> usize {
        self.shown.get()
    }

    /// Whether a row with this name and help is one to draw.
    pub fn matches(&self, label: &str, help: &str) -> bool {
        self.query.is_empty()
            || label.to_lowercase().contains(&self.query)
            || help.to_lowercase().contains(&self.query)
    }

    /// As [`Self::matches`], and if so counts the row and draws its
    /// section's name above it when it is the section's first.
    pub fn admit(&self, ui: &mut Ui, label: &str, help: &str) -> bool {
        if !self.matches(label, help) {
            return false;
        }

        self.shown.set(self.shown.get() + 1);

        if let Some(title) = self.heading.take() {
            section(ui, title);
        }

        true
    }
}

/// The name of a group of rows.
pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(space::L);

    ui.label(
        RichText::new(title)
            .font(theme::strong(size::CAPTION))
            .color(palette::text_muted()),
    );

    ui.add_space(space::XXS);
}

pub fn caption(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .font(theme::small())
                .color(palette::text_muted()),
        )
        .wrap(),
    );
}

/// A row: its name and help on the left, `control` at the right-hand
/// end. Returns what `control` returned, or `None` when the search left
/// the row out.
pub fn row<R>(
    ui: &mut Ui,
    filter: &Filter,
    label: &str,
    help: &str,
    control: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    if !filter.admit(ui, label, help) {
        return None;
    }

    ui.add_space(space::S);

    let inner = ui
        .allocate_ui_with_layout(
            vec2(ui.available_width(), 0.0),
            Layout::right_to_left(Align::Min),
            |ui| {
                let result = control(ui);

                ui.add_space(space::M);

                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    words(ui, label, help);
                });

                result
            },
        )
        .inner;

    rule(ui);

    Some(inner)
}

/// A row whose control is too wide to sit beside its name: the name and
/// help above, `contents` under them.
pub fn block<R>(
    ui: &mut Ui,
    filter: &Filter,
    label: &str,
    help: &str,
    contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    if !filter.admit(ui, label, help) {
        return None;
    }

    ui.add_space(space::S);

    words(ui, label, help);

    ui.add_space(space::XS);

    let inner = contents(ui);

    rule(ui);

    Some(inner)
}

fn words(ui: &mut Ui, label: &str, help: &str) {
    ui.spacing_mut().item_spacing.y = 1.0;

    ui.label(
        RichText::new(label)
            .font(theme::ui())
            .color(palette::text()),
    );

    if !help.is_empty() {
        caption(ui, help);
    }
}

/// The hairline under a row.
fn rule(ui: &mut Ui) {
    ui.add_space(space::S);

    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());

    ui.painter().hline(
        line.x_range(),
        line.center().y,
        Stroke::new(1.0, palette::border().gamma_multiply(0.5)),
    );
}

/// A row with an on/off switch. Returns whether it was flipped.
pub fn switch(
    ui: &mut Ui,
    filter: &Filter,
    label: &str,
    help: &str,
    on: &mut bool,
    enabled: bool,
) -> bool {
    row(ui, filter, label, help, |ui| {
        ui.add_enabled_ui(enabled, |ui| toggle(ui, on)).inner
    })
    .unwrap_or(false)
}

/// An on/off switch. Returns whether it was flipped.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> bool {
    const SIZE: egui::Vec2 = vec2(30.0, 18.0);

    let (rect, response) = ui.allocate_exact_size(SIZE, Sense::click());

    if response.clicked() {
        *on = !*on;
    }

    // The knob slides rather than jumps.
    let position = ui.ctx().animate_bool(response.id, *on);

    let track = if *on {
        palette::accent()
    } else {
        palette::raised()
    };

    let track = if ui.is_enabled() {
        track
    } else {
        track.gamma_multiply(0.5)
    };

    let half = rect.height() / 2.0;

    ui.painter().rect_filled(rect, half, track);

    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            half + 2.0,
            theme::focus_ring(),
            egui::StrokeKind::Middle,
        );
    } else if !*on {
        ui.painter().rect_stroke(
            rect,
            half,
            Stroke::new(1.0, palette::border()),
            egui::StrokeKind::Inside,
        );
    }

    let knob = egui::pos2(
        egui::lerp(rect.left() + half..=rect.right() - half, position),
        rect.center().y,
    );

    ui.painter()
        .circle_filled(knob, half - 2.5, palette::text());

    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// A one-line text field of a row's width.
pub fn text_field(ui: &mut Ui, value: &mut String, hint: &str) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(ui.available_width().min(FIELD_WIDTH))
            .margin(egui::Margin::symmetric(8, 5)),
    )
}

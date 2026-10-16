//! The optional capabilities, under the headings they belong to, with
//! the rarely wanted ones folded away.

use eframe::egui::{self, RichText, Sense, Ui, vec2};

use cortex_cli::features::Feature;

use super::rows::{self, Filter};
use super::{Action, Form};
use crate::settings::capabilities::{self, About, Group};
use crate::theme::{self, palette, space};
use crate::views::tool_card;

pub fn show(ui: &mut Ui, form: &mut Form, filter: &Filter) -> Option<Action> {
    let all = capabilities::all();

    // A search looks through the folded ones too.
    let (usual, rare): (Vec<&About>, Vec<&About>) = all
        .iter()
        .partition(|about| !about.advanced || filter.searching());

    for group in Group::ALL {
        let mut first = true;

        for about in usual.iter().filter(|about| about.group == group) {
            if !filter.matches(&about.label, &about.help) && !filter.matches(&about.name, "") {
                continue;
            }

            if std::mem::take(&mut first) && !filter.searching() {
                rows::section(ui, group.title());
            }

            capability(ui, form, about, filter);
        }
    }

    if rare.is_empty() {
        return None;
    }

    ui.add_space(space::L);

    if fold(ui, &format!("Advanced ({})", rare.len()), form.advanced) {
        form.advanced = !form.advanced;
    }

    if form.advanced {
        rows::caption(
            ui,
            "Rarely needed: most want a service of their own running, or change how the \
             conversation is kept.",
        );

        for about in rare {
            capability(ui, form, about, filter);
        }
    }

    None
}

/// One capability's switch, and under it the choices it brings.
fn capability(ui: &mut Ui, form: &mut Form, about: &About, filter: &Filter) {
    let mut on = form.draft.has(about.feature);

    // Found by its `--with` name as well as by what it is called.
    let label = about.label.as_ref();
    let named = Filter::everything();

    let filter = if filter.matches(label, &about.help) {
        filter
    } else {
        &named
    };

    let help = format!("{}  ·  {}", about.help, about.name);

    if rows::switch(ui, filter, label, &help, &mut on, true) {
        form.draft.set(about.feature, on);
    }

    if about.feature == Feature::Rollouts && on {
        attempts(ui, form, filter);
    }

    if about.feature == Feature::Browser && on {
        browser(ui, form, filter);
    }
}

/// How the `browser` capability shows its work: to the model, and to you.
fn browser(ui: &mut Ui, form: &mut Form, filter: &Filter) {
    ui.indent("browser", |ui| {
        rows::switch(
            ui,
            filter,
            "Show screenshots to the model",
            "On, the model sees the screenshots it takes. Only for a model that takes images. \
             Off, it reads pages as text.",
            &mut form.draft.browser_pictures,
            true,
        );

        rows::switch(
            ui,
            filter,
            "Show the browser window",
            "Off, the browser runs out of sight. On, it opens a window you can watch.",
            &mut form.draft.browser_window,
            true,
        );
    });
}

/// How many attempts the `rollouts` capability makes, and how.
fn attempts(ui: &mut Ui, form: &mut Form, filter: &Filter) {
    ui.indent("attempts", |ui| {
        rows::row(ui, filter, "Attempts at most", "From 2 to 8.", |ui| {
            ui.add(
                egui::DragValue::new(&mut form.draft.rollouts)
                    .range(2..=8)
                    .speed(0.1),
            );
        });

        rows::switch(
            ui,
            filter,
            "Make the attempts at the same time",
            "Off, a second attempt is made only when the first is not convincing. On, all are \
             made at once: faster, and it always costs every attempt.",
            &mut form.draft.rollouts_parallel,
            true,
        );
    });
}

/// A line that unfolds what is under it. Returns whether it was clicked.
fn fold(ui: &mut Ui, title: &str, open: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());

    let color = if response.hovered() || response.has_focus() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    let painter = ui.painter_at(rect);

    tool_card::chevron(&painter, rect.left_center() + vec2(5.0, 0.0), open, color);

    painter.text(
        rect.left_center() + vec2(18.0, 0.0),
        egui::Align2::LEFT_CENTER,
        title,
        theme::strong(theme::size::UI),
        color,
    );

    let _ = RichText::new("");

    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

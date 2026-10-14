//! The models of the provider in use.

use eframe::egui::{self, Ui};

use super::rows::{self, Filter};
use super::{Action, Form};
use crate::settings::Effort;
use crate::theme;

pub fn show(ui: &mut Ui, form: &mut Form, filter: &Filter) -> Option<Action> {
    let provider = form.draft.provider;

    rows::block(
        ui,
        filter,
        "Models, one per line",
        &format!(
            "The models of {} offered in the selector under the message box.",
            provider.label()
        ),
        |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut form.models)
                    .font(theme::mono())
                    .desired_rows(5)
                    .desired_width(ui.available_width().min(460.0))
                    .margin(egui::Margin::symmetric(8, 6)),
            );
        },
    );

    if provider.takes_effort() {
        rows::row(
            ui,
            filter,
            "Reasoning effort",
            "How hard the model thinks before it answers. Also under the message box.",
            |ui| {
                egui::ComboBox::from_id_salt("settings effort")
                    .selected_text(form.draft.effort.label())
                    .show_ui(ui, |ui| {
                        for effort in Effort::ALL {
                            ui.selectable_value(&mut form.draft.effort, effort, effort.label());
                        }
                    });
            },
        );
    }

    None
}

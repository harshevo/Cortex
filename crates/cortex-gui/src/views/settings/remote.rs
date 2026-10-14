//! Letting paired phones follow and drive the chats.

use eframe::egui::Ui;

use super::rows::{self, Filter};
use super::{Action, Form};

const LABEL: &str = "Remote control";

const HELP: &str = "Pair a phone, follow your chats from it, and let it send instructions and \
                    answer the agent's questions.";

pub fn show(ui: &mut Ui, form: &mut Form, filter: &Filter) -> Option<Action> {
    // The pairing code and the list of phones are not rows; a search
    // shows the way to them instead.
    if filter.searching() {
        rows::row(ui, filter, LABEL, HELP, |ui| {
            if ui.button("Open").clicked() {
                form.query.clear();
                form.show_remote();
            }
        });

        return None;
    }

    if crate::remote::bridge().is_none() {
        rows::caption(
            ui,
            "Remote control is not available in this copy of the app.",
        );

        return None;
    }

    let on = crate::views::remote::show(ui, form.draft.remote_control)?;

    // Kept in the draft too, so that applying other settings does not
    // switch it back.
    form.draft.remote_control = on;

    Some(Action::RemoteControl(on))
}

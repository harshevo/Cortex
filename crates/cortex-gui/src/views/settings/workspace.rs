//! Getting a project ready.

use eframe::egui::{self, Ui};

use super::rows::{self, Filter};
use super::{Action, Form, Workspace};

pub fn show(
    ui: &mut Ui,
    _form: &mut Form,
    workspace: &Workspace,
    filter: &Filter,
) -> Option<Action> {
    let mut action = None;

    let prepare = rows::row(
        ui,
        filter,
        "Prepare project",
        "Parse the project, embed it, and have the model write a short description that \
         is shown in this chat.",
        |ui| {
            ui.add_enabled(workspace.can_prepare, egui::Button::new("Prepare"))
                .on_disabled_hover_text("Available when a model is set up and no task is running.")
                .clicked()
        },
    );

    if prepare == Some(true) {
        action = Some(Action::Prepare);
    }

    action
}

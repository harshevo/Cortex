//! Memory, and getting a project ready.

use eframe::egui::{self, Ui};

use super::rows::{self, Filter};
use super::{Action, Form, Workspace};

pub fn show(
    ui: &mut Ui,
    form: &mut Form,
    workspace: &Workspace,
    filter: &Filter,
) -> Option<Action> {
    let mut action = None;

    let opened = rows::row(
        ui,
        filter,
        "Brain network",
        &format!(
            "The one brain shared by chat, code and trading, drawn live: columns, cells, dendrites, \
             synapses, taught memories and what it is curious about. The agent also \
             remembers {} earlier tasks.",
            workspace.memories
        ),
        |ui| ui.button("Open").clicked(),
    );

    if opened == Some(true) {
        action = Some(Action::OpenBrain);
    }

    rows::switch(
        ui,
        filter,
        "Share memory between projects",
        "Off, each project has its own memory. On, the agent also recalls the closest tasks \
         and general lessons from your other projects. What it knows about a project's own \
         files stays with that project.",
        &mut form.draft.combine_memory,
        true,
    );

    let prepare = rows::row(
        ui,
        filter,
        "Prepare project",
        "Parse the project, embed it, and have the model write a short description that \
         every later task is shown.",
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

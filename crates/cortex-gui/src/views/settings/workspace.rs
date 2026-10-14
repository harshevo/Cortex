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
        "Memory space",
        &format!(
            "What Cortex remembers, drawn as a brain: {} earlier tasks so far.",
            workspace.memories
        ),
        |ui| ui.button("Open").clicked(),
    );

    if opened == Some(true) {
        action = Some(Action::OpenMemory);
    }

    let opened = rows::row(
        ui,
        filter,
        "Living network",
        "The dendritic network drawn as it is, growing and rewiring as it watches runs. \
         Needs the Dendritic network or the Living network capability.",
        |ui| ui.button("Open").clicked(),
    );

    if opened == Some(true) {
        action = Some(Action::OpenLiving);
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

//! What the agent may do without asking, and for how long it may go on.

use eframe::egui::{self, Ui};

use super::rows::{self, Filter};
use super::{Action, Form};

pub fn show(ui: &mut Ui, form: &mut Form, filter: &Filter) -> Option<Action> {
    rows::switch(
        ui,
        filter,
        "Edit files and run commands without asking",
        "Off, Cortex asks before each change. On, it does not ask at all.",
        &mut form.draft.approve_everything,
        true,
    );

    rows::switch(
        ui,
        filter,
        "Run commands in a sandbox",
        sandbox_help(),
        &mut form.draft.sandbox,
        true,
    );

    rows::row(
        ui,
        filter,
        "Most model calls per instruction",
        "The agent pauses when it reaches this limit. From 1 to 500.",
        |ui| {
            ui.add(
                egui::DragValue::new(&mut form.draft.max_steps)
                    .range(1..=500)
                    .speed(0.5),
            );
        },
    );

    None
}

fn sandbox_help() -> &'static str {
    if cortex_core::Sandbox::available() {
        "On, a command the agent runs can write only in the project and temporary folders, \
         and cannot read where keys are kept. Off, it runs with your own rights."
    } else {
        "This system has no sandbox yet, so commands are not confined either way."
    }
}

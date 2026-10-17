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

    rows::row(
        ui,
        filter,
        "Budget per run",
        "Cortex wraps up at 80% and stops at the limit. 0 is no limit.",
        |ui| {
            ui.horizontal(|ui| {
                let mut dollars = f64::from(form.draft.max_cost_cents) / 100.0;

                let cost = ui.add(
                    egui::DragValue::new(&mut dollars)
                        .range(0.0..=500.0)
                        .speed(0.05)
                        .prefix("$")
                        .fixed_decimals(2),
                );

                if cost.changed() {
                    form.draft.max_cost_cents = (dollars * 100.0).round() as u32;
                }

                ui.add(
                    egui::DragValue::new(&mut form.draft.max_thousand_tokens)
                        .range(0..=100_000)
                        .speed(5.0)
                        .suffix("k tokens"),
                );

                ui.add(
                    egui::DragValue::new(&mut form.draft.max_minutes)
                        .range(0..=600)
                        .speed(0.5)
                        .suffix(" min"),
                );
            });
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

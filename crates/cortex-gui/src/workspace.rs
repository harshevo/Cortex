use eframe::egui::{self, Layout, Sense, UiBuilder};

use crate::app::CortexApp;
use crate::theme::palette;
use crate::views::top_bar::{Step, Steps};

#[derive(Clone, Copy, PartialEq)]
enum Split {
    Off,
    Columns,
    Rows,
}

pub struct Workspace {
    chats: Vec<(u64, CortexApp)>,
    active: u64,
    secondary: Option<u64>,
    next_id: u64,
    split: Split,
    ratio: f32,

    /// The chats that have been on screen, oldest first, and which of
    /// them is on screen now: what the bar's arrows step through.
    visited: Vec<u64>,
    position: usize,
}

impl Workspace {
    pub fn new(app: CortexApp, split: bool) -> Self {
        let mut workspace = Self {
            chats: vec![(1, app)],
            active: 1,
            secondary: None,
            next_id: 2,
            split: Split::Off,
            ratio: 0.5,
            visited: vec![1],
            position: 0,
        };
        if split {
            workspace.choose_split(Split::Columns);
        }
        workspace
    }

    fn index(&self, id: u64) -> usize {
        self.chats
            .iter()
            .position(|(key, _)| *key == id)
            .unwrap_or(0)
    }

    fn add(&mut self) -> u64 {
        let app = self.chats[self.index(self.active)].1.fork();
        let id = self.next_id;
        self.next_id += 1;
        self.chats.push((id, app));
        id
    }

    fn choose_split(&mut self, split: Split) {
        self.split = split;
        if split != Split::Off && self.secondary.is_none() {
            self.secondary = self
                .chats
                .iter()
                .find(|(id, _)| *id != self.active)
                .map(|(id, _)| *id);
            if self.secondary.is_none() {
                self.secondary = Some(self.add());
            }
        }
    }

    fn close(&mut self, id: u64) {
        if self.chats.len() == 1 {
            return;
        }
        let index = self.index(id);
        self.chats[index].1.stop();
        self.chats.remove(index);
        if self.active == id {
            self.active = self.chats[index.min(self.chats.len() - 1)].0;
        }
        if self.secondary == Some(id) || self.secondary == Some(self.active) {
            self.secondary = None;
            self.split = Split::Off;
        }
    }

    /// Puts the chat `id` on screen and remembers it as visited. Whatever
    /// could have been stepped forward to is forgotten, as in a browser.
    fn activate(&mut self, id: u64) {
        if self.visited.get(self.position) != Some(&id) {
            self.visited.truncate(self.position + 1);
            self.visited.push(id);
            self.position = self.visited.len() - 1;
        }

        self.show(id);
    }

    /// Where a step from the chat on screen leads: the nearest visited
    /// chat in that direction that is still open and is a different one.
    fn step_target(&self, step: Step) -> Option<usize> {
        let open = |position: &usize| {
            let id = self.visited[*position];

            id != self.active && self.chats.iter().any(|(key, _)| *key == id)
        };

        match step {
            Step::Back => (0..self.position).rev().find(open),
            Step::Forward => (self.position + 1..self.visited.len()).find(open),
        }
    }

    fn take_step(&mut self, step: Step) {
        if let Some(position) = self.step_target(step) {
            let id = self.visited[position];

            self.position = position;
            self.show(id);

            // A step leads to the session itself, even if its settings
            // or memory space were open when it was left.
            let index = self.index(id);
            self.chats[index].1.show_chat();
        }
    }

    fn show(&mut self, id: u64) {
        if self.secondary == Some(id) {
            self.secondary = Some(self.active);
        }
        self.active = id;
        let index = self.index(id);
        self.chats[index].1.refresh_history();
    }

    fn pane(&mut self, ui: &mut egui::Ui, id: u64, rect: egui::Rect) -> bool {
        let index = self.index(id);
        let mut pane = ui.new_child(
            UiBuilder::new()
                .id_salt(("chat pane", id))
                .max_rect(rect)
                .layout(Layout::top_down(egui::Align::Min)),
        );
        pane.set_clip_rect(rect);
        let focus = pane.interact(rect, egui::Id::new((id, "pane focus")), Sense::hover());
        let clicked = focus.contains_pointer() && pane.input(|input| input.pointer.any_pressed());
        self.chats[index].1.render_pane(&mut pane);
        clicked
    }
}

impl eframe::App for Workspace {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.chats[0].1.raw_input_hook(ctx, input);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut history_changed = false;
        // Called on the window's own `ui`: a scope for each chat would take
        // up a line's spacing above the top bar.
        for (_, app) in &mut self.chats {
            history_changed |= app.tick(ui);
        }
        let active = self.index(self.active);
        if history_changed {
            self.chats[active].1.refresh_history();
        }
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::B)) {
            self.chats[active].1.toggle_sidebar();
        }
        let mut selected = None;
        let mut closed = None;
        let mut added = false;
        let mut split = self.split;
        let chats: Vec<_> = self
            .chats
            .iter()
            .map(|(id, app)| (*id, app.label(), app.running(), app.agent_id().to_string()))
            .collect();
        let active_id = self.active;
        let steps = Steps {
            back: self.step_target(Step::Back).is_some(),
            forward: self.step_target(Step::Forward).is_some(),
        };
        let step = self.chats[active].1.top_bar_with_menu(ui, steps, |ui| {
            if ui.button("New chat    ⌘N").clicked() {
                added = true;
                ui.close();
            }
            if chats.len() > 1 {
                ui.separator();
                for (id, label, running, agent_id) in &chats {
                    let label = format!("{}{}", if *running { "● " } else { "" }, label);
                    if ui
                        .selectable_label(*id == active_id, label)
                        .on_hover_text(agent_id)
                        .clicked()
                    {
                        selected = Some(*id);
                        ui.close();
                    }
                }
                if ui.button("Close current chat").clicked() {
                    closed = Some(active_id);
                    ui.close();
                }
            }
            ui.separator();
            for (value, label) in [
                (Split::Off, "Single pane"),
                (Split::Columns, "Side by side"),
                (Split::Rows, "Top and bottom"),
            ] {
                if ui.selectable_label(split == value, label).clicked() {
                    split = value;
                    ui.close();
                }
            }
        });
        let keyed = ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::COMMAND, egui::Key::OpenBracket) {
                Some(Step::Back)
            } else if input.consume_key(egui::Modifiers::COMMAND, egui::Key::CloseBracket) {
                Some(Step::Forward)
            } else {
                None
            }
        });
        if let Some(step) = step.or(keyed) {
            self.take_step(step);
        }
        if let Some(id) = selected {
            self.activate(id);
        }
        if let Some(id) = closed {
            self.close(id);
        }
        if added {
            let id = self.add();
            self.activate(id);
        }
        if split != self.split {
            self.choose_split(split);
        }
        let active = self.index(self.active);
        if self.chats[active].1.sidebar_visible() {
            let running: Vec<_> = self
                .chats
                .iter()
                .filter(|(_, app)| app.running())
                .filter_map(|(_, app)| app.log_path().map(std::path::Path::to_path_buf))
                .collect();
            self.chats[active].1.sidebar_with_activity(ui, &running);
        }
        self.chats[active].1.files_panel(ui);
        if let Some((folder, log)) = self.chats[active].1.take_requested_session() {
            let existing = log.as_ref().and_then(|path| {
                self.chats
                    .iter()
                    .find(|(_, app)| app.log_path() == Some(path.as_path()))
                    .map(|(id, _)| *id)
            });
            if let Some(id) = existing {
                self.activate(id);
                let index = self.index(id);
                self.chats[index].1.show_chat();
            } else {
                let id = self.add();
                let index = self.index(id);
                self.chats[index].1.open_requested(folder, log);
                self.activate(id);
            }
        }
        let active = self.index(self.active);
        if let Some(path) = self.chats[active].1.take_requested_delete() {
            if self
                .chats
                .iter()
                .any(|(_, app)| app.running() && app.log_path() == Some(path.as_path()))
            {
                self.chats[active]
                    .1
                    .notice("Stop the agent in this session before deleting its history.");
            } else {
                let open: Vec<_> = self
                    .chats
                    .iter()
                    .filter(|(_, app)| app.log_path() == Some(path.as_path()))
                    .map(|(id, _)| *id)
                    .collect();
                self.chats[active].1.delete_session(&path);
                for id in open {
                    self.close(id);
                }
                for (_, app) in &mut self.chats {
                    app.refresh_history();
                }
            }
        }
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::N)) {
            let id = self.add();
            self.activate(id);
        }
        let rect = ui.available_rect_before_wrap();
        if self.split == Split::Off || self.secondary.is_none() {
            self.pane(ui, self.active, rect);
        } else {
            let columns = self.split == Split::Columns;
            let size = if columns { rect.width() } else { rect.height() };
            let position = if columns { rect.left() } else { rect.top() };
            let cut = position + (size - 6.0) * self.ratio;
            let mut first = rect;
            let mut second = rect;
            let mut divider = rect;
            if columns {
                first.max.x = cut;
                second.min.x = cut + 6.0;
                divider.min.x = cut;
                divider.max.x = cut + 6.0;
            } else {
                first.max.y = cut;
                second.min.y = cut + 6.0;
                divider.min.y = cut;
                divider.max.y = cut + 6.0;
            }
            let drag = ui.interact(divider, ui.id().with("split divider"), Sense::drag());
            let drag = drag.on_hover_cursor(if columns {
                egui::CursorIcon::ResizeHorizontal
            } else {
                egui::CursorIcon::ResizeVertical
            });
            if drag.dragged()
                && let Some(pointer) = drag.interact_pointer_pos()
            {
                self.ratio = (((if columns { pointer.x } else { pointer.y }) - position)
                    / (size - 6.0).max(1.0))
                .clamp(0.25, 0.75);
            }
            ui.painter().rect_filled(
                divider,
                0.0,
                if drag.hovered() || drag.dragged() {
                    palette::accent()
                } else {
                    palette::border()
                },
            );
            self.pane(ui, self.active, first);
            let secondary = self.secondary.unwrap();
            if self.pane(ui, secondary, second) {
                self.activate(secondary);
            }
        }
    }

    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        palette::background().to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{args::RehearsalArgs, engine::Engine, rehearsal::Rehearsal, settings::Settings};

    fn workspace() -> (tokio::runtime::Runtime, Workspace) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let app = CortexApp::new(
            Engine::start(runtime.handle(), egui::Context::default()),
            Settings::default(),
            None,
            true,
            Rehearsal::new(RehearsalArgs::default()),
        );

        (runtime, Workspace::new(app, false))
    }

    #[test]
    fn the_arrows_step_through_sessions_in_the_order_they_were_opened() {
        let (_runtime, mut workspace) = workspace();

        assert!(workspace.step_target(Step::Back).is_none());

        let second = workspace.add();
        workspace.activate(second);
        let third = workspace.add();
        workspace.activate(third);

        workspace.take_step(Step::Back);
        assert_eq!(workspace.active, second);

        workspace.take_step(Step::Back);
        assert_eq!(workspace.active, 1);
        assert!(workspace.step_target(Step::Back).is_none());

        workspace.take_step(Step::Forward);
        workspace.take_step(Step::Forward);
        assert_eq!(workspace.active, third);
        assert!(workspace.step_target(Step::Forward).is_none());
    }

    #[test]
    fn opening_a_session_after_stepping_back_forgets_what_was_ahead() {
        let (_runtime, mut workspace) = workspace();

        let second = workspace.add();
        workspace.activate(second);
        workspace.take_step(Step::Back);

        let third = workspace.add();
        workspace.activate(third);

        assert!(workspace.step_target(Step::Forward).is_none());

        workspace.take_step(Step::Back);
        assert_eq!(workspace.active, 1);
    }

    #[test]
    fn a_step_passes_over_a_session_that_was_closed() {
        let (_runtime, mut workspace) = workspace();

        let second = workspace.add();
        workspace.activate(second);
        let third = workspace.add();
        workspace.activate(third);

        workspace.close(second);
        workspace.take_step(Step::Back);

        assert_eq!(workspace.active, 1);
    }

    #[test]
    fn splits_switch_and_close_without_losing_other_chats() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let app = CortexApp::new(
            Engine::start(runtime.handle(), egui::Context::default()),
            Settings::default(),
            None,
            true,
            Rehearsal::new(RehearsalArgs::default()),
        );
        let mut workspace = Workspace::new(app, true);
        assert_eq!(workspace.chats.len(), 2);
        assert_ne!(
            workspace.chats[0].1.agent_id(),
            workspace.chats[1].1.agent_id()
        );
        let second = workspace.secondary.unwrap();
        workspace.activate(second);
        assert_eq!(workspace.secondary, Some(1));
        assert_eq!(workspace.active, second);
        let third = workspace.add();
        workspace.activate(third);
        workspace.close(third);
        assert_eq!(workspace.chats.len(), 2);
        assert!(
            workspace
                .chats
                .iter()
                .any(|(id, _)| *id == workspace.active)
        );
        workspace.close(1);
        assert_eq!(workspace.chats.len(), 1);
        assert!(workspace.secondary.is_none());
        assert!(workspace.split == Split::Off);
        workspace.close(workspace.active);
        assert_eq!(workspace.chats.len(), 1);
        workspace.choose_split(Split::Rows);
        assert_eq!(workspace.chats.len(), 2);
        assert!(workspace.split == Split::Rows);
    }
}

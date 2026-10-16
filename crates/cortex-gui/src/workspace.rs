use eframe::egui::{self, CornerRadius, Layout, Sense, Stroke, UiBuilder};

use crate::app::CortexApp;
use crate::theme::palette;
use crate::views::icons::{self, Icon};
use crate::views::menu;
use crate::views::top_bar::{Step, Steps};

/// The height of the strip that holds the minimized chats.
const TRAY_HEIGHT: f32 = 34.0;

/// Narrower than this, the window puts the sidebar away so the chat
/// keeps room to be read. It can still be brought back with ⌘B.
const NARROW_WINDOW: f32 = 760.0;

/// The height of the row of buttons above each pane of a split.
const PANE_BAR_HEIGHT: f32 = 28.0;

#[derive(Clone, Copy, PartialEq)]
enum Split {
    Off,
    Columns,
    Rows,
}

/// What the person did to a pane this frame.
#[derive(Clone, Copy, PartialEq)]
enum PaneAction {
    None,
    Focus,
    Close,
    Minimize,
}

pub struct Workspace {
    chats: Vec<(u64, CortexApp)>,
    active: u64,
    secondary: Option<u64>,
    next_id: u64,
    split: Split,
    ratio: f32,

    /// The split to go back to when a minimized chat is brought back.
    layout: Split,

    /// Chats taken off screen while they work, oldest first. They keep
    /// running and wait in the tray above the panes.
    minimized: Vec<u64>,

    /// The chats that have been on screen, oldest first, and which of
    /// them is on screen now: what the bar's arrows step through.
    visited: Vec<u64>,
    position: usize,

    /// The window was narrow on the frame before.
    narrow: bool,

    /// The sidebar was put away because the window became narrow, not
    /// by the person: it comes back when there is room again.
    stepped_aside: bool,
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
            layout: Split::Columns,
            minimized: Vec::new(),
            visited: vec![1],
            position: 0,
            narrow: false,
            stepped_aside: false,
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
        if split != Split::Off {
            self.layout = split;
        }
        if split != Split::Off && self.secondary.is_none() {
            self.secondary = self
                .chats
                .iter()
                .find(|(id, _)| *id != self.active && !self.minimized.contains(id))
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
        self.minimized.retain(|minimized| *minimized != id);
        if self.active == id {
            // The other pane of a split takes over; failing that, a chat
            // that is not waiting in the tray.
            let beside = self.secondary.filter(|secondary| *secondary != id);
            let waiting = |id: &u64| self.minimized.contains(id);
            self.active = beside
                .or_else(|| self.chats.iter().map(|(id, _)| *id).find(|id| !waiting(id)))
                .unwrap_or(self.chats[index.min(self.chats.len() - 1)].0);
            self.minimized.retain(|minimized| *minimized != self.active);
        }
        if self.secondary == Some(id) || self.secondary == Some(self.active) {
            self.secondary = None;
            self.split = Split::Off;
        }
    }

    /// Takes one pane of a split off screen and leaves the other filling
    /// the window. The chat keeps running and waits in the tray.
    fn minimize(&mut self, id: u64) {
        let Some(secondary) = self.secondary else {
            return;
        };
        if self.split == Split::Off || (id != self.active && id != secondary) {
            return;
        }
        if id == self.active {
            self.active = secondary;
            let index = self.index(secondary);
            self.chats[index].1.refresh_history();
        }
        self.secondary = None;
        self.split = Split::Off;
        self.minimized.push(id);
    }

    /// Brings a minimized chat back as the second pane of a split.
    fn restore(&mut self, id: u64) {
        if !self.minimized.contains(&id) {
            return;
        }
        self.minimized.retain(|minimized| *minimized != id);
        self.secondary = Some(id);
        self.split = self.layout;
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
            // were open when it was left.
            let index = self.index(id);
            self.chats[index].1.show_chat();
        }
    }

    fn show(&mut self, id: u64) {
        if self.secondary == Some(id) {
            self.secondary = Some(self.active);
        }
        self.minimized.retain(|minimized| *minimized != id);
        self.active = id;
        let index = self.index(id);
        self.chats[index].1.refresh_history();
    }

    /// Draws one pane. `in_split` adds the pane's own buttons to its top
    /// right-hand corner: close, and minimize while its agent is working.
    fn pane(&mut self, ui: &mut egui::Ui, id: u64, rect: egui::Rect, in_split: bool) -> PaneAction {
        let index = self.index(id);

        // In a split each pane has a row of its own for its buttons, so
        // they never sit on top of the conversation.
        let mut content = rect;
        if in_split {
            content.min.y += PANE_BAR_HEIGHT;
        }

        let mut pane = ui.new_child(
            UiBuilder::new()
                .id_salt(("chat pane", id))
                .max_rect(content)
                .layout(Layout::top_down(egui::Align::Min)),
        );
        pane.set_clip_rect(rect);
        let focus = pane.interact(rect, egui::Id::new((id, "pane focus")), Sense::hover());
        let clicked = focus.contains_pointer() && pane.input(|input| input.pointer.any_pressed());
        self.chats[index].1.render_pane(&mut pane);

        if in_split {
            let running = self.chats[index].1.running();
            let row = egui::Rect::from_min_size(
                rect.min,
                egui::vec2(rect.width() - 6.0, PANE_BAR_HEIGHT),
            );
            let close = icons::slot_from_right(row, 0);
            let minimize = icons::slot_from_right(row, 1);

            if icons::button_at(
                &pane,
                close,
                egui::Id::new((id, "pane close")),
                Icon::Close,
                if running {
                    "Close this pane and stop its agent"
                } else {
                    "Close this pane"
                },
                true,
            )
            .clicked()
            {
                return PaneAction::Close;
            }

            if running
                && icons::button_at(
                    &pane,
                    minimize,
                    egui::Id::new((id, "pane minimize")),
                    Icon::Minimize,
                    "Minimize: it keeps working, and waits above until you bring it back",
                    false,
                )
                .clicked()
            {
                return PaneAction::Minimize;
            }
        }

        if clicked {
            PaneAction::Focus
        } else {
            PaneAction::None
        }
    }

    /// The strip of minimized chats. Returns the one to bring back and
    /// the one to close, if either was asked for.
    fn tray(&mut self, ui: &mut egui::Ui, strip: egui::Rect) -> (Option<u64>, Option<u64>) {
        let mut restored = None;
        let mut closed = None;

        ui.painter().rect_filled(strip, 0.0, palette::sunken());
        ui.painter().hline(
            strip.x_range(),
            strip.bottom() - 0.5,
            Stroke::new(1.0, palette::border()),
        );

        let mut row = ui.new_child(
            UiBuilder::new()
                .id_salt("minimized chats")
                .max_rect(strip.shrink2(egui::vec2(10.0, 4.0)))
                .layout(Layout::left_to_right(egui::Align::Center)),
        );
        row.set_clip_rect(strip);
        row.spacing_mut().item_spacing.x = 6.0;

        for id in self.minimized.clone() {
            let app = &self.chats[self.index(id)].1;
            let running = app.running();
            let label = app.label();

            egui::Frame::new()
                .fill(palette::surface())
                .stroke(Stroke::new(1.0, palette::border()))
                .corner_radius(CornerRadius::same(crate::theme::radius::L))
                .inner_margin(egui::Margin::symmetric(8, 1))
                .show(&mut row, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                    if running {
                        icons::activity_dot(ui, dot.center());
                    } else {
                        ui.painter()
                            .circle_filled(dot.center(), 3.0, palette::good());
                    }

                    let text = egui::RichText::new(label)
                        .size(12.0)
                        .color(palette::text_secondary());
                    let name = ui
                        .add(
                            egui::Label::new(text)
                                .selectable(false)
                                .sense(Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(if running {
                            "Working. Click to bring it back"
                        } else {
                            "Finished. Click to bring it back"
                        });

                    if name.clicked()
                        || icons::button(
                            ui,
                            Icon::Maximize,
                            "Bring it back beside this chat",
                            false,
                        )
                        .clicked()
                    {
                        restored = Some(id);
                    }

                    let tooltip = if running {
                        "Close it and stop its agent"
                    } else {
                        "Close it"
                    };
                    if icons::button(ui, Icon::Close, tooltip, true).clicked() {
                        closed = Some(id);
                    }
                });
        }

        (restored, closed)
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
        // On becoming narrow the sidebar steps aside, once; it can be
        // brought back, and comes back by itself with the room.
        let narrow = ui.available_width() < NARROW_WINDOW;
        let visible = self.chats[active].1.sidebar_visible();
        if narrow && !self.narrow && visible {
            self.chats[active].1.toggle_sidebar();
            self.stepped_aside = true;
        } else if !narrow && self.narrow && self.stepped_aside && !visible {
            self.chats[active].1.toggle_sidebar();
        }
        if !narrow {
            self.stepped_aside = false;
        }
        self.narrow = narrow;
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
            if menu::item(ui, Some(Icon::Plus), "New chat", "⌘N", false).clicked() {
                added = true;
            }
            if chats.len() > 1 {
                menu::separator(ui);
                menu::title(ui, "Open chats");
                for (id, label, running, agent_id) in &chats {
                    let label = format!("{}{}", if *running { "● " } else { "" }, label);
                    if menu::item(ui, None, &label, "", *id == active_id)
                        .on_hover_text(agent_id)
                        .clicked()
                    {
                        selected = Some(*id);
                    }
                }
                if menu::danger_item(ui, Some(Icon::Close), "Close current chat").clicked() {
                    closed = Some(active_id);
                }
            }
            menu::separator(ui);
            menu::title(ui, "Layout");
            for (value, label) in [
                (Split::Off, "Single pane"),
                (Split::Columns, "Side by side"),
                (Split::Rows, "Top and bottom"),
            ] {
                if menu::item(ui, None, label, "", split == value).clicked() {
                    split = value;
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
        let mut rect = ui.available_rect_before_wrap();
        if !self.minimized.is_empty() {
            let mut strip = rect;
            strip.max.y = strip.min.y + TRAY_HEIGHT;
            rect.min.y = strip.max.y;
            let (restored, closed) = self.tray(ui, strip);
            if let Some(id) = closed {
                self.close(id);
            } else if let Some(id) = restored {
                self.restore(id);
            }
        }
        if self.split == Split::Off || self.secondary.is_none() {
            self.pane(ui, self.active, rect, false);
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
            let active = self.active;
            let secondary = self.secondary.unwrap();
            let on_active = self.pane(ui, active, first, true);
            let on_secondary = self.pane(ui, secondary, second, true);
            match (on_active, on_secondary) {
                (PaneAction::Close, _) => self.close(active),
                (_, PaneAction::Close) => self.close(secondary),
                (PaneAction::Minimize, _) => self.minimize(active),
                (_, PaneAction::Minimize) => self.minimize(secondary),
                (_, PaneAction::Focus) => self.activate(secondary),
                _ => {}
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
    fn closing_a_pane_leaves_the_other_one_on_screen() {
        let (_runtime, mut workspace) = workspace();

        workspace.choose_split(Split::Rows);
        let second = workspace.secondary.unwrap();
        let third = workspace.add();

        // The pane on the left goes; the one beside it stays, not the
        // chat that was never on screen.
        workspace.close(1);

        assert_eq!(workspace.active, second);
        assert!(workspace.split == Split::Off);
        assert!(workspace.chats.iter().any(|(id, _)| *id == third));
    }

    #[test]
    fn a_minimized_pane_waits_and_comes_back_in_the_same_split() {
        let (_runtime, mut workspace) = workspace();

        workspace.choose_split(Split::Rows);
        let second = workspace.secondary.unwrap();

        workspace.minimize(1);

        assert_eq!(workspace.active, second);
        assert!(workspace.split == Split::Off);
        assert_eq!(workspace.minimized, [1]);
        assert_eq!(workspace.chats.len(), 2);

        workspace.restore(1);

        assert_eq!(workspace.active, second);
        assert_eq!(workspace.secondary, Some(1));
        assert!(workspace.split == Split::Rows);
        assert!(workspace.minimized.is_empty());
    }

    #[test]
    fn a_minimized_chat_is_not_picked_to_fill_a_new_split() {
        let (_runtime, mut workspace) = workspace();

        workspace.choose_split(Split::Columns);
        let second = workspace.secondary.unwrap();
        workspace.minimize(second);

        workspace.choose_split(Split::Columns);

        let third = workspace.secondary.unwrap();
        assert_ne!(third, second);
        assert_eq!(workspace.minimized, [second]);

        // Brought back, it takes the second pane; the chat it replaces
        // stays open.
        workspace.restore(second);

        assert_eq!(workspace.secondary, Some(second));
        assert_eq!(workspace.chats.len(), 3);
    }

    #[test]
    fn opening_or_closing_a_minimized_chat_takes_it_out_of_the_tray() {
        let (_runtime, mut workspace) = workspace();

        workspace.choose_split(Split::Columns);
        let second = workspace.secondary.unwrap();
        workspace.minimize(second);
        workspace.activate(second);

        assert!(workspace.minimized.is_empty());
        assert_eq!(workspace.active, second);

        workspace.choose_split(Split::Columns);
        workspace.minimize(1);
        workspace.close(1);

        assert!(workspace.minimized.is_empty());
        assert_eq!(workspace.chats.len(), 1);
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

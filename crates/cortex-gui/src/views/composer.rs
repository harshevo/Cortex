//! The box you type instructions into.

use eframe::egui::{
    self, CornerRadius, Key, KeyboardShortcut, Modifiers, RichText, Sense, Stroke, Ui,
};

use crate::theme::{self, palette};

const BUTTON_SIZE: f32 = 34.0;

/// Room kept for the "Continue" button when it is shown.
const CONTINUE_WIDTH: f32 = 84.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Waiting for an instruction.
    Ready,

    /// A run is in progress. What is sent now is passed to it.
    Working,

    /// Another session is on screen while the agent works in its own. It
    /// can be read, and taken up once the agent is free.
    Viewing,

    /// The agent is being built.
    Starting,

    /// There is no agent: something in the settings is missing.
    Unavailable,
}

/// What the person did with the composer.
pub enum Action {
    Send(String),

    /// Tell the run in progress something.
    Steer(String),

    /// Carry on the run that stopped on a provider failure.
    Continue,

    /// Abandon the run in progress.
    Stop,
}

/// `can_continue` offers to carry on a run that a provider failure
/// stopped. It only shows while the composer is ready for an instruction.
/// `queued` is what has been sent to the run in progress and not yet read
/// by the model.
pub fn show(
    ui: &mut Ui,
    draft: &mut String,
    mode: Mode,
    can_continue: bool,
    queued: &[String],
) -> Option<Action> {
    let can_continue = can_continue && mode == Mode::Ready;

    let mut action = None;

    for message in queued {
        ui.horizontal(|ui| {
            ui.add_space(14.0);

            ui.label(
                RichText::new("Queued")
                    .font(theme::small())
                    .color(palette::WARNING),
            );

            let room = ((ui.available_width() - 20.0) / 6.6) as usize;

            ui.label(
                RichText::new(crate::format::one_line(message, room))
                    .font(theme::small())
                    .color(palette::TEXT_SECONDARY),
            )
            .on_hover_text("The model reads this after the step it is working on.");
        });
    }

    egui::Frame::new()
        .fill(palette::SURFACE)
        .stroke(Stroke::new(1.0, palette::BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let hint = match mode {
                    Mode::Ready if can_continue => "Continue the run, or ask for something else",
                    Mode::Ready => "Ask Cortex to change something",
                    Mode::Working => "Cortex is working. Tell it something, or wait…",

                    Mode::Viewing => {
                        "Cortex is busy in another session. This one can be continued when it is done."
                    }
                    Mode::Starting => "Getting ready…",
                    Mode::Unavailable => "Set up a model in Settings first",
                };

                let mut reserved = if can_continue { CONTINUE_WIDTH } else { 0.0 };

                // While working there are two buttons: send and stop.
                if mode == Mode::Working {
                    reserved += BUTTON_SIZE + 8.0;
                }

                let editor = egui::TextEdit::multiline(draft)
                    .hint_text(hint)
                    .desired_rows(1)
                    .desired_width((ui.available_width() - BUTTON_SIZE - 8.0 - reserved).max(40.0))
                    .frame(egui::Frame::NONE)
                    // Return sends; Shift+Return starts a new line.
                    .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter));

                let editor = ui.add_enabled(mode != Mode::Viewing, editor);

                if can_continue && continue_button(ui) {
                    action = Some(Action::Continue);
                }

                if mode == Mode::Working {
                    let can_send = !draft.trim().is_empty();

                    let pressed_return = editor.has_focus()
                        && ui.input(|input| input.key_pressed(Key::Enter) && !input.modifiers.shift);

                    if (send_button(ui, can_send) || pressed_return) && can_send {
                        action = Some(Action::Steer(draft.trim().to_string()));

                        draft.clear();
                    }

                    if stop_button(ui) {
                        action = Some(Action::Stop);
                    }
                } else {
                    let can_send = mode == Mode::Ready && !draft.trim().is_empty();

                    let pressed_return = editor.has_focus()
                        && ui.input(|input| input.key_pressed(Key::Enter) && !input.modifiers.shift);

                    // The button is drawn either way; a click on "Continue"
                    // in the same frame wins.
                    let wants_to_send = send_button(ui, can_send) || pressed_return;

                    if wants_to_send && can_send && action.is_none() {
                        action = Some(Action::Send(draft.trim().to_string()));

                        draft.clear();
                    }
                }

                // Preserve focus in search fields and other chat panes.
                if matches!(mode, Mode::Ready | Mode::Working)
                    && !editor.has_focus()
                    && ui.memory(|memory| memory.focused().is_none())
                {
                    let typed = typed_text(ui);

                    if !typed.is_empty() {
                        draft.push_str(&typed);

                        editor.request_focus();
                    }
                }

                // Focus an idle composer only when no other field owns input.
                if matches!(mode, Mode::Ready | Mode::Working) && ui.memory(|memory| memory.focused().is_none()) {
                    editor.request_focus();
                }
            });
        });

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(match mode {
                Mode::Working => "Messages sent now guide the active run",
                Mode::Viewing => "Viewing a saved conversation",
                Mode::Starting => "Preparing your agent",
                Mode::Unavailable => "Choose a provider and model in Settings",
                Mode::Ready => "Enter to send · Shift + Enter for a new line",
            })
            .font(theme::small())
            .color(palette::TEXT_MUTED),
        );
    });

    action
}

/// The text the keyboard produced this frame.
fn typed_text(ui: &Ui) -> String {
    ui.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Text(text) | egui::Event::Paste(text) => Some(text.as_str()),
                egui::Event::Ime(egui::ImeEvent::Commit(text)) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    })
}

/// A quiet pill that carries on the stopped run. Returns whether it was
/// clicked.
fn continue_button(ui: &mut Ui) -> bool {
    let label = RichText::new("Continue")
        .font(theme::small())
        .color(palette::TEXT);

    let button = egui::Button::new(label)
        .fill(palette::RAISED)
        .stroke(Stroke::new(1.0, palette::BORDER))
        .corner_radius(CornerRadius::same(12))
        .min_size(egui::vec2(CONTINUE_WIDTH - 10.0, BUTTON_SIZE - 6.0));

    ui.add(button)
        .on_hover_text(
            "The provider failed part-way through. Nothing the run had done is lost: \
             this asks the failed step again and carries on from there.",
        )
        .clicked()
}

/// A round button with a square in it. Returns whether it was clicked.
fn stop_button(ui: &mut Ui) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE), Sense::click());

    let fill = if response.hovered() {
        palette::TEXT
    } else {
        palette::TEXT_SECONDARY
    };

    let painter = ui.painter();

    painter.circle_filled(rect.center(), BUTTON_SIZE / 2.0, fill);

    painter.rect_filled(
        egui::Rect::from_center_size(rect.center(), egui::vec2(10.0, 10.0)),
        CornerRadius::same(2),
        palette::SUNKEN,
    );

    response.on_hover_text("Stop").clicked()
}

/// A round button with an upward arrow. Returns whether it was clicked.
fn send_button(ui: &mut Ui, enabled: bool) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };

    let (rect, response) = ui.allocate_exact_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE), sense);

    let fill = match (enabled, response.hovered()) {
        (false, _) => palette::RAISED,
        (true, false) => palette::ACCENT,
        (true, true) => palette::ACCENT.gamma_multiply(1.15),
    };

    let arrow = if enabled {
        palette::SUNKEN
    } else {
        palette::TEXT_MUTED
    };

    let painter = ui.painter();
    let centre = rect.center();
    let stroke = Stroke::new(1.8, arrow);

    painter.circle_filled(centre, BUTTON_SIZE / 2.0, fill);

    let tip = centre - egui::vec2(0.0, 6.0);

    painter.line_segment([centre + egui::vec2(0.0, 6.5), tip], stroke);
    painter.line_segment([tip, tip + egui::vec2(-5.0, 5.0)], stroke);
    painter.line_segment([tip, tip + egui::vec2(5.0, 5.0)], stroke);

    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_in_search_does_not_also_enter_the_composer() {
        let context = egui::Context::default();
        let mut search = String::new();
        let mut draft = String::new();

        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.text_edit_singleline(&mut search).request_focus();
            show(ui, &mut draft, Mode::Ready, false, &[]);
        });
        output.textures_delta.clear();
        let input = egui::RawInput {
            events: vec![egui::Event::Text("find a session".into())],
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            ui.text_edit_singleline(&mut search);
            show(ui, &mut draft, Mode::Ready, false, &[]);
        });

        output.textures_delta.clear();
        assert_eq!(search, "find a session");
        assert!(draft.is_empty());
    }
}

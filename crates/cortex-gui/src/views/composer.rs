//! The box you type instructions into.

use eframe::egui::{
    self, CornerRadius, Key, KeyboardShortcut, Modifiers, RichText, Sense, Stroke, Ui,
};

use cortex_core::Image;

use super::icons::{self, Icon};
use super::menu::{self, Opens};
use crate::attachment::{self, Attachment, Attachments};
use crate::theme::{self, palette};

const BUTTON_SIZE: f32 = crate::theme::control::LARGE;

/// Room kept for the "Continue" button when it is shown.
const CONTINUE_WIDTH: f32 = 84.0;

/// Side of a picture waiting in the composer.
const THUMBNAIL_SIZE: f32 = 52.0;

/// Sent as the instruction when only pictures were attached.
const PICTURES_ONLY: &str = "Look at the attached picture.";

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
    /// An instruction, and the pictures attached to it.
    Send(String, Vec<Image>),

    /// A file that was offered is not a picture that can be sent, and why.
    Refused(String),

    /// Tell the run in progress something.
    Steer(String),

    /// Put the last thing that was said back in the box, to send again
    /// or change.
    Recall,

    /// Carry on the run that stopped on a provider failure.
    Continue,

    /// Abandon the run in progress.
    Stop,
}

/// `can_continue` offers to carry on a run that a provider failure
/// stopped. It only shows while the composer is ready for an instruction.
/// `queued` is what has been sent to the run in progress and not yet read
/// by the model. `attachments` are the pictures that go with the next
/// instruction.
pub fn show(
    ui: &mut Ui,
    draft: &mut String,
    attachments: &mut Attachments,
    mode: Mode,
    can_continue: bool,
    queued: &[String],
) -> Option<Action> {
    let can_continue = can_continue && mode == Mode::Ready;

    let mut action = attachments.poll(ui.ctx()).map(Action::Refused);

    // Pictures go with an instruction, not with a word to a run in
    // progress, so they can only be added while the composer is ready.
    let takes_pictures = mode == Mode::Ready;

    if takes_pictures {
        let dropped: Vec<_> = ui.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .filter(|path| !path.as_os_str().is_empty())
                .collect()
        });

        if let Some(why) = attachments.enqueue(&dropped) {
            action = Some(Action::Refused(why));
        }
    }

    for message in queued {
        ui.horizontal(|ui| {
            ui.add_space(crate::theme::space::L);

            ui.label(
                RichText::new("Queued")
                    .font(theme::small())
                    .color(palette::warning()),
            );

            let room = ((ui.available_width() - 20.0) / 6.6) as usize;

            ui.label(
                RichText::new(crate::format::one_line(message, room))
                    .font(theme::small())
                    .color(palette::text_secondary()),
            )
            .on_hover_text("The model reads this after the step it is working on.");
        });
    }

    egui::Frame::new()
        .fill(palette::surface())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(crate::theme::radius::XL))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            thumbnails(ui, attachments);
            if attachments.loading() { ui.horizontal(|ui| { ui.spinner(); ui.label("Preparing pictures…"); }); }

            ui.horizontal(|ui| {
                if takes_pictures
                    && icons::button(ui, Icon::Plus, "Attach pictures, or drop them here", false).clicked()
                {
                    let chosen = rfd::FileDialog::new()
                        .set_title("Attach pictures")
                        .add_filter("Pictures", &attachment::EXTENSIONS)
                        .pick_files()
                        .unwrap_or_default();

                    if let Some(why) = attachments.enqueue(&chosen) {
                        action = Some(Action::Refused(why));
                    }
                }

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

                let editor_id = ui.id().with("composer editor");

                // Asked before the box is drawn: Escape takes the keyboard
                // away from it in the drawing.
                let had_focus = ui.memory(|memory| memory.has_focus(editor_id));
                let nothing_focused = ui.memory(|memory| memory.focused().is_none());

                // The last message was asked back on the frame before and
                // is in the box now.
                let recalled_id = editor_id.with("recalled");

                if ui.data_mut(|data| data.remove_temp::<bool>(recalled_id)).is_some() {
                    put_cursor_at_end(ui, editor_id, draft);
                }

                if ui.memory(|memory| memory.focused().is_none_or(|focused| focused == editor_id)) {
                    ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Tab));
                }

                let mut reserved = if can_continue { CONTINUE_WIDTH } else { 0.0 };

                // While working there are two buttons: send and stop.
                if mode == Mode::Working {
                    reserved += BUTTON_SIZE + 8.0;
                }

                let editor = egui::TextEdit::multiline(draft)
                    .id(editor_id)
                    // Keep keyboard focus in the composer. Previously, Tab during
                    // a run put the keyboard on the stop button, where
                    // typing went nowhere and Return stopped the run.
                    .lock_focus(true)
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

                let escaped = (had_focus || nothing_focused)
                    && !egui::Popup::is_any_open(ui.ctx())
                    && ui.input(|input| input.key_pressed(Key::Escape));

                // Up in an empty box brings back what was said last.
                let recall = mode == Mode::Ready
                    && had_focus
                    && draft.is_empty()
                    && attachments.is_empty()
                    && ui.input(|input| input.key_pressed(Key::ArrowUp) && input.modifiers.is_none());

                if recall && action.is_none() {
                    action = Some(Action::Recall);

                    ui.data_mut(|data| data.insert_temp(recalled_id, true));
                }

                if mode == Mode::Working {
                    let can_send = !draft.trim().is_empty();

                    let pressed_return = editor.has_focus()
                        && ui.input(|input| input.key_pressed(Key::Enter) && !input.modifiers.shift);

                    if (send_button(ui, can_send) || pressed_return) && can_send {
                        action = Some(Action::Steer(draft.trim().to_string()));

                        draft.clear();
                    }

                    if stop_button(ui) || escaped {
                        action = Some(Action::Stop);
                    }
                } else {
                    let can_send = mode == Mode::Ready
                        && !attachments.loading()
                        && (!draft.trim().is_empty() || !attachments.is_empty());

                    let pressed_return = editor.has_focus()
                        && ui.input(|input| input.key_pressed(Key::Enter) && !input.modifiers.shift);

                    // The button is drawn either way; a click on "Continue"
                    // in the same frame wins.
                    let wants_to_send = send_button(ui, can_send) || pressed_return;

                    if wants_to_send && can_send && action.is_none() {
                        let instruction = match draft.trim() {
                            "" => PICTURES_ONLY,
                            written => written,
                        };

                        let images = attachments
                            .drain(..)
                            .map(|attachment| attachment.image)
                            .collect();

                        action = Some(Action::Send(instruction.to_string(), images));

                        draft.clear();
                    }
                }

                let takes_keyboard = matches!(mode, Mode::Ready | Mode::Working);

                // What is typed while no field has the keyboard belongs
                // here. Search fields and the boxes of other panes keep
                // what is typed into them; a button that happens to hold
                // the keyboard does not swallow it.
                if takes_keyboard && !editor.has_focus() && !ui.ctx().text_edit_focused() {
                    let typed = typed_text(ui);

                    // A space on a button presses the button.
                    let pressing = typed.trim().is_empty()
                        && ui.memory(|memory| memory.focused().is_some());

                    if !typed.is_empty() && !pressing {
                        draft.push_str(&typed);

                        put_cursor_at_end(ui, editor_id, draft);

                        editor.request_focus();
                    }
                }

                // Focus an idle composer only when no other field owns input.
                if takes_keyboard && ui.memory(|memory| memory.focused().is_none()) {
                    editor.request_focus();
                }
            });
        });

    action
}

/// What the person chose in the row above the box.
pub enum PlaceAction {
    /// Start a new session in this folder.
    Open(std::path::PathBuf),

    AddProject,
}

/// The row above the box: the folder the agent works in. It opens, upward,
/// the projects to switch to and the way to add one.
pub fn place_row(
    ui: &mut Ui,
    place: &std::path::Path,
    scratch: &std::path::Path,
    projects: &[std::path::PathBuf],
) -> Option<PlaceAction> {
    let mut action = None;

    let name = |folder: &std::path::Path| {
        if folder == scratch {
            return "No project".to_string();
        }

        folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.display().to_string())
    };

    ui.horizontal(|ui| {
        let (project, _) = menu::dropdown(
            ui,
            Some(Icon::Folder),
            &name(place),
            true,
            Opens::Up,
            |ui| {
                menu::title(ui, "Work in");

                for folder in projects
                    .iter()
                    .map(std::path::PathBuf::as_path)
                    .chain([scratch])
                {
                    let selected = folder == place;

                    if menu::item(ui, Some(Icon::Folder), &name(folder), "", selected).clicked()
                        && !selected
                    {
                        action = Some(PlaceAction::Open(folder.to_path_buf()));
                    }
                }

                menu::separator(ui);

                if menu::item(ui, Some(Icon::FolderPlus), "Add project folder…", "", false)
                    .clicked()
                {
                    action = Some(PlaceAction::AddProject);
                }
            },
        );

        project.on_hover_text(format!("Runs on this machine, in {}", place.display()));
    });

    action
}

/// The pictures that will go with the instruction, each with a button
/// that takes it off again.
fn thumbnails(ui: &mut Ui, attachments: &mut Vec<Attachment>) {
    if attachments.is_empty() {
        return;
    }

    let mut removed = None;

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

        for (index, attachment) in attachments.iter().enumerate() {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(THUMBNAIL_SIZE, THUMBNAIL_SIZE), Sense::hover());

            match attachment::texture(ui.ctx(), &attachment.image) {
                Some(texture) => attachment::paint_cover(ui, rect, &texture, 8),

                None => {
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(crate::theme::radius::L),
                        palette::raised(),
                    );
                }
            }

            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(crate::theme::radius::L),
                Stroke::new(1.0, palette::border()),
                egui::StrokeKind::Inside,
            );

            // A round button on the picture's corner.
            let centre = rect.right_top() + egui::vec2(-3.0, 3.0);
            let button = egui::Rect::from_center_size(centre, egui::vec2(18.0, 18.0));

            let remove = ui.interact(
                button,
                ui.id().with(("remove attachment", index)),
                Sense::click(),
            );

            let (fill, cross) = if remove.hovered() {
                (palette::text(), palette::sunken())
            } else {
                (palette::sunken(), palette::text())
            };

            ui.painter().circle_filled(centre, 8.0, fill);
            ui.painter()
                .circle_stroke(centre, 8.0, Stroke::new(1.0, palette::border()));

            for direction in [egui::vec2(1.0, 1.0), egui::vec2(1.0, -1.0)] {
                ui.painter().line_segment(
                    [centre - direction * 2.8, centre + direction * 2.8],
                    Stroke::new(1.3, cross),
                );
            }

            if remove.on_hover_text("Remove").clicked() {
                removed = Some(index);
            }

            response.on_hover_text(&attachment.name);
        }
    });

    ui.add_space(6.0);

    if let Some(index) = removed {
        attachments.remove(index);
    }
}

/// Puts the caret after the last character, as if the text had been typed.
fn put_cursor_at_end(ui: &Ui, editor: egui::Id, text: &str) {
    let mut state = egui::TextEdit::load_state(ui.ctx(), editor).unwrap_or_default();

    let end = egui::text::CCursor::new(text.chars().count());

    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(end)));

    state.store(ui.ctx(), editor);
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
        .color(palette::text());

    let button = egui::Button::new(label)
        .fill(palette::raised())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(crate::theme::radius::XL))
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
    // It answers the pointer and Escape, and never holds the keyboard: a
    // Return meant for the box must not be able to stop a run.
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE), Sense::CLICK);

    let fill = if response.hovered() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    let painter = ui.painter();

    painter.circle_filled(rect.center(), BUTTON_SIZE / 2.0, fill);

    painter.rect_filled(
        egui::Rect::from_center_size(rect.center(), egui::vec2(10.0, 10.0)),
        CornerRadius::same(crate::theme::radius::XS),
        palette::sunken(),
    );

    response.on_hover_text("Stop  Esc").clicked()
}

/// A round button with an upward arrow. Returns whether it was clicked.
fn send_button(ui: &mut Ui, enabled: bool) -> bool {
    // Return sends, so the button need not hold the keyboard.
    let sense = if enabled {
        Sense::CLICK
    } else {
        Sense::hover()
    };

    let (rect, response) = ui.allocate_exact_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE), sense);

    let fill = match (enabled, response.hovered()) {
        (false, _) => palette::raised(),
        (true, false) => palette::accent(),
        (true, true) => palette::accent().gamma_multiply(1.15),
    };

    let arrow = if enabled {
        palette::sunken()
    } else {
        palette::text_muted()
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
            show(
                ui,
                &mut draft,
                &mut Attachments::default(),
                Mode::Ready,
                false,
                &[],
            );
        });
        output.textures_delta.clear();
        let input = egui::RawInput {
            events: vec![egui::Event::Text("find a session".into())],
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            ui.text_edit_singleline(&mut search);
            show(
                ui,
                &mut draft,
                &mut Attachments::default(),
                Mode::Ready,
                false,
                &[],
            );
        });

        output.textures_delta.clear();
        assert_eq!(search, "find a session");
        assert!(draft.is_empty());
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    fn working(
        context: &egui::Context,
        events: Vec<egui::Event>,
        draft: &mut String,
    ) -> Option<Action> {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };

        let mut action = None;

        let mut output = context.run_ui(input, |ui| {
            action = show(
                ui,
                draft,
                &mut Attachments::default(),
                Mode::Working,
                false,
                &[],
            );
        });

        output.textures_delta.clear();

        action
    }

    #[test]
    fn tab_during_a_run_leaves_the_box_able_to_be_typed_in() {
        let context = egui::Context::default();
        let mut draft = String::new();

        working(&context, Vec::new(), &mut draft);
        working(&context, Vec::new(), &mut draft);

        // Tab used to move the keyboard to the stop button. From there
        // nothing typed reached the box, and Return stopped the run.
        working(&context, vec![key(Key::Tab)], &mut draft);
        working(&context, Vec::new(), &mut draft);

        working(&context, vec![egui::Event::Text("wait".into())], &mut draft);

        assert_eq!(draft, "wait");

        let action = working(&context, vec![key(Key::Enter)], &mut draft);

        assert!(matches!(action, Some(Action::Steer(message)) if message == "wait"));
    }

    #[test]
    fn return_in_an_empty_box_does_not_stop_the_run() {
        let context = egui::Context::default();
        let mut draft = String::new();

        working(&context, Vec::new(), &mut draft);
        working(&context, Vec::new(), &mut draft);
        working(&context, vec![key(Key::Tab)], &mut draft);
        working(&context, Vec::new(), &mut draft);

        for pressed in [Key::Enter, Key::Space] {
            assert!(working(&context, vec![key(pressed)], &mut draft).is_none());
        }
    }

    #[test]
    fn escape_stops_a_run() {
        let context = egui::Context::default();
        let mut draft = String::new();

        working(&context, Vec::new(), &mut draft);
        working(&context, Vec::new(), &mut draft);

        let action = working(&context, vec![key(Key::Escape)], &mut draft);

        assert!(matches!(action, Some(Action::Stop)));

        // And the box takes the keyboard back for what is typed next.
        working(&context, Vec::new(), &mut draft);
        working(
            &context,
            vec![egui::Event::Text("go on".into())],
            &mut draft,
        );

        assert_eq!(draft, "go on");
    }

    #[test]
    fn up_in_an_empty_box_asks_for_the_last_message() {
        let context = egui::Context::default();
        let mut draft = String::new();

        let ready = |events: Vec<egui::Event>, draft: &mut String| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };

            let mut action = None;

            let mut output = context.run_ui(input, |ui| {
                action = show(
                    ui,
                    draft,
                    &mut Attachments::default(),
                    Mode::Ready,
                    false,
                    &[],
                );
            });

            output.textures_delta.clear();

            action
        };

        ready(Vec::new(), &mut draft);
        ready(Vec::new(), &mut draft);

        assert!(matches!(
            ready(vec![key(Key::ArrowUp)], &mut draft),
            Some(Action::Recall)
        ));

        // With something typed, Up is the box's own key again.
        draft.push_str("typed");

        assert!(ready(vec![key(Key::ArrowUp)], &mut draft).is_none());
    }
}

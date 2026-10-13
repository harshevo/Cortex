//! The box you type instructions into.

use eframe::egui::{
    self, CornerRadius, Key, KeyboardShortcut, Modifiers, RichText, Sense, Stroke, Ui,
};

use cortex_core::Image;

use super::icons::{self, Icon};
use crate::attachment::{self, Attachment};
use crate::theme::{self, palette};

const BUTTON_SIZE: f32 = 34.0;

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

    /// Carry on the run that stopped on a provider failure.
    Continue,

    /// Abandon the run in progress.
    Stop,
}

/// `can_continue` offers to carry on a run that a provider failure
/// stopped. It only shows while the composer is ready for an instruction.
/// `queued` is what has been sent to the run in progress and not yet read
/// by the model. `attachments` are the pictures that go with the next
/// instruction. `suggestion` is what the person is expected to ask for
/// next: it shows in the empty box, and Tab puts it there to send or edit.
pub fn show(
    ui: &mut Ui,
    draft: &mut String,
    attachments: &mut Vec<Attachment>,
    mode: Mode,
    can_continue: bool,
    queued: &[String],
    suggestion: Option<&str>,
) -> Option<Action> {
    let can_continue = can_continue && mode == Mode::Ready;

    let suggestion =
        suggestion.filter(|_| mode == Mode::Ready && draft.is_empty() && attachments.is_empty());

    let mut action = None;

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

        if let Some(why) = attach(attachments, &dropped) {
            action = Some(Action::Refused(why));
        }
    }

    for message in queued {
        ui.horizontal(|ui| {
            ui.add_space(14.0);

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
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            thumbnails(ui, attachments);

            ui.horizontal(|ui| {
                if takes_pictures
                    && icons::button(ui, Icon::Plus, "Attach pictures", false).clicked()
                {
                    let chosen = rfd::FileDialog::new()
                        .set_title("Attach pictures")
                        .add_filter("Pictures", &attachment::EXTENSIONS)
                        .pick_files()
                        .unwrap_or_default();

                    if let Some(why) = attach(attachments, &chosen) {
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

                let hint = match suggestion {
                    Some(suggestion) => format!("{suggestion}    Tab"),
                    None => hint.to_string(),
                };

                let editor_id = ui.id().with("composer editor");

                // Tab takes the suggestion, unless another field has the
                // keyboard and Tab is on its way there.
                let accepted = suggestion.is_some_and(|_| {
                    ui.memory(|memory| memory.focused().is_none_or(|focused| focused == editor_id))
                        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Tab))
                });

                if let (true, Some(suggestion)) = (accepted, suggestion) {
                    *draft = suggestion.to_string();
                }

                let mut reserved = if can_continue { CONTINUE_WIDTH } else { 0.0 };

                // While working there are two buttons: send and stop.
                if mode == Mode::Working {
                    reserved += BUTTON_SIZE + 8.0;
                }

                let editor = egui::TextEdit::multiline(draft)
                    .id(editor_id)
                    // While a suggestion shows, Tab is the box's own key
                    // and does not move to the next control.
                    .lock_focus(suggestion.is_some())
                    .hint_text(hint)
                    .desired_rows(1)
                    .desired_width((ui.available_width() - BUTTON_SIZE - 8.0 - reserved).max(40.0))
                    .frame(egui::Frame::NONE)
                    // Return sends; Shift+Return starts a new line.
                    .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter));

                let editor = ui.add_enabled(mode != Mode::Viewing, editor);

                if accepted {
                    put_cursor_at_end(ui, editor_id, draft);

                    editor.request_focus();
                }

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
                    let can_send = mode == Mode::Ready
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

    action
}

/// What the person chose in the row above the box.
pub enum PlaceAction {
    /// Start a new session in this folder.
    Open(std::path::PathBuf),

    AddProject,
}

/// The row above the box: where the agent runs, the folder it works in
/// (which opens the list of projects to switch to), and a button that
/// adds a project.
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
        ui.spacing_mut().item_spacing.x = 6.0;

        chip(ui, Some(Icon::Laptop), "Local", false)
            .on_hover_text("Cortex reads, edits and runs commands on this machine.");

        let project = chip(ui, Some(Icon::Folder), &name(place), true)
            .on_hover_text(place.display().to_string());

        egui::Popup::menu(&project).show(|ui| {
            ui.set_min_width(200.0);

            for folder in projects
                .iter()
                .map(std::path::PathBuf::as_path)
                .chain([scratch])
            {
                let selected = folder == place;

                if ui.selectable_label(selected, name(folder)).clicked() && !selected {
                    action = Some(PlaceAction::Open(folder.to_path_buf()));
                }
            }
        });

        if chip(ui, Some(Icon::FolderPlus), "", true)
            .on_hover_text("Add a project folder")
            .clicked()
        {
            action = Some(PlaceAction::AddProject);
        }
    });

    ui.add_space(2.0);

    action
}

/// A small rounded button with an icon and, if it is not empty, a label.
/// One that is not `clickable` only says something.
fn chip(ui: &mut Ui, icon: Option<Icon>, label: &str, clickable: bool) -> egui::Response {
    const HEIGHT: f32 = 26.0;

    let text = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(12.5),
        palette::text_secondary(),
    );

    let icon_width = if icon.is_some() { 16.0 } else { 0.0 };
    let gap = if icon.is_some() && !label.is_empty() {
        6.0
    } else {
        0.0
    };
    let width = 9.0 + icon_width + gap + text.size().x + 9.0;

    let sense = if clickable {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), sense);

    let fill = if clickable && response.hovered() {
        palette::raised()
    } else {
        palette::surface()
    };

    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);

    if let Some(icon) = icon {
        icons::paint(
            ui,
            egui::pos2(rect.left() + 9.0 + icon_width / 2.0, rect.center().y),
            icon,
            palette::text_secondary(),
        );
    }

    ui.painter().galley(
        egui::pos2(
            rect.left() + 9.0 + icon_width + gap,
            rect.center().y - text.size().y / 2.0,
        ),
        text,
        palette::text_secondary(),
    );

    response
}

/// Reads the pictures in `paths` into `attachments`. Returns what to tell
/// the person about the first one that could not be taken.
fn attach(attachments: &mut Vec<Attachment>, paths: &[std::path::PathBuf]) -> Option<String> {
    let mut refused = None;

    for path in paths {
        let loaded = if attachment::is_picture(path) {
            attachment::load(path).map_err(|error| format!("{error:#}"))
        } else {
            Err(format!(
                "{} was not attached: only pictures can be (PNG, JPEG, WebP, GIF)",
                path.display()
            ))
        };

        match loaded {
            Ok(attachment) => attachments.push(attachment),
            Err(why) => refused = refused.or(Some(why)),
        }
    }

    refused
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
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(8), palette::raised());
                }
            }

            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(8),
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
        palette::text()
    } else {
        palette::text_secondary()
    };

    let painter = ui.painter();

    painter.circle_filled(rect.center(), BUTTON_SIZE / 2.0, fill);

    painter.rect_filled(
        egui::Rect::from_center_size(rect.center(), egui::vec2(10.0, 10.0)),
        CornerRadius::same(2),
        palette::sunken(),
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
                &mut Vec::new(),
                Mode::Ready,
                false,
                &[],
                None,
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
                &mut Vec::new(),
                Mode::Ready,
                false,
                &[],
                None,
            );
        });

        output.textures_delta.clear();
        assert_eq!(search, "find a session");
        assert!(draft.is_empty());
    }

    fn frame(context: &egui::Context, events: Vec<egui::Event>, draft: &mut String) {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };

        let mut output = context.run_ui(input, |ui| {
            show(
                ui,
                draft,
                &mut Vec::new(),
                Mode::Ready,
                false,
                &[],
                Some("run the tests"),
            );
        });

        output.textures_delta.clear();
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

    #[test]
    fn tab_takes_the_suggestion_into_the_box() {
        let context = egui::Context::default();
        let mut draft = String::new();

        // The box takes the keyboard, then holds on to Tab.
        frame(&context, Vec::new(), &mut draft);
        frame(&context, Vec::new(), &mut draft);

        assert!(draft.is_empty());

        frame(&context, vec![key(Key::Tab)], &mut draft);

        assert_eq!(draft, "run the tests");

        // It is ordinary text from here on: typing adds to it.
        frame(
            &context,
            vec![egui::Event::Text(" first".into())],
            &mut draft,
        );

        assert_eq!(draft, "run the tests first");
    }

    #[test]
    fn typing_instead_leaves_the_suggestion_out() {
        let context = egui::Context::default();
        let mut draft = String::new();

        frame(&context, Vec::new(), &mut draft);
        frame(&context, Vec::new(), &mut draft);
        frame(&context, vec![egui::Event::Text("no".into())], &mut draft);
        frame(&context, vec![key(Key::Tab)], &mut draft);

        assert!(!draft.contains("run the tests"), "{draft}");
    }
}

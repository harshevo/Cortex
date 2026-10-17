//! Slash commands: `/name` typed in the composer.
//!
//! Two kinds share the list. The app's own commands act at once and never
//! reach the model. Skills are written procedures; `/name` sends the
//! skill's procedure as the instruction, which is handled where the agent
//! is built.
//!
//! Typing `/` opens the list above the composer, filtered by what follows.
//! Up and Down choose, Tab completes, Return runs.

use eframe::egui::{self, CornerRadius, Key, Modifiers, Sense, Stroke, Ui};

use crate::theme::{self, palette, radius, space};

/// A command of the app's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    New,
    Undo,
    Restore,
    Checkpoints,
    Diff,
    Notes,
    Worktree,
    Run,
    Learned,
    Trace,
    Compact,
    Tasks,
    Prepare,
    Stop,
    Settings,
    Help,
}

/// Name, what may follow it, and what it does.
const BUILTINS: [(Builtin, &str, &str, &str); 16] = [
    (Builtin::New, "new", "", "Start a new session"),
    (Builtin::Undo, "undo", "", "Revert the last run's changes"),
    (
        Builtin::Restore,
        "restore",
        "<id>",
        "Go back to a checkpoint",
    ),
    (
        Builtin::Checkpoints,
        "checkpoints",
        "",
        "List saved checkpoints",
    ),
    (
        Builtin::Diff,
        "diff",
        "",
        "Files changed since the last checkpoint",
    ),
    (Builtin::Notes, "notes", "", "What Cortex remembers here"),
    (
        Builtin::Worktree,
        "worktree",
        "<name>",
        "Work on a new branch in its own folder",
    ),
    (Builtin::Run, "run", "", "Budget, recovery and changes of the run  ⌘I"),
    (Builtin::Learned, "learned", "", "What earlier runs taught"),
    (Builtin::Trace, "trace", "", "Summary of the last run's log"),
    (Builtin::Compact, "compact", "", "Summarise older messages"),
    (Builtin::Tasks, "tasks", "", "Show or hide the task list"),
    (Builtin::Prepare, "prepare", "", "Index the project"),
    (Builtin::Stop, "stop", "", "Stop the run"),
    (Builtin::Settings, "settings", "", "Open settings"),
    (Builtin::Help, "help", "", "List commands"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,

    /// What may follow the name; empty when nothing does.
    pub hint: String,

    pub about: String,

    /// `None` for a skill.
    pub builtin: Option<Builtin>,
}

/// The app's commands, then the skills given, in name order.
pub fn entries(skills: impl IntoIterator<Item = (String, String, Option<String>)>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = BUILTINS
        .iter()
        .map(|(builtin, name, hint, about)| Entry {
            name: name.to_string(),
            hint: hint.to_string(),
            about: about.to_string(),
            builtin: Some(*builtin),
        })
        .collect();

    let mut skills: Vec<Entry> = skills
        .into_iter()
        .filter(|(name, _, _)| !entries.iter().any(|entry| entry.name == *name))
        .map(|(name, about, hint)| Entry {
            name,
            hint: hint.unwrap_or_default(),
            about: first_sentence(&about),
            builtin: None,
        })
        .collect();

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    entries.extend(skills);

    entries
}

/// The app's command in `text`, and what follows its name.
pub fn parse(text: &str) -> Option<(Builtin, String)> {
    let rest = text.trim().strip_prefix('/')?;

    let (name, arguments) = match rest.split_once(char::is_whitespace) {
        Some((name, arguments)) => (name, arguments.trim()),
        None => (rest, ""),
    };

    BUILTINS
        .iter()
        .find(|(_, known, _, _)| *known == name)
        .map(|(builtin, ..)| (*builtin, arguments.to_string()))
}

/// The commands as a short list, for `/help`.
pub fn help(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|entry| {
            let name = match entry.hint.is_empty() {
                true => format!("/{}", entry.name),
                false => format!("/{} {}", entry.name, entry.hint),
            };

            format!("{name} — {}", entry.about)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The entries `draft` could be the start of: while it is `/` and a name
/// with nothing after it. Those whose name begins with what was typed
/// come first, then those that only contain it.
pub fn matching<'a>(draft: &str, entries: &'a [Entry]) -> Vec<&'a Entry> {
    let Some(typed) = draft.strip_prefix('/') else {
        return Vec::new();
    };

    if typed.contains(char::is_whitespace) {
        return Vec::new();
    }

    let typed = typed.to_lowercase();

    let starting = entries
        .iter()
        .filter(|entry| entry.name.starts_with(&typed));
    let containing = entries
        .iter()
        .filter(|entry| !entry.name.starts_with(&typed) && entry.name.contains(&typed));

    starting.chain(containing).collect()
}

/// What the person did in the list.
pub enum Chosen {
    /// Put this in the composer, to go on typing.
    Complete(String),

    /// Run this now.
    Run(String),
}

/// Draws the list for `draft`, when there is one to draw, and handles
/// the keys that move through it. Call before the composer's editor, so
/// that the keys are taken before the editor sees them.
pub fn palette(ui: &mut Ui, draft: &str, entries: &[Entry]) -> Option<Chosen> {
    let found = matching(draft, entries);

    let selected_id = ui.id().with("command palette selected");

    if found.is_empty() {
        ui.data_mut(|data| data.remove::<usize>(selected_id));

        return None;
    }

    let mut selected = ui
        .data(|data| data.get_temp::<usize>(selected_id))
        .unwrap_or(0)
        .min(found.len() - 1);

    let mut chosen = None;

    ui.input_mut(|input| {
        if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
            selected = (selected + 1) % found.len();
        }

        if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
            selected = (selected + found.len() - 1) % found.len();
        }

        if input.consume_key(Modifiers::NONE, Key::Tab) {
            chosen = Some(complete(found[selected]));
        }

        if input.consume_key(Modifiers::NONE, Key::Enter) {
            chosen = Some(choose(found[selected], draft));
        }
    });

    // Keep the chosen row in sight of a long list.
    const SHOWN: usize = 8;

    let first = selected.saturating_sub(SHOWN - 1);

    egui::Frame::new()
        .fill(palette::surface())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(radius::L))
        .inner_margin(egui::Margin::same(space::XS as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;

            for (index, entry) in found.iter().enumerate().skip(first).take(SHOWN) {
                if row(ui, entry, index == selected).clicked() {
                    chosen = Some(choose(entry, draft));
                }
            }
        });

    ui.data_mut(|data| data.insert_temp(selected_id, selected));

    if chosen.is_some() {
        ui.data_mut(|data| data.remove::<usize>(selected_id));
    }

    chosen
}

fn complete(entry: &Entry) -> Chosen {
    match entry.hint.is_empty() && entry.builtin.is_some() {
        true => Chosen::Complete(format!("/{}", entry.name)),
        false => Chosen::Complete(format!("/{} ", entry.name)),
    }
}

/// Return on a row: a command that takes nothing runs, anything else is
/// completed so its arguments can be typed.
fn choose(entry: &Entry, draft: &str) -> Chosen {
    let exact = draft.trim() == format!("/{}", entry.name);

    match entry.hint.is_empty() || exact {
        true => Chosen::Run(format!("/{}", entry.name)),
        false => Chosen::Complete(format!("/{} ", entry.name)),
    }
}

fn row(ui: &mut Ui, entry: &Entry, selected: bool) -> egui::Response {
    const HEIGHT: f32 = 32.0;
    const NAME_WIDTH: f32 = 160.0;

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), HEIGHT), Sense::click());

    if selected || response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::M), palette::raised());
    }

    let painter = ui.painter();
    let middle = rect.center().y;
    let left = rect.left() + space::M;

    let name = painter.text(
        egui::pos2(left, middle),
        egui::Align2::LEFT_CENTER,
        format!("/{}", entry.name),
        theme::strong(theme::size::UI),
        palette::text(),
    );

    if !entry.hint.is_empty() {
        painter.text(
            egui::pos2(name.right() + space::XS, middle),
            egui::Align2::LEFT_CENTER,
            &entry.hint,
            theme::ui(),
            palette::text_muted(),
        );
    }

    let about_left = (left + NAME_WIDTH).max(name.right() + space::L);

    let room = rect.right() - space::M - about_left;
    let about = crate::format::one_line(&entry.about, (room / 6.5).max(4.0) as usize);

    painter.text(
        egui::pos2(about_left, middle),
        egui::Align2::LEFT_CENTER,
        about,
        theme::ui(),
        palette::text_secondary(),
    );

    if entry.builtin.is_none() {
        painter.text(
            egui::pos2(rect.right() - space::M, middle),
            egui::Align2::RIGHT_CENTER,
            "skill",
            theme::small(),
            palette::text_muted(),
        );
    }

    response
}

fn first_sentence(text: &str) -> String {
    let text = text.trim();

    match text.find(". ") {
        Some(end) => text[..end].to_string(),
        None => text.trim_end_matches('.').to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Vec<Entry> {
        entries([
            (
                "code-review".to_string(),
                "Review the diff. Then more.".to_string(),
                None,
            ),
            (
                "undo".to_string(),
                "A skill named like a command".to_string(),
                None,
            ),
        ])
    }

    #[test]
    fn the_apps_commands_are_parsed_with_their_arguments() {
        assert_eq!(parse("/undo"), Some((Builtin::Undo, String::new())));
        assert_eq!(
            parse("  /restore  ab12cd "),
            Some((Builtin::Restore, "ab12cd".to_string()))
        );

        // A skill, or plain text, is an instruction.
        assert_eq!(parse("/code-review main"), None);
        assert_eq!(parse("undo it"), None);
    }

    #[test]
    fn a_skill_does_not_hide_a_command_of_the_same_name() {
        let entries = library();

        let undos: Vec<_> = entries
            .iter()
            .filter(|entry| entry.name == "undo")
            .collect();

        assert_eq!(undos.len(), 1);
        assert!(undos[0].builtin.is_some());

        let review = entries
            .iter()
            .find(|entry| entry.name == "code-review")
            .unwrap();
        assert_eq!(review.about, "Review the diff");
    }

    #[test]
    fn matching_prefers_names_that_start_with_what_was_typed() {
        let entries = library();

        let found: Vec<_> = matching("/re", &entries)
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();

        assert_eq!(found.first(), Some(&"restore"));
        assert!(found.contains(&"code-review"));

        assert!(matching("/restore ab", &entries).is_empty());
        assert!(matching("restore", &entries).is_empty());
        assert_eq!(matching("/", &entries).len(), entries.len());
    }
}

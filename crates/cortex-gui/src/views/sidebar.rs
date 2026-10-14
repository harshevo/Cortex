//! The sidebar: either the project folders with their sessions or the
//! sessions that belong to no project, and the way into settings.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, CornerRadius, RichText, ScrollArea, Sense, Stroke, Ui, vec2};

use super::Section;
use super::icons::{self, Icon};
use crate::format;
use crate::session::history::PastSession;
use crate::theme::{self, palette};

const ROW_HEIGHT: f32 = 28.0;
const FOOTER_HEIGHT: f32 = 40.0;

/// Radius of the mark in front of a session's title.
const MARK_RADIUS: f32 = 2.0;

/// Sessions are indented under the project they belong to.
const INDENT: f32 = 16.0;

/// Everything the sidebar shows.
pub struct Shelf<'a> {
    /// Which of the two lists is shown.
    pub section: Section,

    pub projects: &'a [PathBuf],

    /// The folder that sessions outside any project work in.
    pub scratch: &'a Path,

    /// The past sessions of every folder, newest first.
    pub sessions: &'a HashMap<PathBuf, Vec<PastSession>>,

    /// The folder the agent is working in.
    pub active: &'a Path,

    /// The past session on screen, if one is being viewed.
    pub open: Option<&'a Path>,

    pub running: &'a [PathBuf],

    /// False while a run is in progress: nothing may be switched then.
    pub can_change: bool,

    /// Settings are on screen.
    pub in_settings: bool,
}

pub enum Action {
    AddProject,

    /// Take a project off the list. Its files are not touched.
    RemoveProject(PathBuf),

    /// Start a new session in a folder.
    NewSession(PathBuf),

    /// Show a past session.
    OpenSession(PathBuf),

    /// Delete a past session's log.
    DeleteSession(PathBuf),

    OpenSettings,
}

/// `confirming` is the session or project whose delete button has been
/// pressed once; pressing it again deletes.
pub fn show(ui: &mut Ui, shelf: &Shelf<'_>, confirming: &mut Option<PathBuf>) -> Option<Action> {
    let mut action = None;

    let search_id = ui.id().with("session_search");
    let mut query = ui
        .data(|data| data.get_temp::<String>(search_id))
        .unwrap_or_default();
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::singleline(&mut query)
            .font(egui::FontId::proportional(12.5))
            .hint_text("Search chats and projects")
            .desired_width(ui.available_width())
            .margin(egui::vec2(8.0, 5.0)),
    );
    ui.data_mut(|data| data.insert_temp(search_id, query.clone()));
    ui.add_space(8.0);
    let query = query.trim().to_lowercase();
    let list_height = (ui.available_height() - FOOTER_HEIGHT).max(0.0);

    ScrollArea::vertical()
        .auto_shrink(false)
        .max_height(list_height)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;

            action = match shelf.section {
                Section::Chats => chats(ui, shelf, confirming, &query),
                Section::Code => projects(ui, shelf, confirming, &query),
            };
        });

    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        if settings_row(ui, shelf.in_settings) {
            action = Some(Action::OpenSettings);
        }
    });

    action
}

/// The sessions that belong to no project.
fn chats(
    ui: &mut Ui,
    shelf: &Shelf<'_>,
    confirming: &mut Option<PathBuf>,
    query: &str,
) -> Option<Action> {
    let mut action = None;

    if heading(ui, "Chats", Icon::Plus, "New chat outside any project") && shelf.can_change {
        action = Some(Action::NewSession(shelf.scratch.to_path_buf()));
    }

    let sessions = shelf
        .sessions
        .get(shelf.scratch)
        .map(Vec::as_slice)
        .unwrap_or_default();

    if sessions.is_empty() {
        hint(ui, "For questions and tasks that belong to no project.");
    }

    let matches: Vec<_> = sessions
        .iter()
        .filter(|session| matches_query(&session.title, query))
        .collect();
    if !sessions.is_empty() && matches.is_empty() {
        hint(ui, "No chats match your search.");
    }
    for session in matches {
        action = session_row(ui, shelf, session, 0.0, confirming).or(action.take());
    }

    action
}

/// Every project folder with its sessions under it.
fn projects(
    ui: &mut Ui,
    shelf: &Shelf<'_>,
    confirming: &mut Option<PathBuf>,
    query: &str,
) -> Option<Action> {
    let mut action = None;

    if heading(ui, "Projects", Icon::FolderPlus, "Add a project folder") {
        action = Some(Action::AddProject);
    }

    if shelf.projects.is_empty() {
        hint(ui, "Add a folder to work on its code.");
    }

    let mut found = false;
    let mut projects: Vec<_> = shelf.projects.iter().collect();
    projects.sort_by_key(|project| {
        std::cmp::Reverse(
            shelf
                .sessions
                .get(*project)
                .and_then(|sessions| sessions.first())
                .map_or(0, |session| session.updated_ms),
        )
    });
    for project in projects {
        let project_matches = matches_query(&project.display().to_string(), query);
        let sessions = shelf
            .sessions
            .get(project)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if project_matches
            || sessions
                .iter()
                .any(|session| matches_query(&session.title, query))
        {
            found = true;
            action = project_section(
                ui,
                shelf,
                project,
                confirming,
                if project_matches { "" } else { query },
            )
            .or(action.take());
            ui.add_space(6.0);
        }
    }
    if !shelf.projects.is_empty() && !found {
        hint(ui, "No projects or chats match your search.");
    }

    action
}

/// A section title with one button at its right-hand end. Returns whether
/// the button was clicked.
fn heading(ui: &mut Ui, title: &str, icon: Icon, tooltip: &str) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .font(theme::strong(11.5))
                .color(palette::text_muted()),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            icons::button(ui, icon, tooltip, false).clicked()
        })
        .inner
    })
    .inner
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::text_muted()),
    );
}

/// One project and its sessions.
fn project_section(
    ui: &mut Ui,
    shelf: &Shelf<'_>,
    project: &Path,
    confirming: &mut Option<PathBuf>,
    query: &str,
) -> Option<Action> {
    let active = shelf.active == project;

    // Which projects are folded away, kept for as long as the app runs.
    let folded_id = egui::Id::new("sidebar folded projects");
    let mut folded = ui
        .data(|data| data.get_temp::<HashSet<PathBuf>>(folded_id))
        .unwrap_or_default();
    let mut open = !folded.contains(project);

    let mut action = project_row(ui, shelf, project, active, &mut open, confirming);

    if open == folded.contains(project) {
        if open {
            folded.remove(project);
        } else {
            folded.insert(project.to_path_buf());
        }

        ui.data_mut(|data| data.insert_temp(folded_id, folded));
    }

    // A search looks inside folded projects too.
    if !open && query.is_empty() {
        return action;
    }

    let sessions = shelf
        .sessions
        .get(project)
        .map(Vec::as_slice)
        .unwrap_or_default();

    for session in sessions
        .iter()
        .filter(|session| matches_query(&session.title, query))
    {
        action = session_row(ui, shelf, session, INDENT, confirming).or(action.take());
    }

    action
}

fn project_row(
    ui: &mut Ui,
    shelf: &Shelf<'_>,
    project: &Path,
    active: bool,
    open: &mut bool,
    confirming: &mut Option<PathBuf>,
) -> Option<Action> {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());

    let hovered = ui.rect_contains_pointer(rect);

    if hovered {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(crate::theme::radius::M),
            palette::surface(),
        );
    }

    let color = if active {
        palette::text()
    } else {
        palette::text_secondary()
    };

    // A chevron: pointing down while the sessions are shown.
    let centre = rect.left_center() + vec2(9.0, 0.0);
    let chevron = if *open {
        [vec2(-3.0, -1.5), vec2(0.0, 1.5), vec2(3.0, -1.5)]
    } else {
        [vec2(-1.5, -3.0), vec2(1.5, 0.0), vec2(-1.5, 3.0)]
    };

    ui.painter().line(
        chevron.iter().map(|offset| centre + *offset).collect(),
        Stroke::new(1.2, palette::text_muted()),
    );

    icons::paint(
        ui,
        rect.left_center() + vec2(26.0, 0.0),
        Icon::Folder,
        color,
    );

    let name = project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| project.display().to_string());

    let name_left = rect.left() + 40.0;
    let name_right = if hovered || confirming.as_deref() == Some(project) {
        icons::slot_from_right(rect, 1).left()
    } else {
        rect.right() - 8.0
    };
    elided(
        ui,
        egui::pos2(name_left, rect.center().y),
        name_right - name_left,
        &name,
        if active {
            theme::strong(13.0)
        } else {
            egui::FontId::proportional(13.0)
        },
        color,
    );

    let mut action = None;

    if hovered || confirming.as_deref() == Some(project) {
        let id = ui.id().with(("project", project));

        let remove = delete_button(
            ui,
            icons::slot_from_right(rect, 0),
            id.with("remove"),
            project,
            confirming,
            "Remove from this list. The folder and its files stay where they are.",
        );

        if remove {
            action = Some(Action::RemoveProject(project.to_path_buf()));
        }

        let add = icons::button_at(
            ui,
            icons::slot_from_right(rect, 1),
            id.with("new"),
            Icon::Plus,
            "New session in this project",
            false,
        );

        if add.clicked() && shelf.can_change {
            action = Some(Action::NewSession(project.to_path_buf()));
        }
    }

    // Clicking the row itself shows or hides the project's sessions.
    if action.is_none() && response.clicked() {
        *open = !*open;
    }

    response.on_hover_text(project.display().to_string());

    action
}

fn session_row(
    ui: &mut Ui,
    shelf: &Shelf<'_>,
    session: &PastSession,
    indent: f32,
    confirming: &mut Option<PathBuf>,
) -> Option<Action> {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());

    let selected = shelf.open == Some(session.path.as_path());
    let hovered = ui.rect_contains_pointer(rect);

    if selected || hovered {
        let fill = if selected {
            palette::raised()
        } else {
            palette::surface()
        };

        ui.painter()
            .rect_filled(rect, CornerRadius::same(crate::theme::radius::M), fill);
    }

    let left = rect.left() + 10.0 + indent;
    let running = shelf.running.iter().any(|path| path == &session.path);
    let acting = hovered || confirming.as_deref() == Some(session.path.as_path());

    // A mark in front of the title: a ring for a session at rest, a
    // pulsing dot for one that is running.
    let mark = egui::pos2(left + 4.0, rect.center().y);

    if running {
        icons::activity_dot_sized(ui, mark, MARK_RADIUS);
    } else {
        ui.painter()
            .circle_stroke(mark, MARK_RADIUS, Stroke::new(1.0, palette::text_muted()));
    }

    let text_left = left + 16.0;

    // The right-hand end holds the delete button while the pointer is
    // over the row.
    let right = if acting {
        icons::slot_from_right(rect, 0).left()
    } else {
        rect.right()
    };

    elided(
        ui,
        egui::pos2(text_left, rect.center().y),
        right - 8.0 - text_left,
        &format::one_line(&session.title, 200),
        egui::FontId::proportional(13.0),
        if selected || running {
            palette::text()
        } else {
            palette::text_secondary()
        },
    );

    let mut action = None;

    if acting {
        let delete = delete_button(
            ui,
            icons::slot_from_right(rect, 0),
            ui.id().with(("session", &session.path)),
            &session.path,
            confirming,
            "Delete this session",
        );

        if delete {
            action = Some(Action::DeleteSession(session.path.clone()));
        }
    }

    if action.is_none() && response.clicked() {
        action = Some(Action::OpenSession(session.path.clone()));
    }

    response.on_hover_text(&session.title);

    action
}

/// A delete button that has to be pressed twice. The first press arms it
/// (and it stays red); the second returns true.
fn delete_button(
    ui: &Ui,
    rect: egui::Rect,
    id: egui::Id,
    target: &Path,
    confirming: &mut Option<PathBuf>,
    tooltip: &str,
) -> bool {
    let armed = confirming.as_deref() == Some(target);

    let tooltip = if armed {
        "Click again to confirm"
    } else {
        tooltip
    };

    let response = icons::button_at(ui, rect, id, Icon::Trash, tooltip, true);

    if armed {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(crate::theme::radius::M),
            Stroke::new(1.0, palette::bad()),
            egui::StrokeKind::Inside,
        );
    }

    if !response.clicked() {
        return false;
    }

    if armed {
        *confirming = None;

        return true;
    }

    *confirming = Some(target.to_path_buf());

    false
}

fn settings_row(ui: &mut Ui, selected: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());

    if selected || response.hovered() {
        let fill = if selected {
            palette::raised()
        } else {
            palette::surface()
        };

        ui.painter()
            .rect_filled(rect, CornerRadius::same(crate::theme::radius::M), fill);
    }

    let color = if selected {
        palette::text()
    } else {
        palette::text_secondary()
    };

    icons::paint(ui, rect.left_center() + vec2(15.0, 0.0), Icon::Gear, color);

    ui.painter().text(
        rect.left_center() + vec2(30.0, 0.0),
        Align2::LEFT_CENTER,
        "Settings",
        egui::FontId::proportional(13.0),
        color,
    );

    response.clicked()
}

/// Paints one line of text, vertically centred on `left_centre` and cut
/// off with an ellipsis where it would pass `width`.
fn elided(
    ui: &Ui,
    left_centre: egui::Pos2,
    width: f32,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));

    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let position = left_centre - vec2(0.0, galley.size().y / 2.0);

    ui.painter().galley(position, galley, color);
}

fn matches_query(text: &str, query: &str) -> bool {
    query.is_empty() || text.to_lowercase().contains(query)
}

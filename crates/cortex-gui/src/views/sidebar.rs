//! The sidebar: either the project folders with their sessions or the
//! sessions that belong to no project, and the way into settings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, CornerRadius, RichText, ScrollArea, Sense, Stroke, Ui, vec2};

use super::Section;
use super::icons::{self, Icon};
use crate::format;
use crate::session::history::PastSession;
use crate::theme::{self, palette};

const SESSION_ROW_HEIGHT: f32 = 54.0;
const PROJECT_ROW_HEIGHT: f32 = 36.0;
const FOOTER_HEIGHT: f32 = 48.0;

/// Sessions are indented under the project they belong to.
const INDENT: f32 = 14.0;

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
    ui.add_space(8.0);
    ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text("Search chats and projects")
            .desired_width(ui.available_width())
            .margin(egui::vec2(10.0, 8.0)),
    );
    ui.data_mut(|data| data.insert_temp(search_id, query.clone()));
    ui.add_space(12.0);
    let query = query.trim().to_lowercase();
    let list_height = (ui.available_height() - FOOTER_HEIGHT).max(0.0);

    ScrollArea::vertical()
        .auto_shrink(false)
        .max_height(list_height)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;

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
            ui.add_space(8.0);
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
                .color(palette::TEXT_MUTED),
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
            .color(palette::TEXT_MUTED),
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

    let mut action = project_row(ui, shelf, project, active, confirming);

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
    confirming: &mut Option<PathBuf>,
) -> Option<Action> {
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), PROJECT_ROW_HEIGHT),
        Sense::click(),
    );

    let hovered = ui.rect_contains_pointer(rect);

    if hovered {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(7), palette::SURFACE);
    }

    let color = if active {
        palette::TEXT
    } else {
        palette::TEXT_SECONDARY
    };

    icons::paint(
        ui,
        rect.left_center() + vec2(15.0, 0.0),
        Icon::Folder,
        color,
    );

    let name = project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| project.display().to_string());

    ui.painter_at(rect.with_max_x(icons::slot_from_right(rect, 1).left()))
        .text(
            rect.left_center() + vec2(30.0, 0.0),
            Align2::LEFT_CENTER,
            name,
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

    // Clicking the row itself opens the project.
    if action.is_none() && response.clicked() && shelf.can_change && !active {
        action = Some(Action::NewSession(project.to_path_buf()));
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
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), SESSION_ROW_HEIGHT),
        Sense::click(),
    );

    let selected = shelf.open == Some(session.path.as_path());
    let hovered = ui.rect_contains_pointer(rect);

    if selected || hovered {
        let fill = if selected {
            palette::RAISED
        } else {
            palette::SURFACE
        };

        ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    }

    let left = rect.left() + 10.0 + indent;
    let text_area = rect.with_max_x(icons::slot_from_right(rect, 0).left());
    let painter = ui.painter_at(text_area);

    painter.text(
        egui::pos2(left, rect.top() + 9.0),
        Align2::LEFT_TOP,
        format::one_line(
            &session.title,
            ((text_area.width() - 14.0 - indent).max(0.0) / 7.0) as usize,
        ),
        egui::FontId::proportional(13.0),
        palette::TEXT,
    );

    painter.text(
        egui::pos2(left, rect.bottom() - 9.0),
        Align2::LEFT_BOTTOM,
        if shelf.running.iter().any(|path| path == &session.path) {
            String::new()
        } else {
            format::ago(session.updated_ms)
        },
        theme::small(),
        palette::TEXT_MUTED,
    );

    if shelf.running.iter().any(|path| path == &session.path) {
        let dot = egui::pos2(left + 4.0, rect.bottom() - 15.0);
        icons::activity_dot(ui, dot);
        ui.painter().text(
            dot + vec2(10.0, 0.0),
            Align2::LEFT_CENTER,
            "Working",
            theme::small(),
            palette::TEXT_SECONDARY,
        );
    }

    let mut action = None;

    if hovered || confirming.as_deref() == Some(session.path.as_path()) {
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
            CornerRadius::same(6),
            Stroke::new(1.0, palette::BAD),
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
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());

    if selected || response.hovered() {
        let fill = if selected {
            palette::RAISED
        } else {
            palette::SURFACE
        };

        ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    }

    let color = if selected {
        palette::TEXT
    } else {
        palette::TEXT_SECONDARY
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

fn matches_query(text: &str, query: &str) -> bool {
    query.is_empty() || text.to_lowercase().contains(query)
}

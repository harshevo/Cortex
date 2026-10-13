//! The files of the project, on the right: a tree to look around in and,
//! beside it, the file that was picked.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, CornerRadius, RichText, ScrollArea, Sense, Stroke, Ui, vec2};

use super::icons::{self, Icon};
use crate::theme::{self, palette};

const ROW_HEIGHT: f32 = 24.0;
const INDENT: f32 = 14.0;

/// Width of the tree when a file is open beside it.
const TREE_WIDTH: f32 = 210.0;

/// How long a folder's listing is trusted before it is read again, so
/// that files the agent makes show up without a click.
const LISTING_AGE: Duration = Duration::from_secs(2);

/// Largest file read to be shown.
const MAX_FILE_BYTES: u64 = 1_000_000;

/// Most places looked at, and most matches listed, for one filter.
const MAX_SEARCHED: usize = 20_000;
const MAX_MATCHES: usize = 200;

/// Folders a filter does not look into: build output and the like.
const SKIPPED_IN_SEARCH: [&str; 5] = [".git", "target", "node_modules", ".venv", "__pycache__"];

/// Never listed.
const HIDDEN: [&str; 2] = [".git", ".DS_Store"];

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    name: String,
    folder: bool,
}

/// What the viewer shows of the file that was picked.
enum Shown {
    Text(Vec<String>),

    /// Why the file is not shown.
    Not(String),
}

struct Open {
    path: PathBuf,
    shown: Shown,
}

/// The state of the panel, kept between frames.
#[derive(Default)]
pub struct Files {
    /// The panel is on screen.
    pub visible: bool,

    /// The folder the tree was last drawn for.
    root: PathBuf,

    expanded: HashSet<PathBuf>,

    /// What each folder holds, and when that was read.
    listings: HashMap<PathBuf, (Instant, Vec<Entry>)>,

    filter: String,

    /// The filter the matches belong to, and the matches.
    matches: Option<(String, Vec<Entry>)>,

    open: Option<Open>,

    /// A file to open once the project is known.
    pending: Option<PathBuf>,

    /// The file the window was asked to open with. Opened again when the
    /// project changes, which it does once while a demo starts.
    at_launch: Option<PathBuf>,
}

impl Files {
    /// The panel as it is when the window opens: `visible` or not, and
    /// with `file` open in it if there is one.
    pub fn opened(visible: bool, file: Option<PathBuf>) -> Self {
        Self {
            visible,
            pending: file.clone(),
            at_launch: file,
            ..Self::default()
        }
    }

    /// Whether a file is open beside the tree.
    pub fn has_file(&self) -> bool {
        self.open.is_some()
    }

    /// Draws the panel's contents for the project in `root`.
    pub fn show(&mut self, ui: &mut Ui, root: &Path) {
        if self.root != root {
            *self = Self {
                visible: self.visible,
                root: root.to_path_buf(),
                pending: self.at_launch.clone(),
                at_launch: self.at_launch.take(),
                ..Self::default()
            };
        }

        if let Some(path) = self.pending.take() {
            self.open = Some(Open {
                shown: read(&path),
                path,
            });
        }

        let mut picked = None;
        let mut closed = false;

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            let tree_width = if self.open.is_some() {
                TREE_WIDTH.min(ui.available_width() * 0.5)
            } else {
                ui.available_width()
            };

            ui.allocate_ui_with_layout(
                vec2(tree_width, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(tree_width);

                    picked = self.tree(ui);
                },
            );

            if let Some(open) = &self.open {
                let (line, _) =
                    ui.allocate_exact_size(vec2(9.0, ui.available_height()), Sense::hover());

                ui.painter().vline(
                    line.center().x,
                    line.y_range(),
                    Stroke::new(1.0, palette::border()),
                );

                ui.vertical(|ui| closed = viewer(ui, &self.root, open));
            }
        });

        if closed {
            self.open = None;
        }

        if let Some(path) = picked {
            self.open = Some(Open {
                shown: read(&path),
                path,
            });
        }
    }

    /// The filter box and, under it, the tree or the filter's matches.
    /// Returns the file that was clicked.
    fn tree(&mut self, ui: &mut Ui) -> Option<PathBuf> {
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter files")
                .font(egui::FontId::proportional(12.5))
                .desired_width(ui.available_width())
                .margin(vec2(8.0, 5.0)),
        );

        ui.add_space(6.0);

        let query = self.filter.trim().to_lowercase();
        let mut picked = None;

        ScrollArea::vertical()
            .id_salt("file tree")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;

                if query.is_empty() {
                    self.matches = None;

                    let root = self.root.clone();

                    self.folder(ui, &root, 0, &mut picked);

                    return;
                }

                if self.matches.as_ref().is_none_or(|(kept, _)| *kept != query) {
                    self.matches = Some((query.clone(), search(&self.root, &query)));
                }

                let matches = self
                    .matches
                    .as_ref()
                    .map(|(_, matches)| matches.clone())
                    .unwrap_or_default();

                if matches.is_empty() {
                    ui.label(
                        RichText::new("No files match.")
                            .font(theme::small())
                            .color(palette::text_muted()),
                    );
                }

                for entry in &matches {
                    let shown = entry
                        .path
                        .strip_prefix(&self.root)
                        .unwrap_or(&entry.path)
                        .display()
                        .to_string();

                    let selected = self.is_open(&entry.path);

                    if row(ui, &shown, 0, None, selected).clicked() {
                        picked = Some(entry.path.clone());
                    }
                }
            });

        picked
    }

    /// Draws what `folder` holds, and under each open folder what that
    /// one holds.
    fn folder(&mut self, ui: &mut Ui, folder: &Path, depth: usize, picked: &mut Option<PathBuf>) {
        for entry in self.listing(folder) {
            if entry.folder {
                let open = self.expanded.contains(&entry.path);

                if row(ui, &entry.name, depth, Some(open), false).clicked() {
                    if open {
                        self.expanded.remove(&entry.path);
                    } else {
                        self.expanded.insert(entry.path.clone());
                    }
                }

                if open {
                    self.folder(ui, &entry.path, depth + 1, picked);
                }
            } else if row(ui, &entry.name, depth, None, self.is_open(&entry.path)).clicked() {
                *picked = Some(entry.path.clone());
            }
        }
    }

    fn is_open(&self, path: &Path) -> bool {
        self.open.as_ref().is_some_and(|open| open.path == path)
    }

    /// What `folder` holds: folders first, then files, each by name.
    fn listing(&mut self, folder: &Path) -> Vec<Entry> {
        if let Some((read, entries)) = self.listings.get(folder)
            && read.elapsed() < LISTING_AGE
        {
            return entries.clone();
        }

        let entries = list(folder);

        self.listings
            .insert(folder.to_path_buf(), (Instant::now(), entries.clone()));

        entries
    }
}

fn list(folder: &Path) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(folder) else {
        return Vec::new();
    };

    let mut entries: Vec<Entry> = read
        .flatten()
        .map(|entry| Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            folder: entry.path().is_dir(),
            path: entry.path(),
        })
        .filter(|entry| !HIDDEN.contains(&entry.name.as_str()))
        .collect();

    entries.sort_by_key(|entry| (!entry.folder, entry.name.to_lowercase()));

    entries
}

/// The files under `root` whose path has `query` in it, nearest first.
fn search(root: &Path, query: &str) -> Vec<Entry> {
    let mut matches = Vec::new();
    let mut waiting = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut searched = 0;

    while let Some(folder) = waiting.pop_front() {
        for entry in list(&folder) {
            searched += 1;

            if searched > MAX_SEARCHED || matches.len() >= MAX_MATCHES {
                return matches;
            }

            if entry.folder {
                if !SKIPPED_IN_SEARCH.contains(&entry.name.as_str()) {
                    waiting.push_back(entry.path);
                }

                continue;
            }

            let relative = entry.path.strip_prefix(root).unwrap_or(&entry.path);

            if relative.to_string_lossy().to_lowercase().contains(query) {
                matches.push(entry);
            }
        }
    }

    matches
}

fn read(path: &Path) -> Shown {
    let size = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) => return Shown::Not(format!("This file cannot be read: {error}")),
    };

    if size > MAX_FILE_BYTES {
        return Shown::Not(format!(
            "This file is too large to show ({} KB).",
            size / 1000
        ));
    }

    match std::fs::read(path).map(String::from_utf8) {
        Ok(Ok(text)) => Shown::Text(text.lines().map(str::to_string).collect()),
        Ok(Err(_)) => Shown::Not("This file is not text.".to_string()),
        Err(error) => Shown::Not(format!("This file cannot be read: {error}")),
    }
}

/// One line of the tree. `open` is `None` for a file, and for a folder
/// whether it is unfolded.
fn row(
    ui: &mut Ui,
    name: &str,
    depth: usize,
    open: Option<bool>,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());

    if selected || response.hovered() {
        let fill = if selected {
            palette::raised()
        } else {
            palette::surface()
        };

        ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    }

    let left = rect.left() + 8.0 + depth as f32 * INDENT;
    let centre = egui::pos2(left + 4.0, rect.center().y);

    if let Some(open) = open {
        let chevron = if open {
            [vec2(-3.0, -1.5), vec2(0.0, 1.5), vec2(3.0, -1.5)]
        } else {
            [vec2(-1.5, -3.0), vec2(1.5, 0.0), vec2(-1.5, 3.0)]
        };

        ui.painter().line(
            chevron.iter().map(|offset| centre + *offset).collect(),
            Stroke::new(1.2, palette::text_muted()),
        );
    }

    let color = if selected {
        palette::text()
    } else {
        palette::text_secondary()
    };

    let mut job = egui::text::LayoutJob::simple_singleline(
        name.to_string(),
        egui::FontId::proportional(13.0),
        color,
    );

    let text_left = left + 14.0;

    job.wrap =
        egui::text::TextWrapping::truncate_at_width((rect.right() - 6.0 - text_left).max(0.0));

    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));

    ui.painter().galley(
        egui::pos2(text_left, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );

    response
}

/// The file that was picked: its path, a button that closes it, and its
/// lines with their numbers. Returns whether it was closed.
fn viewer(ui: &mut Ui, root: &Path, open: &Open) -> bool {
    let mut closed = false;

    let relative = open.path.strip_prefix(root).unwrap_or(&open.path);

    ui.horizontal(|ui| {
        ui.set_height(26.0);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if close_button(ui).clicked() {
                closed = true;
            }

            if let Shown::Text(lines) = &open.shown
                && icons::button(ui, Icon::Copy, "Copy the file", false).clicked()
            {
                ui.ctx().copy_text(lines.join("\n"));
            }

            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(relative.display().to_string())
                            .font(theme::small())
                            .color(palette::text_secondary()),
                    )
                    .truncate(),
                );
            });
        });
    });

    ui.add_space(4.0);

    let lines = match &open.shown {
        Shown::Text(lines) => lines,

        Shown::Not(why) => {
            ui.label(RichText::new(why).color(palette::text_muted()));

            return closed;
        }
    };

    let font = theme::mono();
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font)) + 3.0;

    // Wide enough for the last line's number.
    let digits = lines.len().max(1).to_string().len();
    let number_width = digits as f32 * 7.5 + 14.0;

    ScrollArea::both()
        .id_salt(("file", &open.path))
        .auto_shrink(false)
        .show_rows(ui, row_height, lines.len(), |ui, rows| {
            ui.spacing_mut().item_spacing.y = 0.0;

            for index in rows {
                ui.horizontal(|ui| {
                    ui.set_height(row_height);

                    let (number, _) =
                        ui.allocate_exact_size(vec2(number_width, row_height), Sense::hover());

                    ui.painter().text(
                        number.right_center() - vec2(12.0, 0.0),
                        Align2::RIGHT_CENTER,
                        index + 1,
                        font.clone(),
                        palette::text_muted(),
                    );

                    ui.add(
                        egui::Label::new(
                            RichText::new(lines[index].as_str())
                                .font(font.clone())
                                .color(palette::text()),
                        )
                        .extend(),
                    );
                });
            }
        });

    closed
}

/// A small button with a cross.
fn close_button(ui: &mut Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), palette::raised());
    }

    let color = if response.hovered() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    for direction in [vec2(1.0, 1.0), vec2(1.0, -1.0)] {
        ui.painter().line_segment(
            [
                rect.center() - direction * 4.0,
                rect.center() + direction * 4.0,
            ],
            Stroke::new(1.3, color),
        );
    }

    response.on_hover_text("Close the file")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(folder.path().join("src")).unwrap();
        std::fs::create_dir_all(folder.path().join("target/debug")).unwrap();
        std::fs::create_dir_all(folder.path().join(".git")).unwrap();
        std::fs::write(folder.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(folder.path().join("target/debug/main.rs"), "").unwrap();
        std::fs::write(folder.path().join("Cargo.toml"), "[package]\n").unwrap();

        folder
    }

    #[test]
    fn folders_are_listed_before_files_and_git_is_left_out() {
        let project = project();

        let names: Vec<_> = list(project.path())
            .into_iter()
            .map(|entry| entry.name)
            .collect();

        assert_eq!(names, ["src", "target", "Cargo.toml"]);
    }

    #[test]
    fn a_filter_finds_files_by_path_and_skips_build_output() {
        let project = project();

        let found = search(project.path(), "main");

        assert_eq!(found.len(), 1);
        assert!(found[0].path.ends_with("src/main.rs"));
    }

    #[test]
    fn a_file_that_is_not_text_is_not_shown() {
        let project = project();
        let path = project.path().join("blob.bin");

        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();

        assert!(matches!(read(&path), Shown::Not(_)));
        assert!(
            matches!(read(&project.path().join("Cargo.toml")), Shown::Text(lines) if lines == ["[package]"])
        );
    }
}

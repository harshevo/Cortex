//! The details panel on the right: where the agent works, how full the
//! model's context is, and what has changed in the project since the last
//! commit. Switched on and off from the top bar.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cortex_core::ContextUse;
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Ui, vec2};

use super::icons::{self, Icon};
use crate::background::Background;
use crate::details::{self, Branch, Snapshot};
use crate::format;
use crate::theme::{self, palette, radius, space};

/// How often what git says is read again while the panel is open.
const REFRESH: Duration = Duration::from_secs(5);

const ROW_HEIGHT: f32 = 24.0;
const SWATCH: f32 = 10.0;
const BAR_HEIGHT: f32 = 6.0;
const COLLAPSE_BELOW_WIDTH: f32 = 1100.0;

/// What the person asked for from the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Replace the older part of the conversation with a summary now.
    Compact,

    /// Show the files of the project.
    ShowFiles,
}

/// What the panel is told about the session on screen.
pub struct Facts<'a> {
    pub place: &'a Path,

    /// The session works outside any project.
    pub outside_project: bool,

    pub model: Option<&'a str>,
    pub context: Option<&'a ContextUse>,

    /// The share of the window at which the conversation is compacted on
    /// its own. `None` when it never is.
    pub auto_compact_at: Option<f32>,
    pub compactions: u32,

    /// Compacting now is allowed: the agent is ready and idle.
    pub can_compact: bool,
}

/// The panel's own state: whether it is open, which cards are folded,
/// and the latest reading of the folder.
pub struct Details {
    pub visible: bool,
    compact_window: bool,

    environment_open: bool,
    context_open: bool,
    changes_open: bool,

    /// The latest reading, and the folder it is of.
    snapshot: Option<(PathBuf, Snapshot)>,
    job: Background<(PathBuf, Snapshot)>,
    job_for: Option<PathBuf>,
    read_at: Option<Instant>,

    /// A run finished while a reading was under way: read again after.
    again: bool,
    was_running: bool,
}

impl Default for Details {
    fn default() -> Self {
        Self {
            visible: false,
            compact_window: false,
            environment_open: true,
            context_open: true,
            changes_open: true,
            snapshot: None,
            job: Background::default(),
            job_for: None,
            read_at: None,
            again: false,
            was_running: false,
        }
    }
}

impl Details {
    pub fn adapt_to_width(&mut self, width: f32) {
        let compact = width < COLLAPSE_BELOW_WIDTH;
        if compact && !self.compact_window {
            self.visible = false;
        }
        self.compact_window = compact;
    }

    /// The panel, shown from the start when `visible`.
    pub fn opened(visible: bool) -> Self {
        Self {
            visible,
            ..Self::default()
        }
    }

    /// Takes in a finished reading and starts another when the one shown
    /// is missing, of another folder, old, or from before a run ended.
    /// Called on every frame, open or not.
    pub fn refresh(&mut self, context: &egui::Context, place: &Path, running: bool) {
        if let Some((folder, snapshot)) = self.job.poll() {
            self.job_for = None;

            if folder == place {
                self.snapshot = Some((folder, snapshot));
            }
        }

        // The agent changes files while it runs: read again once it stops.
        if self.was_running && !running {
            self.again = true;
        }

        self.was_running = running;

        if !self.visible {
            return;
        }

        // A reading of a folder no longer on screen is of no use.
        if self.job.pending() && self.job_for.as_deref() != Some(place) {
            self.job.cancel();
            self.job_for = None;
        }

        let current = matches!(&self.snapshot, Some((folder, _)) if folder == place);
        let old = self.read_at.is_none_or(|at| at.elapsed() >= REFRESH);

        if !self.job.pending() && (!current || old || self.again) {
            let folder = place.to_path_buf();

            let accepted = self.job.start(context, move |cancelled| {
                let snapshot = details::read(&folder, cancelled);

                (folder, snapshot)
            });

            if accepted {
                self.job_for = Some(place.to_path_buf());
                self.read_at = Some(Instant::now());
                self.again = false;
            }
        }

        // Come back to read again, even with nothing else going on.
        context.request_repaint_after(REFRESH);
    }

    /// The reading of `place`, when there is one.
    fn snapshot_of(&self, place: &Path) -> Option<&Snapshot> {
        self.snapshot
            .as_ref()
            .filter(|(folder, _)| folder == place)
            .map(|(_, snapshot)| snapshot)
    }
}

/// Draws the panel's content.
pub fn show(ui: &mut Ui, details: &mut Details, facts: &Facts<'_>) -> Option<Action> {
    let mut action = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = space::M;

            let snapshot = details.snapshot_of(facts.place).cloned();

            let mut open = details.environment_open;
            section(ui, "Environment", &mut open, |ui| {
                environment(ui, facts, snapshot.as_ref());
            });
            details.environment_open = open;

            let mut open = details.context_open;
            section(ui, "Context", &mut open, |ui| {
                if context(ui, facts) {
                    action = Some(Action::Compact);
                }
            });
            details.context_open = open;

            let mut open = details.changes_open;
            section(ui, "Changes", &mut open, |ui| {
                if changes(ui, snapshot.as_ref()) {
                    action = Some(Action::ShowFiles);
                }
            });
            details.changes_open = open;
        });

    action
}

/// A collapsible card for one group of session details.
fn section(ui: &mut Ui, title: &str, open: &mut bool, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(if palette::current().dark {
            palette::surface()
        } else {
            palette::background()
        })
        .stroke(Stroke::new(1.0, palette::border().gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(radius::XL))
        .inner_margin(egui::Margin::same(space::M as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = space::XS;

            let header = header(ui, title, *open);

            if header.clicked() {
                *open = !*open;
            }

            if *open {
                ui.add_space(space::XS);
                body(ui);
            }
        });
}

fn header(ui: &mut Ui, title: &str, open: bool) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(title.to_string(), theme::strong(13.0), palette::text());

    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), galley.size().y.max(20.0)),
        Sense::click(),
    );

    let color = if response.hovered() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    let text_at = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);

    ui.painter().galley(text_at, galley, palette::text());

    let centre = egui::pos2(rect.right() - space::XS, rect.center().y + 0.5);

    if open {
        icons::chevron(ui, centre, color);
    } else {
        ui.painter().line(
            vec![
                centre + vec2(-1.5, -3.0),
                centre + vec2(1.5, 0.0),
                centre + vec2(-1.5, 3.0),
            ],
            Stroke::new(1.2, color),
        );
    }

    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if open { "Fold" } else { "Unfold" })
}

/// An icon, a name and, after a dot, something quieter.
fn row(ui: &mut Ui, icon: Icon, text: &str, quiet: Option<&str>, color: Color32) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());

    icons::paint(
        ui,
        egui::pos2(rect.left() + 8.0, rect.center().y),
        icon,
        palette::text_secondary(),
    );

    let left = rect.left() + 26.0;
    let room = (rect.right() - left).max(0.0);

    let mut job = egui::text::LayoutJob::default();

    job.append(text, 0.0, egui::TextFormat::simple(theme::ui(), color));

    if let Some(quiet) = quiet {
        job.append(
            &format!("  ·  {quiet}"),
            0.0,
            egui::TextFormat::simple(theme::ui(), palette::text_muted()),
        );
    }

    job.wrap = egui::text::TextWrapping::truncate_at_width(room);

    let galley = ui.painter().layout_job(job);
    let clipped = galley.elided;

    ui.painter().galley(
        egui::pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );

    if clipped {
        let full = match quiet {
            Some(quiet) => format!("{text}  ·  {quiet}"),
            None => text.to_string(),
        };

        response.on_hover_text(full);
    }
}

fn environment(ui: &mut Ui, facts: &Facts<'_>, snapshot: Option<&Snapshot>) {
    let machine = snapshot
        .and_then(|snapshot| snapshot.machine.clone())
        .or_else(details::cached_machine_name)
        .unwrap_or_else(|| "This computer".to_string());

    row(ui, Icon::Laptop, &machine, None, palette::text());

    let name = facts
        .place
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| facts.place.display().to_string());

    if facts.outside_project {
        row(ui, Icon::Folder, "No project", Some(&name), palette::text());
    } else {
        let parent = facts.place.parent().map(home_relative);

        row(ui, Icon::Folder, &name, parent.as_deref(), palette::text());
    }

    match snapshot {
        None => row(ui, Icon::Branch, "Reading…", None, palette::text_muted()),
        Some(Snapshot { git: None, .. }) => row(
            ui,
            Icon::Branch,
            "Not a git repository",
            None,
            palette::text_muted(),
        ),
        Some(Snapshot { git: Some(git), .. }) => match &git.branch {
            Branch::Named(name) => row(ui, Icon::Branch, name, None, palette::text()),
            Branch::Detached(commit) => {
                row(ui, Icon::Branch, commit, Some("detached"), palette::text())
            }
        },
    }

    if let Some(model) = facts.model.filter(|model| !model.is_empty()) {
        row(ui, Icon::Chat, model, None, palette::text());
    }
}

/// `~/projects` for a folder under the home folder.
fn home_relative(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }

    path.display().to_string()
}

/// One part of what fills the window.
struct Part {
    name: &'static str,
    tokens: u64,
    color: Color32,
}

/// The parts of the context, largest first, without the empty ones.
fn parts(context: &ContextUse) -> Vec<Part> {
    let tokens = &context.estimated_tokens;

    let mut parts = vec![
        Part {
            name: "Messages",
            tokens: tokens.user + tokens.assistant,
            color: palette::accent(),
        },
        Part {
            name: "Tool results",
            tokens: tokens.tool_results,
            color: palette::warning(),
        },
        Part {
            name: "Tool calls",
            tokens: tokens.tool_calls,
            color: palette::good(),
        },
        Part {
            name: "System prompt",
            tokens: tokens.system,
            color: palette::text_secondary(),
        },
        Part {
            name: "Tool descriptions",
            tokens: tokens.tool_specs,
            color: palette::model(),
        },
        Part {
            name: "Briefings and advice",
            tokens: tokens.briefings,
            color: palette::bad(),
        },
        Part {
            name: "Summary",
            tokens: tokens.summary,
            color: palette::code(),
        },
        Part {
            name: "Other",
            tokens: tokens.other,
            color: palette::text_muted(),
        },
    ];

    parts.retain(|part| part.tokens > 0);
    parts.sort_by_key(|part| std::cmp::Reverse(part.tokens));

    parts
}

/// Returns whether "Compact session" was pressed.
fn context(ui: &mut Ui, facts: &Facts<'_>) -> bool {
    let Some(context) = facts.context else {
        ui.label(
            RichText::new("No context yet")
                .font(theme::ui())
                .color(palette::text_muted()),
        );

        return false;
    };

    let about = if context.estimated { "~" } else { "" };
    let window = context.window.filter(|window| *window > 0);

    // The headline: how full, and of how much.
    ui.horizontal(|ui| {
        let headline = match context.share {
            Some(share) => format!("{about}{} used", format::percent(share)),
            None => format!("{about}{} tokens", format::count(context.tokens)),
        };

        ui.label(
            RichText::new(headline)
                .font(theme::ui())
                .color(palette::text()),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let of = match window {
                Some(window) => format!(
                    "{about}{} / {}",
                    format::count(context.tokens),
                    format::count(window)
                ),
                None => "Limit unknown".to_string(),
            };

            ui.label(
                RichText::new(of)
                    .font(theme::ui())
                    .color(palette::text_muted()),
            );
        });
    });

    let parts = parts(context);

    // The share of the window kept free so that compaction has room.
    let buffer = window
        .zip(facts.auto_compact_at)
        .map(|(window, at)| (window as f64 * (1.0 - f64::from(at.clamp(0.0, 1.0)))) as u64)
        .unwrap_or(0);

    let scale = window.unwrap_or(context.tokens).max(1);

    bar(ui, &parts, buffer, scale);

    ui.add_space(space::XS);

    let share_of = |tokens: u64| match window {
        Some(window) => format!("{:.1}%", tokens as f64 * 100.0 / window as f64),
        None => String::new(),
    };

    for part in &parts {
        legend(
            ui,
            part.color,
            part.name,
            &format::count(part.tokens),
            &share_of(part.tokens),
        );
    }

    if let Some(window) = window {
        if buffer > 0 {
            legend(
                ui,
                palette::border(),
                "Autocompact buffer",
                &format::count(buffer),
                &share_of(buffer),
            );
        }

        let free = window.saturating_sub(context.tokens).saturating_sub(buffer);

        legend(
            ui,
            palette::sunken(),
            "Free space",
            &format::count(free),
            &share_of(free),
        );
    }

    ui.add_space(space::XS);

    compact_row(ui, facts, Some(context))
}

/// The bar of what fills the window, each part in its colour, then the
/// buffer kept for compaction.
fn bar(ui: &mut Ui, parts: &[Part], buffer: u64, scale: u64) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, CornerRadius::same(radius::S), palette::sunken());

    let width = rect.width();
    let mut left = rect.left();

    let mut segment = |tokens: u64, color: Color32| {
        let share = (tokens as f64 / scale as f64).clamp(0.0, 1.0) as f32;
        let right = (left + share * width).min(rect.right());

        if right - left >= 0.5 {
            // A hair of a gap between parts, as long as there is room.
            let gap = if right - left > 3.0 { 1.0 } else { 0.0 };

            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(left, rect.top()),
                    egui::pos2(right - gap, rect.bottom()),
                ),
                CornerRadius::same(radius::XS),
                color,
            );
        }

        left = right;
    };

    for part in parts {
        segment(part.tokens, part.color);
    }

    // The buffer sits at the far end of the window, where compaction
    // starts.
    if buffer > 0 {
        let share = (buffer as f64 / scale as f64).clamp(0.0, 1.0) as f32;
        let from = (rect.right() - share * width).max(left);

        if rect.right() - from >= 0.5 {
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(from, rect.top()), rect.max),
                CornerRadius::same(radius::XS),
                palette::border(),
            );
        }
    }
}

/// One line of the legend: swatch, name, tokens and share.
fn legend(ui: &mut Ui, color: Color32, name: &str, tokens: &str, share: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + SWATCH / 2.0, rect.center().y),
            vec2(SWATCH, SWATCH),
        ),
        CornerRadius::same(radius::XS),
        color,
    );

    painter.rect_stroke(
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + SWATCH / 2.0, rect.center().y),
            vec2(SWATCH, SWATCH),
        ),
        CornerRadius::same(radius::XS),
        Stroke::new(1.0, palette::border()),
        egui::StrokeKind::Inside,
    );

    // Without a share the tokens take its place at the right-hand end.
    let share_width = if share.is_empty() {
        0.0
    } else {
        48.0 + space::S
    };
    let tokens_right = rect.right() - share_width;

    painter.text(
        egui::pos2(rect.right(), rect.center().y),
        egui::Align2::RIGHT_CENTER,
        share,
        theme::small(),
        palette::text_secondary(),
    );

    let tokens_galley =
        painter.layout_no_wrap(tokens.to_string(), theme::small(), palette::text_muted());
    let tokens_width = tokens_galley.size().x;

    painter.galley(
        egui::pos2(
            tokens_right - tokens_width,
            rect.center().y - tokens_galley.size().y / 2.0,
        ),
        tokens_galley,
        palette::text_muted(),
    );

    let name_left = rect.left() + SWATCH + space::S;
    let room = (tokens_right - tokens_width - space::S - name_left).max(0.0);

    let mut job = egui::text::LayoutJob::simple_singleline(
        name.to_string(),
        theme::small(),
        palette::text_secondary(),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(room);

    let name_galley = painter.layout_job(job);

    painter.galley(
        egui::pos2(name_left, rect.center().y - name_galley.size().y / 2.0),
        name_galley,
        palette::text_secondary(),
    );
}

/// How far off compaction is, and the button that does it now. Returns
/// whether the button was pressed.
fn compact_row(ui: &mut Ui, facts: &Facts<'_>, context: Option<&ContextUse>) -> bool {
    let until = context.and_then(|context| {
        let window = context.window.filter(|window| *window > 0)?;
        let at = facts.auto_compact_at?;
        let threshold = (window as f64 * f64::from(at.clamp(0.0, 1.0))) as u64;

        Some(threshold.saturating_sub(context.tokens))
    });

    let note = match (until, facts.auto_compact_at) {
        (Some(0), _) => "Compacts at the next step".to_string(),
        (Some(until), _) => format!("{} until auto-compact", format::count(until)),
        (None, None) => "Auto-compact is off".to_string(),
        (None, Some(_)) if context.is_some_and(|context| context.window.is_none()) => {
            "Limit unknown".to_string()
        }
        (None, Some(_)) => String::new(),
    };

    let note = match facts.compactions {
        0 => note,
        1 => format!(
            "{note}{}compacted once",
            if note.is_empty() { "" } else { " · " }
        ),
        times => format!(
            "{note}{}compacted {times}×",
            if note.is_empty() { "" } else { " · " }
        ),
    };

    let mut pressed = false;

    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let button = egui::Button::new(
                RichText::new("Compact")
                    .font(theme::small())
                    .color(palette::text_secondary()),
            )
            .fill(palette::surface())
            .stroke(Stroke::new(1.0, palette::border()))
            .corner_radius(CornerRadius::same(radius::M));

            let can = facts.can_compact && context.is_some();

            let response =
                ui.add_enabled(can, button)
                    .on_disabled_hover_text(if context.is_none() {
                        "Nothing to compact yet"
                    } else {
                        "Wait for the agent to finish"
                    });

            if response
                .on_hover_text("Replace the older part of the conversation with a summary")
                .clicked()
            {
                pressed = true;
            }

            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(note)
                            .font(theme::small())
                            .color(palette::text_muted()),
                    )
                    .truncate(),
                );
            });
        });
    });

    pressed
}

/// Returns whether "Show files" was pressed.
fn changes(ui: &mut Ui, snapshot: Option<&Snapshot>) -> bool {
    let quiet = |ui: &mut Ui, text: &str| {
        ui.label(
            RichText::new(text)
                .font(theme::ui())
                .color(palette::text_muted()),
        );
    };

    let Some(snapshot) = snapshot else {
        ui.horizontal(|ui| {
            ui.spinner();
            quiet(ui, "Reading…");
        });

        return false;
    };

    let Some(git) = &snapshot.git else {
        quiet(ui, "Not a git repository");

        return false;
    };

    let Some(changes) = git.changes else {
        quiet(ui, "Could not count the changes");

        return false;
    };

    if changes.is_empty() {
        quiet(ui, "No changes since the last commit");

        return false;
    }

    let mut pressed = false;

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, ROW_HEIGHT), Sense::hover());

        icons::paint(ui, rect.center(), Icon::Diff, palette::text_secondary());

        ui.label(
            RichText::new(format!("+{}", group(changes.added)))
                .font(theme::ui())
                .color(palette::good()),
        );

        ui.label(
            RichText::new(format!("−{}", group(changes.removed)))
                .font(theme::ui())
                .color(palette::bad()),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let files = egui::Button::new(
                RichText::new(match changes.files {
                    1 => "1 file".to_string(),
                    files => format!("{files} files"),
                })
                .font(theme::small())
                .color(palette::text_secondary()),
            )
            .fill(palette::surface())
            .stroke(Stroke::new(1.0, palette::border()))
            .corner_radius(CornerRadius::same(radius::M));

            if ui.add(files).on_hover_text("Show the files  ⌘E").clicked() {
                pressed = true;
            }
        });
    });

    if changes.untracked > 0 {
        quiet(
            ui,
            &match changes.untracked {
                1 => "1 untracked file".to_string(),
                new => format!("{new} untracked files"),
            },
        );
    }

    pressed
}

/// `8,773`.
fn group(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }

        out.push(digit);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_windows_hide_details_until_reopened() {
        let mut details = Details::opened(true);
        details.adapt_to_width(820.0);
        assert!(!details.visible);

        details.visible = true;
        details.adapt_to_width(820.0);
        details.adapt_to_width(760.0);
        assert!(details.visible);

        details.adapt_to_width(1440.0);
        assert!(details.visible);
        details.adapt_to_width(1000.0);
        assert!(!details.visible);

        details.adapt_to_width(1440.0);
        assert!(!details.visible);
    }

    #[test]
    fn large_numbers_are_grouped_in_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(279), "279");
        assert_eq!(group(8_773), "8,773");
        assert_eq!(group(1_234_567), "1,234,567");
    }

    #[test]
    fn a_folder_under_home_is_shown_from_home() {
        let Some(home) = dirs::home_dir() else {
            return;
        };

        assert_eq!(home_relative(&home), "~");
        assert_eq!(home_relative(&home.join("ai_projects")), "~/ai_projects");
        assert_eq!(home_relative(Path::new("/opt/x")), "/opt/x");
    }

    #[test]
    fn empty_parts_are_left_out_and_the_largest_comes_first() {
        let mut context = ContextUse::default();

        context.estimated_tokens.system = 10;
        context.estimated_tokens.user = 5;
        context.estimated_tokens.assistant = 20;

        let parts = parts(&context);

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "Messages");
        assert_eq!(parts[0].tokens, 25);
        assert_eq!(parts[1].name, "System prompt");
    }
}

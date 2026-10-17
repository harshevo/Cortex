//! Background jobs, on the right: the agents working in parallel on the
//! project, what each is doing, and what came of the ones that finished.
//!
//! The jobs live on disk (see [`cortex_core::jobs`]) and are carried out
//! by a runner apart from the window, so this panel only watches them. A
//! thread reads them once a second and wakes the window only when
//! something changed, so an idle panel costs nothing to draw. The same
//! thread starts a runner when jobs are waiting and none is running, as
//! when the app opens on a project whose jobs were cut off.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cortex_core::jobs::{
    self, Activity, ActivityKind, Job, JobStatus, JobStore, NewJob, Readiness, RunnerStart,
};
use eframe::egui::{self, Align2, CornerRadius, RichText, ScrollArea, Sense, Stroke, Ui, vec2};

use super::icons::{self, Icon};
use crate::background::Background;
use crate::theme::{self, palette, radius, space};

/// How often the jobs are read.
const POLL: Duration = Duration::from_secs(1);

/// How long after a failed start a runner is tried again.
const RUNNER_RETRY: Duration = Duration::from_secs(20);

/// Lines of activity kept for a job.
const ACTIVITY_LINES: usize = 10;

pub const DEFAULT_WIDTH: f32 = 340.0;
pub const MIN_WIDTH: f32 = 280.0;
pub const MAX_WIDTH: f32 = 560.0;

/// What the watching thread last read.
#[derive(Default, Clone, PartialEq)]
struct Snapshot {
    jobs: Vec<Job>,
    runner_alive: bool,

    /// Why no runner could be started, when one was needed.
    runner_problem: Option<String>,

    /// What the open jobs, and the one picked, have done lately.
    activity: BTreeMap<String, Vec<Activity>>,
}

/// The thread that reads one project's jobs.
struct Watcher {
    project: PathBuf,
    stop: Arc<AtomicBool>,
    poke: Arc<AtomicBool>,
    snapshot: Arc<Mutex<Snapshot>>,
}

impl Watcher {
    fn start(context: &egui::Context, project: PathBuf, picked: Arc<Mutex<Option<String>>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let poke = Arc::new(AtomicBool::new(false));
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));

        let watcher = Self {
            project: project.clone(),
            stop: stop.clone(),
            poke: poke.clone(),
            snapshot: snapshot.clone(),
        };

        let context = context.clone();

        let started = std::thread::Builder::new()
            .name("cortex-jobs-watcher".to_string())
            .spawn(move || watch(&context, &project, &stop, &poke, &snapshot, &picked));

        if let Err(error) = started {
            eprintln!("warning: the jobs cannot be watched: {error}");
        }

        watcher
    }

    fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Reads the jobs again now rather than on the next second.
    fn refresh(&self) {
        self.poke.store(true, Ordering::Relaxed);
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn watch(
    context: &egui::Context,
    project: &Path,
    stop: &AtomicBool,
    poke: &AtomicBool,
    snapshot: &Mutex<Snapshot>,
    picked: &Mutex<Option<String>>,
) {
    let store = JobStore::for_project(project);
    let mut runner_tried: Option<Instant> = None;
    let mut runner_problem = None;

    while !stop.load(Ordering::Relaxed) {
        let all = store.list();
        let runner_alive = store.runner_alive();

        if !runner_alive && needs_runner(&all) {
            let due = runner_tried.is_none_or(|tried| tried.elapsed() >= RUNNER_RETRY);

            if due {
                runner_tried = Some(Instant::now());

                runner_problem = match store.ensure_runner() {
                    RunnerStart::Failed(error) => Some(error),
                    RunnerStart::Running | RunnerStart::Started => None,
                };
            }
        } else if runner_alive {
            runner_problem = None;
            runner_tried = None;
        }

        let picked = picked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        let activity = all
            .iter()
            .filter(|job| job.status.is_open() || picked.as_deref() == Some(job.id.as_str()))
            .map(|job| {
                (
                    job.id.clone(),
                    jobs::activity(&store.log_path(&job.id), ACTIVITY_LINES),
                )
            })
            .collect();

        let next = Snapshot {
            jobs: all,
            runner_alive,
            runner_problem: runner_problem.clone(),
            activity,
        };

        let changed = {
            let mut current = snapshot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

            let changed = *current != next;

            if changed {
                *current = next;
            }

            changed
        };

        if changed {
            context.request_repaint();
        }

        // A second, in slices, so a poke or a stop is seen soon.
        for _ in 0..10 {
            if stop.load(Ordering::Relaxed) || poke.swap(false, Ordering::Relaxed) {
                break;
            }

            std::thread::sleep(POLL / 10);
        }
    }
}

/// Whether some job waits on a runner: one that could start, one whose
/// runner died under it, or a merge someone asked for.
fn needs_runner(all: &[Job]) -> bool {
    all.iter().any(|job| match job.status {
        JobStatus::Running => true,
        JobStatus::Queued => {
            !job.cancel_requested
                && (job.resolve_requested
                    || JobStore::readiness(job, all) == Readiness::Ready)
        }
        JobStatus::Done => job.merge_requested,
        _ => false,
    })
}

/// What can be done to a job from the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Stop,
    Merge,
    Resolve,
    Retry,
    Remove,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Stop => "Stop",
            Self::Merge => "Merge",
            Self::Resolve => "Resolve",
            Self::Retry => "Retry",
            Self::Remove => "Remove",
        }
    }

    /// What may be done to `job` now.
    fn for_job(job: &Job) -> Vec<Self> {
        match job.status {
            JobStatus::Queued | JobStatus::Running if job.cancel_requested => Vec::new(),
            JobStatus::Queued | JobStatus::Running => vec![Self::Stop],
            JobStatus::Done if job.branch.is_some() && !job.merge_requested => {
                vec![Self::Merge, Self::Remove]
            }
            JobStatus::Done => vec![Self::Remove],
            JobStatus::Conflicted => vec![Self::Resolve, Self::Remove],
            JobStatus::Failed | JobStatus::Cancelled => vec![Self::Retry, Self::Remove],
            JobStatus::Merged => vec![Self::Remove],
        }
    }
}

/// The panel's state, kept between frames.
#[derive(Default)]
pub struct JobsPanel {
    /// The panel is on screen.
    pub visible: bool,

    watcher: Option<Watcher>,

    /// The job opened to show its detail.
    picked: Arc<Mutex<Option<String>>>,

    /// The instruction being written for a new job.
    draft: String,

    /// An action under way, and what came of the last one.
    acting: Background<Result<String, String>>,
    said: Option<(String, bool)>,
}

impl JobsPanel {
    /// Watches the jobs of the project at `project`, from now on.
    pub fn follow(&mut self, context: &egui::Context, project: &Path) {
        if self
            .watcher
            .as_ref()
            .is_some_and(|watcher| watcher.project == project)
        {
            return;
        }

        self.set_picked(None);
        self.said = None;
        self.watcher = Some(Watcher::start(
            context,
            project.to_path_buf(),
            self.picked.clone(),
        ));
    }

    /// Jobs running, and waiting, in the project followed.
    pub fn counts(&self) -> (usize, usize) {
        let Some(watcher) = &self.watcher else {
            return (0, 0);
        };

        let snapshot = watcher.snapshot();

        let running = snapshot
            .jobs
            .iter()
            .filter(|job| job.status == JobStatus::Running)
            .count();

        let queued = snapshot
            .jobs
            .iter()
            .filter(|job| job.status == JobStatus::Queued)
            .count();

        (running, queued)
    }

    fn picked(&self) -> Option<String> {
        self.picked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_picked(&self, id: Option<String>) {
        *self
            .picked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = id;

        if let Some(watcher) = &self.watcher {
            watcher.refresh();
        }
    }

    /// Draws the panel. Returns `true` when it asks to be closed.
    pub fn show(&mut self, ui: &mut Ui) -> bool {
        if let Some(result) = self.acting.poll() {
            self.said = Some(match result {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            });

            if let Some(watcher) = &self.watcher {
                watcher.refresh();
            }
        }

        let Some(watcher) = &self.watcher else {
            return false;
        };

        let project = watcher.project.clone();
        let snapshot = watcher.snapshot();

        let closed = header(ui, &snapshot);

        if let Some(problem) = snapshot
            .runner_problem
            .as_deref()
            .filter(|_| !snapshot.runner_alive)
        {
            ui.add_space(space::XS);
            ui.label(
                RichText::new(format!("No runner: {problem}"))
                    .font(theme::small())
                    .color(palette::warning()),
            );
        }

        if let Some((text, bad)) = &self.said {
            ui.add_space(space::XS);
            ui.label(RichText::new(text).font(theme::small()).color(match bad {
                true => palette::bad(),
                false => palette::text_muted(),
            }));
        }

        ui.add_space(space::S);

        let composer_height = 72.0;
        let list_height = (ui.available_height() - composer_height).max(80.0);

        let mut picked = self.picked();
        let mut acted = None;

        ScrollArea::vertical()
            .id_salt("jobs list")
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if snapshot.jobs.is_empty() {
                    ui.add_space(space::XL);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("No background jobs")
                                .font(theme::ui())
                                .color(palette::text_muted()),
                        );
                    });

                    return;
                }

                let now = jobs::now_ms();

                // Newest first: what is happening now is at the top.
                for job in snapshot.jobs.iter().rev() {
                    let open = picked.as_deref() == Some(job.id.as_str());

                    if row(ui, job, &snapshot.jobs, now).clicked() {
                        picked = match open {
                            true => None,
                            false => Some(job.id.clone()),
                        };
                    }

                    if open {
                        let activity = snapshot.activity.get(&job.id);

                        if let Some(action) = detail(ui, job, activity, self.acting.pending()) {
                            acted = Some((job.id.clone(), action));
                        }
                    }

                    ui.add_space(space::XXS);
                }
            });

        if picked != self.picked() {
            self.set_picked(picked);
        }

        if let Some((id, action)) = acted {
            self.act(ui.ctx(), &project, id, action);
        }

        self.composer(ui, &project);

        // Elapsed times tick while something runs.
        if snapshot.jobs.iter().any(|job| job.status == JobStatus::Running) {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }

        closed
    }

    /// The box at the foot of the panel that queues a job.
    fn composer(&mut self, ui: &mut Ui, project: &Path) {
        ui.add_space(space::S);
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.cursor().top(),
            Stroke::new(1.0, palette::border()),
        );
        ui.add_space(space::S);

        let edit = egui::TextEdit::multiline(&mut self.draft)
            .hint_text("New job…")
            .font(theme::ui())
            .desired_rows(2)
            .desired_width(ui.available_width())
            .frame(egui::Frame::NONE);

        let response = ui.add(edit);

        let send = response.has_focus()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));

        if send && !self.draft.trim().is_empty() && !self.acting.pending() {
            let instruction = std::mem::take(&mut self.draft).trim().to_string();
            let project = project.to_path_buf();

            self.acting.start(ui.ctx(), move |_| {
                let store = JobStore::for_project(&project);

                let job = store.add(NewJob {
                    instruction,
                    created_by: "person".to_string(),
                    auto_merge: true,
                    folder: jobs::folder_in_repository(&project),
                    ..NewJob::default()
                })?;

                let started = match store.ensure_runner() {
                    RunnerStart::Failed(error) => format!(" No runner yet: {error}"),
                    _ => String::new(),
                };

                Ok(format!("Queued {}.{started}", job.id))
            });
        }
    }

    fn act(&mut self, context: &egui::Context, project: &Path, id: String, action: Action) {
        let project = project.to_path_buf();

        self.acting.start(context, move |_| {
            let store = JobStore::for_project(&project);

            let job = match action {
                Action::Stop => store.request_cancel(&id)?,
                Action::Merge => store.request_merge(&id)?,
                Action::Resolve => store.request_resolve(&id)?,
                Action::Retry => store.retry(&id)?,

                Action::Remove => {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| error.to_string())?;

                    return runtime.block_on(cortex_cli::commands::jobs::remove(&store, &id, false));
                }
            };

            if matches!(action, Action::Merge | Action::Resolve | Action::Retry)
                && let RunnerStart::Failed(error) = store.ensure_runner()
            {
                return Err(format!("{}: no runner could be started: {error}", job.id));
            }

            Ok(match action {
                Action::Stop => format!("Stopping {}.", job.id),
                Action::Merge => format!("Merging {}.", job.id),
                Action::Resolve => format!("Resolving {}.", job.id),
                _ => format!("Queued {} again.", job.id),
            })
        });
    }
}

/// The title row. Returns whether the close button was pressed.
fn header(ui: &mut Ui, snapshot: &Snapshot) -> bool {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), theme::control::MEDIUM), Sense::hover());

    let painter = ui.painter();
    let middle = rect.center().y;

    let title = painter.text(
        egui::pos2(rect.left(), middle),
        Align2::LEFT_CENTER,
        "Jobs",
        theme::strong(theme::size::UI),
        palette::text(),
    );

    let running = snapshot
        .jobs
        .iter()
        .filter(|job| job.status == JobStatus::Running)
        .count();
    let queued = snapshot
        .jobs
        .iter()
        .filter(|job| job.status == JobStatus::Queued)
        .count();

    let counts = match (running, queued) {
        (0, 0) => String::new(),
        (running, 0) => format!("{running} running"),
        (0, queued) => format!("{queued} queued"),
        (running, queued) => format!("{running} running · {queued} queued"),
    };

    painter.text(
        egui::pos2(title.right() + space::S, middle),
        Align2::LEFT_CENTER,
        counts,
        theme::ui(),
        palette::text_muted(),
    );

    let close = icons::slot_from_right(rect, 0);

    icons::button_at(ui, close, ui.id().with("close jobs"), Icon::Close, "Close  ⌘J", false)
        .clicked()
}

/// One job's row. Clicking it opens or folds its detail.
fn row(ui: &mut Ui, job: &Job, all: &[Job], now: u64) -> egui::Response {
    let height = 44.0;

    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());

    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::M), palette::raised());
    }

    let painter = ui.painter();
    let left = rect.left() + space::S;
    let top = rect.top() + 13.0;
    let bottom = rect.top() + 31.0;

    mark(ui, egui::pos2(left + 4.0, top), job.status);

    let text_left = left + space::L + 2.0;

    let elapsed = job
        .elapsed(now)
        .map(|elapsed| short_duration(elapsed.as_secs()))
        .unwrap_or_default();

    let meta = painter.text(
        egui::pos2(rect.right() - space::S, top),
        Align2::RIGHT_CENTER,
        elapsed,
        theme::small(),
        palette::text_muted(),
    );

    let room = (meta.left() - text_left - space::S).max(20.0);
    let title = crate::format::one_line(&job.title, (room / 6.6) as usize);

    painter.text(
        egui::pos2(text_left, top),
        Align2::LEFT_CENTER,
        title,
        theme::ui(),
        match job.status {
            JobStatus::Merged | JobStatus::Cancelled => palette::text_secondary(),
            _ => palette::text(),
        },
    );

    let (line, color) = subline(job, all);
    let room = rect.right() - space::S - text_left;

    painter.text(
        egui::pos2(text_left, bottom),
        Align2::LEFT_CENTER,
        crate::format::one_line(&line, (room / 6.0) as usize),
        theme::small(),
        color,
    );

    response.on_hover_text(&job.instruction)
}

/// The second line of a job's row: its state and what stands out.
fn subline(job: &Job, all: &[Job]) -> (String, egui::Color32) {
    let mut parts = vec![job.id.clone()];

    let state = match job.status {
        JobStatus::Queued if job.resolve_requested => "resolving next".to_string(),
        JobStatus::Queued => match JobStore::readiness(job, all) {
            Readiness::Ready => "queued".to_string(),
            Readiness::Waiting(on) => format!("after {}", on.join(", ")),
            Readiness::Blocked(by) => format!("blocked by {by}"),
        },
        JobStatus::Running if job.cancel_requested => "stopping".to_string(),
        JobStatus::Done if job.merge_requested => "merging".to_string(),
        JobStatus::Done if job.branch.is_some() => "ready to merge".to_string(),
        status => status.label().to_string(),
    };

    parts.push(state);

    if job.steps > 0 {
        parts.push(format!("{} steps", job.steps));
    }

    if !job.changed_files.is_empty() {
        let files = job.changed_files.len();

        parts.push(match files {
            1 => "1 file".to_string(),
            files => format!("{files} files"),
        });
    }

    let color = match job.status {
        JobStatus::Failed | JobStatus::Conflicted => palette::bad(),
        _ if !job.overlaps.is_empty() && job.status.is_open() => palette::warning(),
        _ => palette::text_muted(),
    };

    if !job.overlaps.is_empty() && !job.status.is_settled() {
        let with: Vec<&str> = job.overlaps.iter().map(|overlap| overlap.job.as_str()).collect();

        parts.push(format!("overlaps {}", with.join(", ")));
    }

    (parts.join(" · "), color)
}

/// The circle beside a job's title, by its status.
fn mark(ui: &Ui, centre: egui::Pos2, status: JobStatus) {
    let painter = ui.painter();

    match status {
        JobStatus::Queued => {
            painter.circle_stroke(centre, 4.5, Stroke::new(1.25, palette::text_muted()));
        }

        JobStatus::Running => {
            painter.circle_stroke(centre, 4.5, Stroke::new(1.25, palette::accent()));
            painter.circle_filled(centre, 2.25, palette::accent());
        }

        JobStatus::Done | JobStatus::Merged => {
            let fill = match status {
                JobStatus::Done => palette::text_secondary(),
                _ => palette::text_muted(),
            };

            painter.circle_filled(centre, 5.0, fill);
            painter.line(
                vec![
                    centre + vec2(-2.3, 0.0),
                    centre + vec2(-0.6, 1.7),
                    centre + vec2(2.4, -1.7),
                ],
                Stroke::new(1.3, palette::sidebar()),
            );
        }

        JobStatus::Conflicted | JobStatus::Failed => {
            painter.circle_stroke(centre, 4.5, Stroke::new(1.25, palette::bad()));
            painter.line_segment(
                [centre + vec2(0.0, -2.2), centre + vec2(0.0, 0.8)],
                Stroke::new(1.3, palette::bad()),
            );
            painter.circle_filled(centre + vec2(0.0, 2.3), 0.7, palette::bad());
        }

        JobStatus::Cancelled => {
            painter.circle_stroke(centre, 4.5, Stroke::new(1.25, palette::text_muted()));
            painter.line_segment(
                [centre + vec2(-2.0, 0.0), centre + vec2(2.0, 0.0)],
                Stroke::new(1.25, palette::text_muted()),
            );
        }
    }
}

/// A job opened: what it is doing or did, and what can be done to it.
/// Returns the action picked.
fn detail(
    ui: &mut Ui,
    job: &Job,
    activity: Option<&Vec<Activity>>,
    busy: bool,
) -> Option<Action> {
    let mut picked = None;
    let indent = space::S + space::L + 2.0;

    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: indent as i8,
            right: space::S as i8,
            top: 0,
            bottom: space::S as i8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = space::XS;

            let muted = |text: String| RichText::new(text).font(theme::small()).color(palette::text_muted());

            if let Some(branch) = &job.branch {
                ui.label(muted(branch.clone()));
            }

            if let Some(error) = &job.error {
                ui.label(RichText::new(error).font(theme::small()).color(match job.status {
                    JobStatus::Failed | JobStatus::Conflicted => palette::bad(),
                    _ => palette::warning(),
                }));
            }

            for overlap in job.overlaps.iter().filter(|_| !job.status.is_settled()) {
                ui.label(
                    RichText::new(format!("{} also changes {}", overlap.job, overlap.files.join(", ")))
                        .font(theme::small())
                        .color(palette::warning()),
                );
            }

            if !job.conflicts.is_empty() {
                ui.label(
                    RichText::new(format!("Conflicts: {}", job.conflicts.join(", ")))
                        .font(theme::small())
                        .color(palette::bad()),
                );
            }

            if let Some(activity) = activity.filter(|activity| !activity.is_empty()) {
                ui.add_space(space::XXS);

                for item in activity {
                    let color = match item.kind {
                        ActivityKind::Said => palette::text_secondary(),
                        ActivityKind::Tool | ActivityKind::Milestone => palette::text_muted(),
                        ActivityKind::Failed => palette::bad(),
                    };

                    let font = match item.kind {
                        ActivityKind::Tool => theme::mono(),
                        _ => theme::small(),
                    };

                    ui.add(
                        egui::Label::new(RichText::new(&item.text).font(font).color(color))
                            .truncate(),
                    );
                }
            }

            if !job.summary.trim().is_empty() && !job.status.is_open() {
                ui.add_space(space::XXS);

                let summary = crate::format::one_line(job.summary.trim(), 600);

                ui.label(
                    RichText::new(summary)
                        .font(theme::small())
                        .color(palette::text_secondary()),
                );
            }

            if !job.changed_files.is_empty() && !job.status.is_open() {
                ui.label(muted(job.changed_files.join(", ")));
            }

            let actions = Action::for_job(job);

            if !actions.is_empty() {
                ui.add_space(space::XS);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::S;

                    for action in actions {
                        let button = match action {
                            Action::Merge | Action::Resolve => theme::primary_button(action.label()),

                            _ => egui::Button::new(
                                RichText::new(action.label())
                                    .font(theme::ui())
                                    .color(palette::text_secondary()),
                            )
                            .fill(palette::surface())
                            .stroke(Stroke::new(1.0, palette::border()))
                            .corner_radius(CornerRadius::same(radius::M))
                            .min_size(vec2(0.0, theme::control::MEDIUM)),
                        };

                        let hint = match action {
                            Action::Stop => "Stop the job; Retry carries on from here",
                            Action::Merge => "Merge its branch into the project, after a conflict check",
                            Action::Resolve => "Have an agent resolve the conflicts in the job's worktree, then merge",
                            Action::Retry => "Queue it again, carrying on where it stopped",
                            Action::Remove => "Forget the job and remove its worktree; an unmerged branch is kept",
                        };

                        if ui.add_enabled(!busy, button).on_hover_text(hint).clicked() {
                            picked = Some(action);
                        }
                    }
                });
            }
        });

    picked
}

/// `12s`, `4m`, `1h 5m`.
fn short_duration(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, status: JobStatus) -> Job {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": id, "instruction": "x",
            "status": status, "created_ms": 0,
        }))
        .unwrap()
    }

    #[test]
    fn a_runner_is_needed_only_for_work_that_can_go_on() {
        let mut waiting = job("job-2", JobStatus::Queued);
        waiting.depends_on = vec!["job-1".into()];

        assert!(!needs_runner(&[job("job-1", JobStatus::Failed), waiting.clone()]));
        assert!(needs_runner(&[job("job-1", JobStatus::Queued)]));
        assert!(needs_runner(&[job("job-1", JobStatus::Running)]));

        let mut merging = job("job-1", JobStatus::Done);
        assert!(!needs_runner(&[merging.clone()]));

        merging.merge_requested = true;
        assert!(needs_runner(&[merging]));
    }

    #[test]
    fn actions_follow_the_status() {
        let mut done = job("job-1", JobStatus::Done);

        assert_eq!(Action::for_job(&done), vec![Action::Remove]);

        done.branch = Some("cortex/job-1".into());

        assert_eq!(Action::for_job(&done), vec![Action::Merge, Action::Remove]);
        assert_eq!(Action::for_job(&job("j", JobStatus::Running)), vec![Action::Stop]);
        assert_eq!(
            Action::for_job(&job("j", JobStatus::Conflicted)),
            vec![Action::Resolve, Action::Remove]
        );
    }

    #[test]
    fn the_panel_reads_a_projects_jobs() {
        let folder = tempfile::tempdir().unwrap();
        let store = JobStore::for_project(folder.path());

        store
            .add(NewJob {
                instruction: "Write the parser".into(),
                ..NewJob::default()
            })
            .unwrap();

        let context = egui::Context::default();
        let mut panel = JobsPanel::default();

        panel.follow(&context, folder.path());

        let deadline = Instant::now() + Duration::from_secs(5);

        while panel.counts() == (0, 0) {
            assert!(Instant::now() < deadline, "the jobs were not read");

            std::thread::sleep(Duration::from_millis(20));
        }

        // Queued, so its runner was asked for; none can start here.
        assert_eq!(panel.counts(), (0, 1));
    }

    #[test]
    fn durations_are_short() {
        assert_eq!(short_duration(5), "5s");
        assert_eq!(short_duration(125), "2m");
        assert_eq!(short_duration(3_900), "1h 5m");
    }
}

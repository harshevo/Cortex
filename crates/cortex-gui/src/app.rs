//! The window: holds the state, feeds it what the agent reports, and lays
//! out the views.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use cortex_core::Image;
use eframe::egui::{self, Margin, RichText, Stroke};

use crate::attachment::Attachments;
use crate::background::Background;
use crate::engine::{Engine, Info, Launch, PendingApproval, Update};
use crate::rehearsal::Rehearsal;
use crate::session::history::{self, PastSession};
use crate::session::{Session, Tone};
use crate::settings::Settings;
use crate::theme::{self, palette};
use crate::views::commands::{self, Builtin};
use crate::views::composer::{self, Mode};
use crate::views::conversation::{self, ChatView};
use crate::views::details::{self as details_view, Details};
use crate::views::files::Files;
use crate::views::inspector::{self, Inspector};
use crate::views::settings::Form;
use crate::views::sidebar::{self, Shelf};
use crate::views::stats_strip::{self, Change, Reading};
use crate::views::{Section, View, settings as settings_page, tasks, top_bar, typeset};

/// Low enough that its buttons sit on a line with the window's own.
const TOP_BAR_HEIGHT: f32 = 32.0;
const SIDEBAR_WIDTH: f32 = 272.0;
const SIDEBAR_MIN_WIDTH: f32 = 200.0;
const SIDEBAR_MAX_WIDTH: f32 = 440.0;

const DETAILS_ID: &str = "details";
const DETAILS_WIDTH: f32 = 300.0;
const DETAILS_MIN_WIDTH: f32 = 240.0;
const DETAILS_MAX_WIDTH: f32 = 460.0;

const FILES_WIDTH: f32 = 264.0;
const INSPECTOR_WIDTH: f32 = 280.0;
const FILES_WIDTH_WITH_FILE: f32 = 640.0;
const FILES_MIN_WIDTH: f32 = 180.0;
const FILES_MAX_WIDTH: f32 = 1100.0;

/// Whether there is an agent to talk to.
enum Status {
    /// The agent is being built.
    Starting,

    Ready(Info),

    /// The agent could not be built, and why.
    Unavailable(String),
}

pub struct CortexApp {
    engine: Engine,
    settings: Settings,

    /// The agent is the scripted stand-in. Nothing is saved in this mode.
    demo: bool,

    /// The folder that sessions outside any project work in.
    scratch: PathBuf,

    /// The folder the agent works in: a project, or `scratch`.
    place: PathBuf,

    status: Status,
    view: View,

    /// Which list the sidebar shows: chats or coding projects.
    section: Section,

    /// False while the sidebar is hidden.
    sidebar_open: bool,

    /// The session the agent is working in.
    live: Session,

    /// The project is being got ready; the button waits until it is.
    preparing: bool,

    /// A past session opened from the sidebar, shown instead of `live`.
    viewing: Option<(PathBuf, Session)>,

    /// How the session on screen is laid out and scrolled, and which
    /// past session that is, if it is one.
    chat: ChatView,
    chat_shows: Option<PathBuf>,

    /// The past sessions of every folder in the sidebar, newest first.
    sessions: HashMap<PathBuf, Vec<PastSession>>,
    history_revision: u64,
    history_job: Background<HashMap<PathBuf, Vec<PastSession>>>,
    history_dirty: bool,
    session_job: Background<(PathBuf, Result<Session, String>)>,
    loading_session: Option<PathBuf>,

    /// The session or project whose delete button has been pressed once.
    confirming: Option<PathBuf>,

    /// Questions from tools, oldest first. The first is on screen.
    approvals: VecDeque<PendingApproval>,

    /// The files of the project, shown on the right.
    files: Files,

    /// Where the agent works, how full the context is and what changed,
    /// on the far right.
    details: Details,

    /// The instruction being typed.
    draft: String,

    insights: crate::views::insights::InsightsView,

    /// Pictures waiting to go with the next instruction.
    attachments: Attachments,

    /// What has been sent to the run in progress and not yet read by the
    /// model.
    queued: Vec<String>,

    /// The log of the live session, when it is one that was taken up
    /// again. A session started here has no log until its first run.
    live_log: Option<PathBuf>,

    /// The settings page, while it is open.
    form: Option<Form>,

    /// The slash commands, and the folder whose skills are among them.
    commands: Vec<commands::Entry>,
    commands_for: Option<PathBuf>,

    /// The task list lists every step, not only the current one.
    tasks_open: bool,

    /// The task list was put away with `/tasks`.
    tasks_hidden: bool,

    /// The run and what was learned, on the right.
    inspector: Inspector,

    /// List prices, for what a run has cost.
    prices: cortex_core::PriceTable,

    rehearsal: Rehearsal,
    requested_session: Option<(PathBuf, Option<PathBuf>)>,
    requested_delete: Option<PathBuf>,
    pending_settings: Option<Settings>,
}

impl CortexApp {
    /// `open` is a project folder to start in; otherwise the app starts in
    /// the first project, or outside any project if there is none.
    pub fn new(
        engine: Engine,
        settings: Settings,
        open: Option<PathBuf>,
        demo: bool,
        rehearsal: Rehearsal,
    ) -> Self {
        let scratch = crate::settings::scratch_folder().unwrap_or_else(|error| {
            eprintln!("warning: {error:#}");

            std::env::temp_dir()
        });

        let place = open
            .or_else(|| settings.projects.iter().find(|path| path.is_dir()).cloned())
            .unwrap_or_else(|| scratch.clone());

        let section = if place == scratch {
            Section::Chats
        } else {
            Section::Code
        };

        let mut app = Self {
            engine,
            settings,
            demo,
            scratch,
            place,
            status: Status::Starting,
            view: if rehearsal.opens_insights() {
                View::Insights
            } else {
                View::Conversation
            },
            section,
            sidebar_open: true,
            live: Session::default(),
            preparing: false,
            viewing: None,
            chat: ChatView::default(),
            chat_shows: None,
            sessions: HashMap::new(),
            history_revision: 0,
            history_job: Background::default(),
            history_dirty: false,
            session_job: Background::default(),
            loading_session: None,
            confirming: None,
            approvals: VecDeque::new(),
            files: Files::opened(rehearsal.opens_files(), rehearsal.opens_file()),
            details: Details::opened(rehearsal.opens_details()),
            draft: String::new(),
            insights: Default::default(),
            attachments: Attachments::default(),
            queued: Vec::new(),
            live_log: None,
            form: None,
            commands: Vec::new(),
            commands_for: None,
            tasks_open: true,
            tasks_hidden: false,
            inspector: Inspector::default(),
            prices: cortex_core::PriceTable::at_home(),
            rehearsal,
            requested_session: None,
            requested_delete: None,
            pending_settings: None,
        };

        app.refresh_sessions();
        app.launch(true);

        if app.rehearsal.opens_settings() {
            app.open_settings();

            if app.rehearsal.tries_theme()
                && let Some(form) = &mut app.form
            {
                form.show_appearance();
            }
        }

        app
    }

    /// Builds the agent for the current folder and settings. `fresh`
    /// forgets the conversation; without it the conversation carries on
    /// with the new model or settings.
    fn launch(&mut self, fresh: bool) {
        self.launch_into(fresh, None);
    }

    /// As [`Self::launch`], taking up the session logged in `open` once
    /// the agent is built.
    fn launch_into(&mut self, fresh: bool, open: Option<PathBuf>) {
        if fresh {
            self.live = Session::default();
            self.chat.reset();
            self.viewing = None;
            self.approvals.clear();

            self.queued.clear();
            self.live_log = None;
        }

        self.status = Status::Starting;

        self.engine.launch(Launch {
            place: self.place.clone(),
            settings: self.settings.clone(),

            // The keychain is asked by the engine, off the window's
            // thread: it can take a while, and may ask the person.
            api_key: None,
            look_up_key: !self.demo,
            demo: self.demo,
            fresh,
            open,
        });
    }

    fn save_settings(&self) {
        if self.demo {
            return;
        }

        if let Err(error) = self.settings.save() {
            eprintln!("warning: the settings were not saved: {error:#}");
        }
    }

    fn refresh_sessions(&mut self) {
        self.history_revision += 1;
        self.history_dirty = true;
    }

    fn poll_background(&mut self) {
        if let Some(sessions) = self.history_job.poll() {
            self.sessions = sessions;
        }
        if self.history_dirty && !self.history_job.pending() {
            let folders: Vec<_> = self
                .settings
                .projects
                .iter()
                .chain([&self.scratch])
                .cloned()
                .collect();
            if self
                .history_job
                .start(self.engine.context(), move |cancelled| {
                    folders
                        .into_iter()
                        .take_while(|_| !cancelled.load(std::sync::atomic::Ordering::Relaxed))
                        .map(|folder| {
                            let sessions = history::list(&folder);
                            (folder, sessions)
                        })
                        .collect()
                })
            {
                self.history_dirty = false;
            }
        }
        if let Some((path, result)) = self.session_job.poll() {
            self.loading_session = None;
            match result {
                Ok(session) => self.install_session(path, session),
                Err(error) => self
                    .live
                    .notice(&format!("Cannot open chat: {error}"), Tone::Bad),
            }
        }
        if let Some(path) = &self.loading_session
            && !self.session_job.pending()
        {
            let path = path.clone();
            self.session_job
                .start(self.engine.context(), move |cancelled| {
                    let session = history::load_cancelled(&path, cancelled)
                        .map_err(|error| error.to_string());
                    (path, session)
                });
        }
    }

    fn cancel_session_load(&mut self) {
        self.session_job.cancel();
        self.loading_session = None;
    }

    /// Takes in everything the agent has reported since the last frame.
    fn absorb_updates(&mut self) {
        let updates: Vec<Update> = self.engine.updates().take(64).collect();
        if updates.len() == 64 {
            self.engine.context().request_repaint();
        }

        for update in updates {
            match update {
                Update::ServiceStarting(text) => self.live.notice(&text, Tone::Muted),
                Update::Ready(info) => self.agent_ready(info),

                Update::Unavailable(problem) => self.status = Status::Unavailable(problem),

                // The session is listed from its first event and marked as
                // the one on screen. Otherwise a click on it mid-run opens
                // it as if it were another session, which is read-only.
                Update::Logging(path) => {
                    self.live_log = Some(path);

                    self.refresh_sessions();
                }

                Update::Event(event) => {
                    self.heard(&event);
                    self.live.apply(&event);

                    crate::session::slim_after(&mut self.live, &event);
                }

                Update::ApprovalNeeded(approval) => self.approvals.push_back(approval),

                Update::WritingStarted => self.live.writing_started(),

                Update::Writing(piece) => self.live.write(&piece),

                Update::Thinking(piece) => self.live.think(&piece),

                Update::Calling(tool) => self.live.calling(&tool),

                Update::Retrying { reason, wait } => self.live.retrying(&reason, wait),

                Update::RunFailed {
                    error,
                    can_continue,
                } => self.live.fail(&error, can_continue),

                Update::RunStopped => {
                    self.live.stop();

                    // Questions from the abandoned run have nobody left to
                    // hear the answer.
                    self.approvals.clear();
                }

                Update::Idle => {
                    self.live.running = false;

                    // What the run never got to hear goes back in the box.
                    self.return_queued();

                    self.refresh_sessions();
                }

                // One that arrives after the next run began is too late.
                Update::Compacted((text, failed)) | Update::Said((text, failed)) => {
                    let tone = if failed { Tone::Bad } else { Tone::Muted };

                    self.live.notice(&text, tone);
                }

                Update::WorktreeMade((folder, branch)) => {
                    self.live.notice(
                        &format!("Worktree on {branch} · {}", folder.display()),
                        Tone::Muted,
                    );

                    self.settings.add_project(&folder);
                    self.save_settings();
                    self.refresh_sessions();

                    if !self.live.running {
                        self.requested_session = Some((folder, None));
                    }
                }

                Update::Prepared { text, failed } => {
                    self.preparing = false;

                    let tone = if failed { Tone::Bad } else { Tone::Muted };

                    self.live.notice(&text, tone);
                }
            }
        }
    }

    fn agent_ready(&mut self, info: Info) {
        // The demo works in a throwaway project of its own, which is
        // listed like any other so that its sessions can be seen.
        if info.demo && self.place != info.place {
            self.place = info.place.clone();
            self.section = Section::Code;
            self.settings.add_project(&info.place);

            self.refresh_sessions();
        }

        self.status = Status::Ready(info);
    }

    fn submit(&mut self, instruction: &str, images: Vec<Image>) {
        self.viewing = None;
        self.live.running = true;

        self.engine.submit(instruction, images);
    }

    /// Tells the run in progress something. It waits, shown as queued,
    /// until the model has read it.
    fn steer(&mut self, message: &str) {
        self.queued.push(message.to_string());

        self.engine.steer(message);
    }

    /// Takes off the queue what `event` shows the model has now read.
    fn heard(&mut self, event: &cortex_core::Event) {
        match event {
            cortex_core::Event::UserInterjected { text, .. } => {
                if let Some(position) = self.queued.iter().position(|queued| queued == text) {
                    self.queued.remove(position);
                }
            }

            // What arrived too late for one run opens the next.
            cortex_core::Event::RunStarted { instruction, .. } => {
                self.refresh_sessions();
                self.queued
                    .retain(|queued| !instruction.contains(queued.as_str()));
            }

            _ => {}
        }
    }

    /// Puts unread messages back in the composer, ahead of anything
    /// typed since.
    fn return_queued(&mut self) {
        if self.queued.is_empty() {
            return;
        }

        let mut parts = std::mem::take(&mut self.queued);

        if !self.draft.trim().is_empty() {
            parts.push(std::mem::take(&mut self.draft));
        }

        self.draft = parts.join("\n");
    }

    /// Carries on the run that stopped on a provider failure.
    fn carry_on(&mut self) {
        self.live.running = true;
        self.live.can_continue = false;

        self.engine.carry_on();
    }

    /// Starts a new session in `folder`, moving the agent there if it is
    /// somewhere else.
    fn start_session(&mut self, folder: PathBuf) {
        self.cancel_session_load();
        self.view = View::Conversation;

        let elsewhere = folder != self.place && !self.demo;

        if elsewhere || !matches!(self.status, Status::Ready(_)) {
            self.place = folder;

            self.launch(true);

            return;
        }

        self.viewing = None;
        self.live = Session::default();
        self.chat.reset();
        self.live_log = None;

        self.engine.reset();
    }

    fn add_project(&mut self) {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Add a project folder")
            .pick_folder()
        else {
            return;
        };

        self.settings.add_project(&folder);
        self.save_settings();
        self.refresh_sessions();

        if !self.live.running {
            self.requested_session = Some((folder, None));
        }
    }

    /// Takes a project off the list. Its files are left alone.
    fn remove_project(&mut self, project: &Path) {
        self.settings.projects.retain(|listed| listed != project);

        self.save_settings();
        self.refresh_sessions();

        if self.place == project && !self.live.running {
            self.start_session(self.scratch.clone());
        }
    }

    /// Opens a session from the sidebar and takes it up again: it
    /// becomes the live session, with the agent in its folder and its
    /// conversation as the model last saw it. While a run is in progress
    /// elsewhere the session is only shown, and can be taken up after.
    fn open_past_session(&mut self, path: PathBuf) {
        self.cancel_session_load();
        self.view = View::Conversation;

        if self.live_log.as_deref() == Some(path.as_path()) {
            self.viewing = None;

            return;
        }

        self.loading_session = Some(path);
    }

    fn install_session(&mut self, path: PathBuf, session: Session) {
        if self.live.running {
            self.viewing = Some((path, session));

            return;
        }

        // A log is at `<folder>/.cortex/trajectories/<name>.jsonl`.
        let folder = path.ancestors().nth(3).map(Path::to_path_buf);

        let elsewhere = folder.as_ref().is_some_and(|folder| *folder != self.place) && !self.demo;

        if elsewhere || !matches!(self.status, Status::Ready(_)) {
            if let Some(folder) = folder {
                self.place = folder;
            }

            self.launch_into(true, Some(path.clone()));
        } else {
            self.viewing = None;
            self.approvals.clear();

            self.engine.open(path.clone());
        }

        self.live = session;
        self.live_log = Some(path);
        self.chat.reset();
    }

    pub(crate) fn delete_session(&mut self, path: &Path) {
        if self.loading_session.as_deref() == Some(path) {
            self.cancel_session_load();
        }
        if let Err(error) = history::delete(path) {
            eprintln!("warning: cannot delete {}: {error}", path.display());
        }

        if self.viewing.as_ref().is_some_and(|(open, _)| open == path) {
            self.viewing = None;
        }

        // Deleting the session being worked in leaves nothing to carry
        // on, so a new one is started in its place.
        if self.live_log.as_deref() == Some(path) && !self.live.running {
            self.start_session(self.place.clone());
        }

        self.refresh_sessions();
    }

    fn open_settings(&mut self) {
        self.form = Some(Form::new(&self.settings));
        self.view = View::Settings;
    }

    /// Takes over settings from the settings page and rebuilds the agent
    /// with them.
    fn apply_settings(&mut self, settings: Settings) {
        if self.live.running || self.preparing {
            self.pending_settings = Some(settings);
            return;
        }
        // A different provider cannot carry on another's conversation.
        let fresh = settings.provider != self.settings.provider;

        self.settings = settings;

        self.save_settings();
        self.launch(fresh);

        self.view = View::Conversation;
    }

    /// A choice made in the row under the composer.
    fn apply_change(&mut self, change: Change) {
        match change {
            Change::Model(provider, model) => {
                // A different provider cannot carry on another's conversation.
                let fresh = provider != self.settings.provider;

                self.settings.provider = provider;
                self.settings.current_mut().model = model;

                self.save_settings();
                self.launch(fresh);

                return;
            }
            Change::Effort(effort) => self.settings.effort = effort,
            Change::ApproveEverything(approve) => self.settings.approve_everything = approve,

            // The conversation, not the settings, changes.
            Change::Compact => {
                if self.viewing.is_none() && matches!(self.status, Status::Ready(_)) {
                    self.live
                        .notice("Writing a summary of the conversation so far…", Tone::Muted);
                    self.engine.compact();
                } else {
                    self.live
                        .notice("There is no conversation in use to compact.", Tone::Muted);
                }

                return;
            }
        }

        self.save_settings();
        self.launch(false);
    }

    /// Puts the last thing the person said back in the composer, to send
    /// again or change. Asked for with the Up key in an empty composer.
    fn recall_last(&mut self) {
        let last = self.live.items.iter().rev().find_map(|item| match item {
            crate::session::Item::User { text, .. } => Some(text.as_str()),
            _ => None,
        });

        if let Some(last) = last
            && self.draft.is_empty()
        {
            self.draft = last.to_string();
        }
    }

    /// Takes what was pasted. Text is handed back as the event that puts
    /// it in the field with the keyboard; pictures and files are attached
    /// to the next instruction.
    pub fn paste(&mut self, pasted: crate::clipboard::Pasted) -> Option<egui::Event> {
        use crate::clipboard::Pasted;

        if let Pasted::Text(text) = pasted {
            return Some(egui::Event::Paste(text));
        }

        if self.composer_mode() != Mode::Ready {
            self.live.notice(
                "Files can be attached once Cortex is ready for an instruction.",
                Tone::Muted,
            );
            return None;
        }

        let refused = match pasted {
            Pasted::Files(paths) => self.attachments.enqueue(&paths),
            Pasted::Picture {
                width,
                height,
                rgba,
            } => self.attachments.enqueue_pixels(width, height, rgba),
            Pasted::Text(_) => None,
        };

        if let Some(why) = refused {
            self.live.notice(&why, Tone::Bad);
        }

        None
    }

    fn composer_mode(&self) -> Mode {
        if self.loading_session.is_some() {
            return if self.live.running {
                Mode::Working
            } else {
                Mode::Starting
            };
        }
        if self.viewing.is_some() {
            return Mode::Viewing;
        }

        match &self.status {
            Status::Starting => Mode::Starting,
            Status::Unavailable(_) => Mode::Unavailable,
            Status::Ready(_) if self.live.running => Mode::Working,
            Status::Ready(_) => Mode::Ready,
        }
    }

    fn max_steps(&self) -> u32 {
        match &self.status {
            Status::Ready(info) => info.max_steps,
            _ => self.settings.max_steps,
        }
    }

    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        self.top_bar_with_menu(ui, top_bar::Steps::default(), |_| {});
    }

    pub(crate) fn top_bar_with_menu(
        &mut self,
        ui: &mut egui::Ui,
        steps: top_bar::Steps,
        workspace_menu: impl FnOnce(&mut egui::Ui),
    ) -> Option<top_bar::Step> {
        // As wide as the sidebar was dragged to, so that the switch in
        // the bar stays at its edge.
        let sidebar_width =
            egui::containers::panel::PanelState::load(ui.ctx(), ui.id().with("sidebar"))
                .map_or(SIDEBAR_WIDTH + 16.0, |state| state.size().x);

        let files_width = self.files.visible.then(|| {
            let (id, width) = self.files_panel_id();

            egui::containers::panel::PanelState::load(ui.ctx(), ui.id().with(id))
                .map_or(width + 16.0, |state| state.size().x)
        });

        let panel_width = |id: &str, width: f32| {
            egui::containers::panel::PanelState::load(ui.ctx(), ui.id().with(id))
                .map_or(width, |state| state.size().x)
        };

        // Every panel on the right is under the bar's right-hand end.
        let right_width = [
            files_width,
            self.details
                .visible
                .then(|| panel_width(DETAILS_ID, DETAILS_WIDTH + 24.0)),
            self.inspector
                .visible
                .then(|| panel_width("inspector", INSPECTOR_WIDTH + 32.0)),
        ]
        .into_iter()
        .flatten()
        .reduce(|total, width| total + width);
        // The bar is not a strip of its own: it is the top of whatever is
        // under it, so it takes the chat's colour.
        let frame = egui::Frame::new()
            .fill(palette::background())
            .inner_margin(Margin::symmetric(8, 0));

        let project = (self.place != self.scratch)
            .then(|| self.place.file_name())
            .flatten()
            .map(|name| name.to_string_lossy().into_owned());

        let place = top_bar::Place {
            project: project.as_deref(),
            demo: self.demo,
            busy: self.live.running || self.preparing || matches!(self.status, Status::Starting),
        };

        let mut navigation = self.sidebar_visible();
        let mut right = self.files.visible;
        let mut details_open = self.details.visible;
        let result = egui::Panel::top(ui.id().with("top bar"))
            .exact_size(TOP_BAR_HEIGHT)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                // Over the sidebar, and over the files on the right, it
                // takes theirs.
                let bar = ui.max_rect().expand2(egui::vec2(8.0, 0.0));

                if self.sidebar_visible() {
                    ui.painter().rect_filled(
                        bar.with_max_x(bar.left() + sidebar_width),
                        0.0,
                        palette::sidebar(),
                    );
                }

                if let Some(width) = right_width {
                    ui.painter().rect_filled(
                        bar.with_min_x(bar.right() - width),
                        0.0,
                        palette::sidebar(),
                    );
                }

                top_bar::show(
                    ui,
                    &place,
                    &mut self.view,
                    &mut self.section,
                    &mut navigation,
                    &mut right,
                    &mut details_open,
                    sidebar_width,
                    steps,
                    workspace_menu,
                )
            })
            .inner;
        self.sidebar_open = navigation;
        self.files.visible = right;
        self.details.visible = details_open;
        result
    }

    /// Gets the project ready ahead of any task.
    fn prepare(&mut self) {
        self.preparing = true;
        self.view = View::Conversation;

        self.live.notice("Getting the project ready…", Tone::Muted);
        self.engine.prepare();
    }

    fn can_prepare(&self) -> bool {
        matches!(self.status, Status::Ready(_)) && !self.live.running && !self.preparing
    }

    /// The name the files panel keeps its width under, and the width it
    /// starts with: it is wider with a file open in it, and each of the
    /// two keeps the width it was dragged to.
    fn files_panel_id(&self) -> (&'static str, f32) {
        if self.files.has_file() {
            ("files and viewer", FILES_WIDTH_WITH_FILE)
        } else {
            ("files", FILES_WIDTH)
        }
    }

    /// The files of the project, on the right, while they are switched on.
    pub(crate) fn files_panel(&mut self, ui: &mut egui::Ui) {
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::E)) {
            self.files.visible = !self.files.visible;
        }

        if !self.files.visible {
            return;
        }

        let frame = egui::Frame::new()
            .fill(palette::sidebar())
            .inner_margin(Margin::symmetric(12, 8));

        let (id, width) = self.files_panel_id();

        egui::Panel::right(ui.id().with(id))
            .default_size(width)
            .size_range(FILES_MIN_WIDTH..=FILES_MAX_WIDTH)
            .resizable(true)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| self.files.show(ui, &self.place));
    }

    /// The run and what the project has taught, on the right, while
    /// switched on.
    pub(crate) fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::I)) {
            self.inspector.visible = !self.inspector.visible;
        }

        if !self.inspector.visible {
            return;
        }

        let frame = egui::Frame::new()
            .fill(palette::sidebar())
            .inner_margin(Margin::symmetric(16, 12));

        let settings = &self.settings;
        let positive = |value: u32| (value > 0).then_some(value);

        let context = inspector::Context {
            place: &self.place,
            session: &self.live,
            provider: settings.provider.argument(),
            limits: inspector::Limits {
                cost: positive(settings.max_cost_cents).map(|cents| f64::from(cents) / 100.0),
                tokens: positive(settings.max_thousand_tokens)
                    .map(|thousands| u64::from(thousands) * 1000),
                minutes: positive(settings.max_minutes),
            },
            prices: &self.prices,
        };

        egui::Panel::right(ui.id().with("inspector"))
            .default_size(INSPECTOR_WIDTH)
            .size_range(240.0..=420.0)
            .resizable(true)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| self.inspector.show(ui, &context));
    }

    /// What the live session's latest run did, from its log.
    fn trace(&mut self) {
        let Some(log) = &self.live_log else {
            self.live.notice("No run to trace yet.", Tone::Muted);

            return;
        };

        let text = match cortex_core::trace::read(log) {
            Ok(events) => match cortex_core::trace::runs(&events).last() {
                Some(run) => cortex_core::trace::summarize(run).render(),
                None => "No run to trace yet.".to_string(),
            },

            Err(error) => format!("The log could not be read: {error}"),
        };

        self.live.notice(&text, Tone::Muted);
    }

    /// Where the agent works, how full the context is and what changed,
    /// on the far right, while it is switched on.
    pub(crate) fn details_panel(&mut self, ui: &mut egui::Ui) {
        let shortcut = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;

        if ui.input_mut(|input| input.consume_key(shortcut, egui::Key::D)) {
            self.details.visible = !self.details.visible;
        }

        // Looked at even while hidden, so that a finished reading is
        // taken in and the end of a run is noticed.
        self.details
            .refresh(ui.ctx(), &self.place, self.live.running || self.preparing);

        if !self.details.visible {
            return;
        }

        let ready = matches!(self.status, Status::Ready(_));

        let (model, compact_at) = match &self.status {
            Status::Ready(info) => (Some(info.model.as_str()), info.compact_at),
            _ => (None, None),
        };

        let session = match &self.viewing {
            Some((_, session)) => session,
            None => &self.live,
        };

        let facts = details_view::Facts {
            place: &self.place,
            outside_project: self.place == self.scratch,
            model,
            context: session.stats.context.as_ref(),
            auto_compact_at: compact_at,
            compactions: session.stats.compactions,

            // Compacting acts on the live session, so only it can be.
            can_compact: ready
                && self.viewing.is_none()
                && !self.live.running
                && !self.preparing
                && !self.demo,
        };

        let frame = egui::Frame::new()
            .fill(palette::sidebar())
            .inner_margin(Margin::symmetric(12, 8));

        // Beside another panel, a line keeps the two apart.
        let action = egui::Panel::right(ui.id().with(DETAILS_ID))
            .default_size(DETAILS_WIDTH)
            .size_range(DETAILS_MIN_WIDTH..=DETAILS_MAX_WIDTH)
            .resizable(true)
            .show_separator_line(self.files.visible || self.inspector.visible)
            .frame(frame)
            .show(ui, |ui| details_view::show(ui, &mut self.details, &facts))
            .inner;

        match action {
            Some(details_view::Action::Compact) => self.command(Builtin::Compact, ""),
            Some(details_view::Action::ShowFiles) => self.files.visible = true,
            None => {}
        }
    }

    pub(crate) fn sidebar(&mut self, ui: &mut egui::Ui) {
        let running: Vec<_> = self
            .live_log
            .iter()
            .filter(|_| self.live.running)
            .cloned()
            .collect();
        self.sidebar_with_activity(ui, &running);
    }

    pub(crate) fn sidebar_with_activity(&mut self, ui: &mut egui::Ui, running: &[PathBuf]) {
        let frame = egui::Frame::new()
            .fill(palette::sidebar())
            .inner_margin(Margin {
                left: 12,
                right: 12,
                top: 8,
                bottom: 12,
            });

        let shelf = Shelf {
            section: self.section,
            projects: &self.settings.projects,
            scratch: &self.scratch,
            sessions: &self.sessions,
            active: &self.place,
            open: match &self.viewing {
                Some((path, _)) => Some(path.as_path()),
                None => self.live_log.as_deref(),
            },
            running,
            can_change: true,
            in_settings: self.view == View::Settings,
        };

        let action = egui::Panel::left(ui.id().with("sidebar"))
            .default_size(SIDEBAR_WIDTH)
            .size_range(SIDEBAR_MIN_WIDTH..=SIDEBAR_MAX_WIDTH)
            .resizable(true)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| sidebar::show(ui, &shelf, &mut self.confirming))
            .inner;

        let Some(action) = action else {
            return;
        };

        // Anything other than the second press of a delete button means
        // the first press is no longer waiting to be confirmed.
        self.confirming = None;

        match action {
            sidebar::Action::AddProject => self.add_project(),
            sidebar::Action::RemoveProject(project) => self.remove_project(&project),
            sidebar::Action::NewSession(folder) => self.requested_session = Some((folder, None)),
            sidebar::Action::OpenSession(path) => {
                let folder = path.ancestors().nth(3).unwrap_or(&self.place).to_path_buf();
                self.requested_session = Some((folder, Some(path)));
            }
            sidebar::Action::DeleteSession(path) => self.requested_delete = Some(path),
            sidebar::Action::OpenSettings => self.open_settings(),
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new()
            .fill(palette::background())
            .inner_margin(Margin {
                left: 24,
                right: 24,
                top: 8,
                bottom: 16,
            });

        let mode = self.composer_mode();
        let max_steps = self.max_steps();

        // The selectors work only when the agent is free and the live
        // session is the one on screen.
        let can_change = matches!(mode, Mode::Ready | Mode::Unavailable) && !self.demo;

        let (action, change, moved) = egui::Panel::bottom(ui.id().with("footer"))
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                // The footer lines up with the conversation column above it.
                let width = ui.available_width().min(theme::COLUMN_WIDTH);
                let margin = (ui.available_width() - width) / 2.0;

                ui.horizontal(|ui| {
                    ui.add_space(margin);

                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = theme::space::S;

                        if self.view == View::Conversation {
                            self.task_list(ui);
                        }

                        self.refresh_commands();

                        let can_continue = self.viewing.is_none() && self.live.can_continue;

                        let queued: &[String] = match self.viewing {
                            Some(_) => &[],
                            None => &self.queued,
                        };

                        let moved = if self.view == View::Conversation {
                            composer::place_row(
                                ui,
                                &self.place,
                                &self.scratch,
                                &self.settings.projects,
                            )
                        } else {
                            None
                        };

                        let action = composer::show(
                            ui,
                            &mut self.draft,
                            &mut self.attachments,
                            mode,
                            can_continue,
                            queued,
                            &self.commands,
                        );

                        let stand_in = match &self.status {
                            Status::Ready(info) if info.demo => Some(info.model.as_str()),
                            _ => None,
                        };

                        let session = match &self.viewing {
                            Some((_, session)) => session,
                            None => &self.live,
                        };

                        let reading = Reading {
                            stats: &session.stats,
                            max_steps,
                            live_speed: session
                                .writing
                                .as_ref()
                                .and_then(|writing| writing.estimated_speed()),
                            stand_in,
                        };

                        let change = stats_strip::show(ui, &reading, &self.settings, can_change);

                        (action, change, moved)
                    })
                    .inner
                })
                .inner
            })
            .inner;

        match action {
            Some(composer::Action::Send(instruction, images)) => {
                match commands::parse(&instruction) {
                    Some((command, arguments)) => self.command(command, &arguments),
                    None => self.submit(&instruction, images),
                }
            }
            Some(composer::Action::Refused(why)) => self.live.notice(&why, Tone::Bad),
            Some(composer::Action::Steer(message)) => match commands::parse(&message) {
                Some((command, arguments)) => self.command(command, &arguments),
                None => self.steer(&message),
            },
            Some(composer::Action::Recall) => self.recall_last(),
            Some(composer::Action::Continue) => self.carry_on(),
            Some(composer::Action::Stop) => self.engine.stop(),
            None => {}
        }

        if let Some(change) = change {
            self.apply_change(change);
        }

        match moved {
            Some(composer::PlaceAction::Open(folder)) => {
                self.requested_session = Some((folder, None));
            }
            Some(composer::PlaceAction::AddProject) => self.add_project(),
            None => {}
        }
    }

    /// The plan of the session on screen, pinned above the composer while
    /// it is being worked through.
    fn task_list(&mut self, ui: &mut egui::Ui) {
        if self.tasks_hidden {
            return;
        }

        let session = match &self.viewing {
            Some((_, session)) => session,
            None => &self.live,
        };

        let Some(plan) = session.plan() else {
            return;
        };

        if plan.is_finished() && !session.running {
            return;
        }

        tasks::show(ui, &plan, &mut self.tasks_open, session.running);
    }

    /// The slash commands, with the skills of the folder the agent works
    /// in. Read again only when the folder changes.
    fn refresh_commands(&mut self) {
        if self.commands_for.as_deref() == Some(self.place.as_path()) {
            return;
        }

        let library = cortex_skills::SkillLibrary::for_project(&self.place);

        self.commands = commands::entries(library.skills().map(|skill| {
            (
                skill.name.clone(),
                skill.description.clone(),
                skill.argument_hint.clone(),
            )
        }));

        self.commands_for = Some(self.place.clone());
    }

    /// Carries out one of the app's own slash commands.
    fn command(&mut self, command: Builtin, arguments: &str) {
        let busy = self.live.running || self.preparing;

        // These change the project or the conversation under a run.
        let needs_quiet = matches!(
            command,
            Builtin::Undo
                | Builtin::Restore
                | Builtin::Compact
                | Builtin::Prepare
                | Builtin::New
                | Builtin::Worktree
        );

        if busy && needs_quiet {
            self.live
                .notice("Wait for the run to finish, or /stop it first.", Tone::Bad);

            return;
        }

        let ready = matches!(self.status, Status::Ready(_));

        if !ready
            && !matches!(
                command,
                Builtin::Help
                    | Builtin::Settings
                    | Builtin::Tasks
                    | Builtin::Run
                    | Builtin::Learned
                    | Builtin::Trace
            )
        {
            self.live
                .notice("Set up a model in Settings first.", Tone::Bad);

            return;
        }

        self.viewing = None;

        match command {
            Builtin::New => self.start_session(self.place.clone()),
            Builtin::Undo => self.engine.restore(None),

            Builtin::Restore if arguments.is_empty() => {
                self.live.notice(
                    "Name a checkpoint: /restore <id>. /checkpoints lists them.",
                    Tone::Bad,
                );
            }

            Builtin::Restore => self.engine.restore(Some(arguments.to_string())),
            Builtin::Checkpoints => self.engine.checkpoints(),
            Builtin::Diff => self.engine.diff(),
            Builtin::Notes => self.engine.notes(),
            Builtin::Run => self.inspector.show_run(),
            Builtin::Learned => self.inspector.show_learned(),
            Builtin::Trace => self.trace(),

            Builtin::Worktree if arguments.is_empty() => {
                self.live
                    .notice("Name the branch: /worktree <name>.", Tone::Bad);
            }

            Builtin::Worktree => self.engine.worktree(arguments.to_string()),
            Builtin::Compact => self.engine.compact(),

            Builtin::Tasks => {
                self.tasks_hidden = !self.tasks_hidden;

                if !self.tasks_hidden && self.live.plan().is_none() {
                    self.live.notice("No task list yet.", Tone::Muted);
                }
            }

            Builtin::Prepare if self.can_prepare() => self.prepare(),
            Builtin::Prepare => {}
            Builtin::Stop => self.engine.stop(),
            Builtin::Settings => self.open_settings(),
            Builtin::Help => {
                let help = commands::help(&self.commands);

                self.live.notice(&help, Tone::Muted);
            }
        }
    }

    fn centre(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new()
            .fill(palette::background())
            .stroke(Stroke::NONE);

        egui::CentralPanel::default()
            .frame(frame)
            .show(ui, |ui| match self.view {
                View::Conversation => {
                    if self.loading_session.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Opening chat…");
                        });
                    }
                    self.conversation(ui);
                }

                View::Insights => crate::views::insights::show(ui, &mut self.insights),
                View::Settings => self.settings_page(ui),
            });
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        if self.pending_settings.is_some() {
            ui.label("Changes will apply when this agent finishes.");
        }
        let form = self.form.get_or_insert_with(|| Form::new(&self.settings));

        let workspace = settings_page::Workspace {
            can_prepare: matches!(self.status, Status::Ready(_))
                && !self.live.running
                && !self.preparing,
        };

        match settings_page::show(ui, form, &workspace) {
            Some(settings_page::Action::Apply(settings)) => {
                self.form = None;
                self.apply_settings(*settings);
            }
            Some(settings_page::Action::Back) => self.show_chat(),

            // Only what the window draws changes: the agent is left alone.
            Some(settings_page::Action::Theme(name)) => {
                palette::choose(name);
                theme::restyle(ui.ctx());

                self.settings.theme = name.to_string();
                self.save_settings();
            }

            Some(settings_page::Action::Fonts(fonts)) => {
                self.settings.interface_font = fonts.interface.clone();
                self.settings.reply_font = fonts.reply.clone();
                self.settings.code_font = fonts.code.clone();

                theme::fonts::select(fonts);
                theme::apply(ui.ctx());
                self.save_settings();
            }

            Some(settings_page::Action::Prepare) if self.can_prepare() => self.prepare(),
            Some(settings_page::Action::Prepare) => {}

            // Nothing about the agent changes, so it is left alone.
            None => {}
        }
    }

    fn conversation(&mut self, ui: &mut egui::Ui) {
        if let Status::Unavailable(problem) = &self.status
            && self.viewing.is_none()
            && self.problem_banner(ui, problem)
        {
            self.open_settings();

            return;
        }

        // Another session on screen is laid out and scrolled afresh.
        let shown = self.viewing.as_ref().map(|(path, _)| path.as_path());

        if self.chat_shows.as_deref() != shown {
            self.chat_shows = shown.map(Path::to_path_buf);
            self.chat.reset();
        }

        // Questions belong to the live session, not to a past one.
        let question = match self.viewing {
            Some(_) => None,
            None => self.approvals.front().map(|approval| &approval.request),
        };

        let session = match &mut self.viewing {
            Some((_, session)) => session,
            None => &mut self.live,
        };

        let decision = conversation::show(
            ui,
            &mut self.chat,
            session,
            question,
            self.settings.show_thinking,
        );

        if let Some(decision) = decision
            && let Some(approval) = self.approvals.pop_front()
        {
            approval.answer(decision);
        }
    }

    /// Says why there is no agent. Returns whether the way to settings
    /// was taken.
    fn problem_banner(&self, ui: &mut egui::Ui, problem: &str) -> bool {
        let frame = egui::Frame::new().inner_margin(Margin::symmetric(24, 14));

        egui::Panel::top(ui.id().with("problem"))
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                // The banner lines up with the conversation column below it.
                let width = ui.available_width().min(theme::COLUMN_WIDTH);
                let margin = (ui.available_width() - width) / 2.0;

                ui.horizontal(|ui| {
                    ui.add_space(margin);

                    theme::card()
                        .stroke(Stroke::new(1.0, palette::warning().gamma_multiply(0.6)))
                        .show(ui, |ui| {
                            ui.set_width(width - 26.0);

                            ui.label(
                                RichText::new("Cortex is not set up yet").font(theme::strong(14.0)),
                            );

                            ui.label(RichText::new(problem).color(palette::text_secondary()));

                            ui.button("Open settings").clicked()
                        })
                        .inner
                })
                .inner
            })
            .inner
    }
}

impl eframe::App for CortexApp {
    fn raw_input_hook(&mut self, _context: &egui::Context, input: &mut egui::RawInput) {
        input.events.extend(self.rehearsal.take_typed());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.tick(ui);
        self.top_bar(ui);
        if self.sidebar_visible() {
            self.sidebar(ui);
        }
        self.details_panel(ui);
        self.files_panel(ui);
        self.inspector_panel(ui);
        self.render_pane(ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        palette::background().to_normalized_gamma_f32()
    }
}

impl CortexApp {
    pub(crate) fn tick(&mut self, ui: &mut egui::Ui) -> bool {
        // Looked at on every frame, whatever is on screen: see there.
        typeset::begin_frame(ui.ctx());

        let revision = self.history_revision;
        self.absorb_updates();
        self.poll_background();
        self.approvals.retain(|approval| !approval.answered());
        if !self.live.running
            && !self.preparing
            && let Some(settings) = self.pending_settings.take()
        {
            self.apply_settings(settings);
        }

        if matches!(self.status, Status::Ready(_))
            && let Some(path) = self.rehearsal.take_session()
        {
            self.open_past_session(path);
        }

        let paths = self.rehearsal.take_attachments();
        if let Some(error) = self.attachments.enqueue(&paths) {
            self.live.notice(&error, Tone::Bad);
        }
        if let Some(error) = self.attachments.poll(ui.ctx()) {
            self.live.notice(&error, Tone::Bad);
        }

        // The rehearsed instruction waits until there is an agent for it.
        if matches!(self.status, Status::Ready(_))
            && self.loading_session.is_none()
            && !self.attachments.loading()
            && let Some(instruction) = self.rehearsal.take_instruction()
        {
            let (instruction, images) = self.attachments.take_message(&instruction);

            self.submit(&instruction, images);
        }

        if self.live.can_continue && self.rehearsal.take_continue() {
            self.carry_on();
        }

        if self.rehearsal.take_stop() {
            self.engine.stop();
        }

        if self.live.running
            && let Some(message) = self.rehearsal.take_steer()
        {
            self.steer(&message);
        }

        // The first run is over and its session is listed: open it from
        // the list, as a click would, and carry on in it.
        let newest = self
            .sessions
            .get(&self.place)
            .and_then(|sessions| sessions.first())
            .map(|session| session.path.clone());

        if self.rehearsal.reopens()
            && !self.live.running
            && !self.live.items.is_empty()
            && let Some(path) = newest
            && let Some(instruction) = self.rehearsal.take_reopen()
        {
            self.open_past_session(path);
            self.rehearsal.defer_instruction(instruction);
        }

        // The "thinking" square pulses while a run is in progress.
        if self.live.running {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }

        self.rehearsal.advance(ui.ctx());
        revision != self.history_revision
    }

    pub(crate) fn render_pane(&mut self, ui: &mut egui::Ui) {
        // The composer is shown only in the conversation.
        if !matches!(self.view, View::Settings | View::Insights) {
            self.footer(ui);
        }
        self.centre(ui);
    }

    pub(crate) fn take_requested_delete(&mut self) -> Option<PathBuf> {
        self.requested_delete.take()
    }
    pub(crate) fn notice(&mut self, text: &str) {
        self.live.notice(text, Tone::Muted);
    }
    pub(crate) fn show_chat(&mut self) {
        self.cancel_session_load();
        self.view = View::Conversation;
    }

    pub(crate) fn refresh_history(&mut self) {
        self.refresh_sessions();
    }

    pub(crate) fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
    }
    pub(crate) fn sidebar_visible(&self) -> bool {
        self.sidebar_open
    }
    pub(crate) fn running(&self) -> bool {
        self.live.running
    }
    pub(crate) fn log_path(&self) -> Option<&Path> {
        self.live_log.as_deref()
    }
    pub(crate) fn take_requested_session(&mut self) -> Option<(PathBuf, Option<PathBuf>)> {
        self.requested_session.take()
    }
    pub(crate) fn fork(&self) -> Self {
        Self::new(
            self.engine.fork(),
            self.settings.clone(),
            Some(self.place.clone()),
            self.demo,
            Rehearsal::new(crate::args::RehearsalArgs::default()),
        )
    }
    pub(crate) fn open_requested(&mut self, folder: PathBuf, log: Option<PathBuf>) {
        if let Some(log) = log {
            self.open_past_session(log);
        } else if folder != self.place {
            self.start_session(folder);
        }
    }
    pub(crate) fn label(&self) -> String {
        self.live
            .items
            .iter()
            .find_map(|item| match item {
                crate::session::Item::User { text, .. } => Some(text.chars().take(28).collect()),
                _ => None,
            })
            .unwrap_or_else(|| "New chat".to_string())
    }
    pub(crate) fn agent_id(&self) -> &str {
        self.engine.agent_id()
    }
    pub(crate) fn stop(&self) {
        self.engine.stop();
    }
    /// The folder whose background jobs go with this chat: its project,
    /// or `None` for the demo, whose stand-in model cannot run jobs.
    pub(crate) fn jobs_place(&self) -> Option<&Path> {
        (!self.demo).then_some(self.place.as_path())
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    #[test]
    fn returning_from_settings_preserves_the_current_chat() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut app = CortexApp::new(
            Engine::start(runtime.handle(), egui::Context::default()),
            Settings::default(),
            None,
            true,
            Rehearsal::new(crate::args::RehearsalArgs::default()),
        );
        app.live_log = Some(PathBuf::from("current.jsonl"));
        app.live.items.push(crate::session::Item::User {
            text: "Current task".into(),
            images: Vec::new(),
        });
        app.live.running = true;
        app.open_settings();
        app.show_chat();
        assert_eq!(app.view, View::Conversation);
        assert_eq!(app.log_path(), Some(Path::new("current.jsonl")));
        assert!(app.live.running);
        assert_eq!(app.live.items.len(), 1);
    }

    /// Draws the whole window once and says how soon it asked to be
    /// drawn again.
    fn draw(context: &egui::Context, app: &mut CortexApp) -> std::time::Duration {
        // Whatever the engine has reported, the agent is ready and idle.
        app.status = Status::Ready(Info {
            place: app.place.clone(),
            model: "stand-in".to_string(),
            max_steps: 10,
            demo: true,
            compact_at: None,
        });

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };

        let mut output = context.run_ui(input, |ui| {
            app.tick(ui);
            app.top_bar(ui);
            app.sidebar(ui);
            app.details_panel(ui);
            app.files_panel(ui);
            app.render_pane(ui);
        });

        output.textures_delta.clear();

        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
    }

    #[test]
    fn an_idle_window_does_not_draw_itself_again_and_again() {
        let runtime = tokio::runtime::Runtime::new().unwrap();

        // The engine wakes a context of its own, not the one drawn here.
        let mut app = CortexApp::new(
            Engine::start(runtime.handle(), egui::Context::default()),
            Settings::default(),
            None,
            true,
            Rehearsal::new(crate::args::RehearsalArgs::default()),
        );

        app.live.items.push(crate::session::Item::User {
            text: "A finished task".into(),
            images: Vec::new(),
        });

        let context = egui::Context::default();

        theme::apply(&context);

        for _ in 0..5 {
            draw(&context, &mut app);
        }

        // Only the composer's blinking caret asks to be drawn again.
        let idle = draw(&context, &mut app);

        assert!(idle >= std::time::Duration::from_millis(300), "{idle:?}");

        // A run in progress animates, and asks for it.
        app.live.running = true;

        let working = draw(&context, &mut app);

        assert!(
            working <= std::time::Duration::from_millis(100),
            "{working:?}"
        );
    }
}

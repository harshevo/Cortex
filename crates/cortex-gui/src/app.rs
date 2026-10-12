//! The window: holds the state, feeds it what the agent reports, and lays
//! out the views.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use cortex_memory::WorkingMemory;
use eframe::egui::{self, Margin, RichText, Stroke};

use crate::engine::{Engine, Info, Launch, PendingApproval, Update};
use crate::memory::working;
use crate::rehearsal::Rehearsal;
use crate::session::history::{self, PastSession};
use crate::session::{Session, Tone};
use crate::settings::{Provider, Settings, secrets};
use crate::theme::{self, palette};
use crate::views::composer::{self, Mode};
use crate::views::memory_space::{self, MemoryView};
use crate::views::settings::Form;
use crate::views::sidebar::{self, Shelf};
use crate::views::stats_strip::{self, Change, Reading};
use crate::views::{Section, View, conversation, settings as settings_page, top_bar};

const TOP_BAR_HEIGHT: f32 = 40.0;
const SIDEBAR_WIDTH: f32 = 244.0;

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

    /// The engine's working memory for the current run, kept in step by
    /// watching the same events the engine's own copy does.
    working: WorkingMemory,

    /// A past session opened from the sidebar, shown instead of `live`.
    viewing: Option<(PathBuf, Session)>,

    /// The past sessions of every folder in the sidebar, newest first.
    sessions: HashMap<PathBuf, Vec<PastSession>>,
    history_revision: u64,

    /// The session or project whose delete button has been pressed once.
    confirming: Option<PathBuf>,

    /// Questions from tools, oldest first. The first is on screen.
    approvals: VecDeque<PendingApproval>,

    /// The instruction being typed.
    draft: String,

    /// What has been sent to the run in progress and not yet read by the
    /// model.
    queued: Vec<String>,

    /// The log of the live session, when it is one that was taken up
    /// again. A session started here has no log until its first run.
    live_log: Option<PathBuf>,

    memory: MemoryView,

    /// The settings page, while it is open.
    form: Option<Form>,

    /// API keys already read from the keychain, so that it is asked once
    /// rather than on every change of model.
    keys: HashMap<Provider, Option<String>>,

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
            .or_else(|| settings.projects.first().cloned())
            .unwrap_or_else(|| scratch.clone());

        let view = if rehearsal.opens_memory() {
            View::MemorySpace
        } else {
            View::Conversation
        };

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
            view,
            section,
            sidebar_open: true,
            live: Session::default(),
            preparing: false,
            working: WorkingMemory::default(),
            viewing: None,
            sessions: HashMap::new(),
            history_revision: 0,
            confirming: None,
            approvals: VecDeque::new(),
            draft: String::new(),
            queued: Vec::new(),
            live_log: None,
            memory: MemoryView::default(),
            form: None,
            keys: HashMap::new(),
            rehearsal,
            requested_session: None,
            requested_delete: None,
            pending_settings: None,
        };

        app.refresh_sessions();
        app.launch(true);

        if app.rehearsal.opens_settings() {
            app.open_settings();
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
        let provider = self.settings.provider;

        let api_key = if self.demo {
            None
        } else {
            self.keys
                .entry(provider)
                .or_insert_with(|| secrets::resolve(provider))
                .clone()
        };

        if fresh {
            self.live = Session::default();
            self.working = WorkingMemory::default();
            self.viewing = None;
            self.approvals.clear();
            self.memory = MemoryView::default();
            self.queued.clear();
            self.live_log = None;
        }

        self.status = Status::Starting;

        self.engine.launch(Launch {
            place: self.place.clone(),
            settings: self.settings.clone(),
            api_key,
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
        let folders = self.settings.projects.iter().chain([&self.scratch]);

        self.sessions = folders
            .map(|folder| (folder.clone(), history::list(folder)))
            .collect();
    }

    /// Takes in everything the agent has reported since the last frame.
    fn absorb_updates(&mut self, now: f64) {
        let updates: Vec<Update> = self.engine.updates().collect();

        for update in updates {
            match update {
                Update::BrainInspection(states) => self.memory.activity.inspection = states,
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
                    if let cortex_core::Event::BrainObserved {
                        source,
                        step,
                        detail,
                    } = &event
                    {
                        self.memory.observe_brain(source, *step, detail, now);
                    }
                    self.heard(&event);
                    self.working.observe(&event);
                    self.live.apply(&event);
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

                Update::Prepared { text, failed } => {
                    self.preparing = false;

                    let tone = if failed { Tone::Bad } else { Tone::Muted };

                    self.live.notice(&text, tone);
                }

                Update::Memory(remembered) => {
                    self.memory.load(remembered);

                    if self.rehearsal.selects_newest() {
                        self.memory.select_newest();
                    }
                }

                Update::MemoryFailed(problem) => self.memory.problem = Some(problem),
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

    fn submit(&mut self, instruction: &str) {
        self.viewing = None;
        self.live.running = true;

        self.engine.submit(instruction);
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
        self.view = View::Conversation;

        let elsewhere = folder != self.place && !self.demo;

        if elsewhere || !matches!(self.status, Status::Ready(_)) {
            self.place = folder;

            self.launch(true);

            return;
        }

        self.viewing = None;
        self.live = Session::default();
        self.working = WorkingMemory::default();
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
        self.view = View::Conversation;

        if self.live_log.as_deref() == Some(path.as_path()) {
            self.viewing = None;

            return;
        }

        let session = history::load(&path);

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
            self.working = WorkingMemory::default();
            self.approvals.clear();

            self.engine.open(path.clone());
        }

        self.working = WorkingMemory::default();
        self.memory.activity = crate::brain::activity::ActivityView::default();
        for event in history::events(&path) {
            self.working.observe(&event);
            if let cortex_core::Event::BrainObserved {
                source,
                step,
                detail,
            } = event
            {
                self.memory.observe_brain(&source, step, &detail, 0.0);
            }
        }
        self.memory.activity.pause();
        self.live = session;
        self.live_log = Some(path);
    }

    pub(crate) fn delete_session(&mut self, path: &Path) {
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

        // A key may have been saved or removed on the page.
        self.keys.clear();

        self.save_settings();
        self.launch(fresh);

        self.view = View::Conversation;
    }

    /// A choice made in the row under the composer.
    fn apply_change(&mut self, change: Change) {
        match change {
            Change::Model(model) => self.settings.current_mut().model = model,
            Change::Effort(effort) => self.settings.effort = effort,
            Change::ApproveEverything(approve) => self.settings.approve_everything = approve,

            // Only what the window draws changes: the agent is left alone.
            Change::ShowThinking(show) => {
                self.settings.show_thinking = show;
                self.save_settings();

                return;
            }
        }

        self.save_settings();
        self.launch(false);
    }

    /// The session on screen: the past one being viewed, or the live one.
    fn shown_session(&mut self) -> &mut Session {
        match &mut self.viewing {
            Some((_, session)) => session,
            None => &mut self.live,
        }
    }

    fn composer_mode(&self) -> Mode {
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
        self.top_bar_with_menu(ui, |_| {});
    }

    pub(crate) fn top_bar_with_menu(
        &mut self,
        ui: &mut egui::Ui,
        workspace_menu: impl FnOnce(&mut egui::Ui),
    ) {
        let frame = egui::Frame::new()
            .fill(palette::SIDEBAR)
            .inner_margin(Margin::symmetric(8, 0));

        let project = (self.place != self.scratch)
            .then(|| self.place.file_name())
            .flatten()
            .map(|name| name.to_string_lossy().into_owned());

        let place = top_bar::Place {
            project: project.as_deref(),
            demo: self.demo,
            memories: self.memory.episode_count(),
            busy: self.live.running || self.preparing || matches!(self.status, Status::Starting),
        };

        egui::Panel::top(ui.id().with("top bar"))
            .exact_size(TOP_BAR_HEIGHT)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                let can_prepare = matches!(self.status, Status::Ready(_))
                    && !self.live.running
                    && !self.preparing;

                let asked = top_bar::show(
                    ui,
                    &place,
                    &mut self.view,
                    &mut self.section,
                    &mut self.sidebar_open,
                    can_prepare,
                    workspace_menu,
                );

                if asked {
                    self.preparing = true;
                    self.view = View::Conversation;

                    self.live.notice("Getting the project ready…", Tone::Muted);
                    self.engine.prepare();
                }
            });
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
            .fill(palette::SIDEBAR)
            .inner_margin(Margin::symmetric(10, 6));

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
            .exact_size(SIDEBAR_WIDTH)
            .resizable(false)
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
            .fill(palette::BACKGROUND)
            .inner_margin(Margin::symmetric(24, 10));

        let mode = self.composer_mode();
        let max_steps = self.max_steps();

        // The selectors work only when the agent is free and the live
        // session is the one on screen.
        let can_change = matches!(mode, Mode::Ready | Mode::Unavailable) && !self.demo;

        let (action, change) = egui::Panel::bottom(ui.id().with("footer"))
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
                        ui.spacing_mut().item_spacing.y = 4.0;

                        let can_continue = self.viewing.is_none() && self.live.can_continue;

                        let queued: &[String] = match self.viewing {
                            Some(_) => &[],
                            None => &self.queued,
                        };

                        let action =
                            composer::show(ui, &mut self.draft, mode, can_continue, queued);

                        let settings = self.settings.clone();

                        let stand_in = match &self.status {
                            Status::Ready(info) if info.demo => Some(info.model.clone()),
                            _ => None,
                        };

                        let session = self.shown_session();

                        let reading = Reading {
                            stats: &session.stats,
                            max_steps,
                            live_speed: session
                                .writing
                                .as_ref()
                                .and_then(|writing| writing.estimated_speed()),
                            stand_in: stand_in.as_deref(),
                        };

                        let change = stats_strip::show(ui, &reading, &settings, can_change);

                        (action, change)
                    })
                    .inner
                })
                .inner
            })
            .inner;

        match action {
            Some(composer::Action::Send(instruction)) => self.submit(&instruction),
            Some(composer::Action::Steer(message)) => self.steer(&message),
            Some(composer::Action::Continue) => self.carry_on(),
            Some(composer::Action::Stop) => self.engine.stop(),
            None => {}
        }

        if let Some(change) = change {
            self.apply_change(change);
        }
    }

    fn centre(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new()
            .fill(palette::BACKGROUND)
            .stroke(Stroke::NONE);

        egui::CentralPanel::default()
            .frame(frame)
            .show(ui, |ui| match self.view {
                View::Conversation => self.conversation(ui),
                View::MemorySpace => self.memory_space(ui),
                View::Settings => self.settings_page(ui),
            });
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        if self.pending_settings.is_some() {
            ui.label("Changes will apply when this agent finishes.");
        }
        let form = self.form.get_or_insert_with(|| Form::new(&self.settings));

        match settings_page::show(ui, form) {
            Some(settings_page::Action::Apply(settings)) => {
                self.form = None;
                self.apply_settings(*settings);
            }
            Some(settings_page::Action::Back) => self.show_chat(),
            None => {}
        }
    }

    fn memory_space(&mut self, ui: &mut egui::Ui) {
        self.memory.set_thoughts(working::thoughts(&self.working));

        memory_space::show(ui, &mut self.memory, self.live.running);
        if let Some(query) = self.memory.activity.request.take() {
            self.engine.inspect_brain(query);
        }
    }

    fn conversation(&mut self, ui: &mut egui::Ui) {
        if let Status::Unavailable(problem) = &self.status
            && self.viewing.is_none()
            && self.problem_banner(ui, &problem.clone())
        {
            self.open_settings();

            return;
        }

        // Questions belong to the live session, not to a past one.
        let question = match self.viewing {
            Some(_) => None,
            None => self
                .approvals
                .front()
                .map(|approval| approval.request.clone()),
        };

        let show_thinking = self.settings.show_thinking;

        let decision =
            conversation::show(ui, self.shown_session(), question.as_ref(), show_thinking);

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
                        .stroke(Stroke::new(1.0, palette::WARNING.gamma_multiply(0.6)))
                        .show(ui, |ui| {
                            ui.set_width(width - 26.0);

                            ui.label(
                                RichText::new("Cortex is not set up yet").font(theme::strong(14.0)),
                            );

                            ui.label(RichText::new(problem).color(palette::TEXT_SECONDARY));

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
        if self.sidebar_open {
            self.sidebar(ui);
        }
        self.render_pane(ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        palette::BACKGROUND.to_normalized_gamma_f32()
    }
}

impl CortexApp {
    pub(crate) fn tick(&mut self, ui: &mut egui::Ui) -> bool {
        let revision = self.history_revision;
        self.absorb_updates(ui.input(|i| i.time));
        if !self.live.running
            && !self.preparing
            && let Some(settings) = self.pending_settings.take()
        {
            self.apply_settings(settings);
        }

        // The rehearsed instruction waits until there is an agent for it.
        if matches!(self.status, Status::Ready(_))
            && let Some(instruction) = self.rehearsal.take_instruction()
        {
            self.submit(&instruction);
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
            self.submit(&instruction);
        }

        // The "thinking" square pulses while a run is in progress.
        if self.live.running {
            ui.ctx().request_repaint();
        }

        self.rehearsal.advance(ui.ctx());
        revision != self.history_revision
    }

    pub(crate) fn render_pane(&mut self, ui: &mut egui::Ui) {
        if self.view != View::Settings {
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
                crate::session::Item::User { text } => Some(text.chars().take(28).collect()),
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
        });
        app.live.running = true;
        app.open_settings();
        app.show_chat();
        assert_eq!(app.view, View::Conversation);
        assert_eq!(app.log_path(), Some(Path::new("current.jsonl")));
        assert!(app.live.running);
        assert_eq!(app.live.items.len(), 1);
    }
}

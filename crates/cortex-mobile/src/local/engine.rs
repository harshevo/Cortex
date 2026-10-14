//! The background task that owns the agent on this phone.
//!
//! It is the desktop app's engine without a window: the agent is put
//! together by the same [`features::setup`] the `cortex` command uses,
//! and everything it does is published as the messages a desktop would
//! send, so the screens cannot tell the two apart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use cortex_cli::features::{self, MemoryLocation};
use cortex_core::agent::close_unanswered_calls;
use cortex_core::trajectory::unix_millis;
use cortex_core::{
    Agent, AgentConfig, AgentError, ApprovalRequest, Approver, AutoApprove, Block, Event, Image,
    Inbox, JsonlSink, Message, ModelProvider, Progress, ProviderError, Sandbox, StreamObserver,
    ToolContext, TrajectorySink, Workspace, prompt, replay,
};
use cortex_living::Living;
use cortex_providers::{
    AnthropicConfig, AnthropicProvider, CodexConfig, CodexProvider, OpenAiCompatConfig,
    OpenAiCompatProvider,
};
use cortex_remote::Mirror;
use cortex_remote::wire::{
    ChatDown, ChatInfo, ChatStatus, ChatUp, Decision, Down, HostState, PastSession, Project,
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;

use super::settings::{self, Settings};
use super::{Ask, CHAT, memory};
use crate::lock;

/// Folder inside a project where the engine logs every run, as the
/// desktop does.
const TRAJECTORIES: &str = ".cortex/trajectories";

/// The project that sessions about nothing in particular work in.
const SCRATCH: &str = "Scratch";

const SESSIONS_LISTED: usize = 30;

/// Told to the model, so that it does not reach for what is not there.
const PHONE_NOTE: &str = "You are running on a phone, inside an app. The project's files are real \
and yours to read and change, but there are no compilers, package managers, test runners or git \
here, so a command that needs one will fail: say what should be run on a computer instead of \
trying it repeatedly.";

/// What everything in the engine publishes through.
#[derive(Clone)]
struct Outbox {
    mirror: Arc<Mirror>,
}

impl Outbox {
    fn tell(&self, message: ChatDown) {
        self.mirror.publish(Down::Chat {
            chat: CHAT.to_string(),
            message,
        });
    }

    fn notice(&self, text: impl Into<String>, bad: bool) {
        self.tell(ChatDown::Notice {
            text: text.into(),
            bad,
        });
    }
}

/// The agent in use, and what goes with it.
struct Running {
    agent: Agent,
    inbox: Arc<Inbox>,
    log: Arc<SessionLog>,

    /// Records how each run went, and hears when the person stops one.
    effect: Option<Arc<cortex_core::effect::EffectSink>>,

    /// The provider's name, to tell whose conversation a log holds.
    provider: String,

    memory: MemoryLocation,
}

struct Engine {
    folder: PathBuf,
    settings: Arc<Mutex<Settings>>,
    outbox: Outbox,
    questions: Arc<Questions>,

    /// The folder the agent works in.
    place: PathBuf,

    running: Option<Running>,
    status: ChatStatus,
    conversation: Vec<Message>,

    /// A run that stopped on a provider failure and can be carried on.
    interrupted: Option<Progress>,

    /// The first words of the conversation.
    title: Option<String>,

    busy: bool,

    /// The settings changed during a run: the agent is built again when
    /// the run is over.
    stale: bool,

    /// A model to use whatever the settings say: a scripted one, in tests.
    stand_in: Option<Arc<dyn ModelProvider>>,
}

pub async fn run(
    folder: PathBuf,
    settings: Arc<Mutex<Settings>>,
    mirror: Arc<Mirror>,
    mut asks: UnboundedReceiver<Ask>,
    stand_in: Option<Arc<dyn ModelProvider>>,
) {
    let place = lock(&settings)
        .project
        .clone()
        .filter(|project| project.is_dir())
        .unwrap_or_else(|| projects(&folder).join(SCRATCH));

    let mut engine = Engine {
        folder,
        settings,
        outbox: Outbox { mirror },
        questions: Arc::default(),
        place,
        running: None,
        status: ChatStatus::Starting,
        conversation: Vec::new(),
        interrupted: None,
        title: None,
        busy: false,
        stale: false,
        stand_in,
    };

    engine.build(true).await;

    while let Some(ask) = asks.recv().await {
        engine.obey(ask, &mut asks).await;

        if engine.stale {
            engine.stale = false;
            engine.build(false).await;
        }
    }
}

impl Engine {
    async fn obey(&mut self, ask: Ask, asks: &mut UnboundedReceiver<Ask>) {
        match ask {
            Ask::Chat(ChatUp::Submit { text, images }) => self.instruct(text, images, asks).await,
            Ask::Chat(ChatUp::Steer { text }) => self.instruct(text, Vec::new(), asks).await,

            Ask::Chat(ChatUp::Continue) => {
                match self.interrupted.take() {
                    Some(progress) => {
                        self.carry_out(Start::CarryOn(progress), asks).await;
                    }

                    // Nothing to carry on: the button was stale.
                    None => self.outbox.tell(ChatDown::Idle),
                }
            }

            // Nothing is running.
            Ask::Chat(ChatUp::Stop) => {}

            Ask::Chat(ChatUp::NewSession { folder }) => {
                let moved = folder
                    .map(PathBuf::from)
                    .filter(|folder| *folder != self.place && folder.is_dir());

                match moved {
                    Some(folder) => {
                        self.place = folder;
                        self.build(true).await;
                    }

                    None => self.forget_conversation(),
                }
            }

            Ask::Chat(ChatUp::OpenSession { path }) => self.open(Path::new(&path)).await,

            Ask::NewProject(name) => match self.create_project(&name) {
                Ok(project) => {
                    self.place = project;
                    self.build(true).await;
                }

                Err(error) => self.outbox.notice(format!("{error:#}"), true),
            },

            ask => {
                if self.change(ask) {
                    self.build(false).await;
                }
            }
        }
    }

    /// Takes in a change to the settings. Returns whether the agent has
    /// to be built again for it.
    fn change(&mut self, ask: Ask) -> bool {
        let mut settings = lock(&self.settings);

        let rebuild = match ask {
            Ask::Chat(ChatUp::Approve { id, decision }) => {
                self.questions.answer(id, decision);

                return false;
            }

            Ask::Chat(ChatUp::SetCapability { name, on }) => {
                settings.set_capability(&name, on);

                true
            }

            Ask::Chat(ChatUp::SetModel { provider, model }) => {
                if !settings::is_provider(&provider) || model.trim().is_empty() {
                    return false;
                }

                settings
                    .models
                    .insert(provider.clone(), model.trim().to_string());
                settings.provider = provider;

                true
            }

            Ask::Chat(ChatUp::SetEffort { effort }) => {
                if !settings::is_effort(&effort) {
                    return false;
                }

                settings.effort = effort;

                true
            }

            Ask::Chat(ChatUp::SetApproveEverything { on }) => {
                settings.approve_everything = on;

                true
            }

            Ask::SetKey { provider, key } => {
                settings.keys.insert(provider, key.trim().to_string());

                true
            }

            Ask::SetAddress(address) => {
                settings.address = address.trim().to_string();

                true
            }

            // The sign-in changed: the provider is made again with it.
            Ask::Reload => true,

            _ => false,
        };

        if let Err(error) = settings.save(&self.folder) {
            self.outbox
                .notice(format!("The settings were not saved: {error:#}"), true);
        }

        rebuild
    }

    /// Builds the agent from the settings as they are now. `fresh`
    /// forgets the conversation, as moving to another project does.
    async fn build(&mut self, fresh: bool) {
        self.status = ChatStatus::Starting;
        self.publish_state();

        if fresh {
            self.conversation.clear();
            self.interrupted = None;
            self.title = None;

            self.outbox.tell(ChatDown::Replaced { events: Vec::new() });
        }

        let previous = self.running.take();

        match self.assemble(previous.filter(|_| !fresh)).await {
            Ok(running) => {
                {
                    let mut settings = lock(&self.settings);

                    settings.project = Some(self.place.clone());

                    let _ = settings.save(&self.folder);
                }

                tokio::spawn(report_memory(
                    running.memory.path.clone(),
                    running.memory.scope.clone(),
                    self.outbox.clone(),
                ));

                self.running = Some(running);
                self.status = ChatStatus::Ready;
            }

            Err(error) => {
                self.status = ChatStatus::Unavailable {
                    reason: format!("{error:#}"),
                };
            }
        }

        self.publish_state();
    }

    async fn assemble(&self, previous: Option<Running>) -> Result<Running> {
        let settings = lock(&self.settings).clone();

        std::fs::create_dir_all(&self.place)
            .with_context(|| format!("cannot create the folder {}", self.place.display()))?;

        let model = settings.model_args()?;

        let workspace = Workspace::new(&self.place)
            .with_context(|| format!("cannot open the folder {}", self.place.display()))?
            .with_sandbox(Sandbox::enabled(false));

        let provider = match &self.stand_in {
            Some(provider) => provider.clone(),
            None => build_provider(&settings)?,
        };

        // A project is known by its name, not by where it lies: the
        // system may move the app's folder when the app is updated.
        let scope = workspace
            .root()
            .file_name()
            .map(|name| format!("phone:{}", name.to_string_lossy()))
            .unwrap_or_default();

        let file = workspace.root().join(".cortex").join("memory.db");

        let location = || MemoryLocation {
            path: file.clone(),
            scope: scope.clone(),
            skip_identical: false,
        };

        let setup = features::setup(&model, location()).await?;

        let mut notes = setup.notes(&workspace);

        notes.push(PHONE_NOTE.to_string());

        let config = AgentConfig {
            system_prompt: prompt::system_prompt_with(&workspace, &notes),
            max_steps: model.max_steps,
            prune: setup.prune.clone(),
            max_verification_rounds: model.max_check_rounds,
            fold_repeats: setup.fold_repeats,
            extras: setup.extras.clone(),
        };

        let log = match previous {
            Some(previous) => previous.log,
            None => Arc::new(SessionLog::new(workspace.root())),
        };

        let approver: Arc<dyn Approver> = if settings.approve_everything {
            Arc::new(AutoApprove)
        } else {
            Arc::new(PhoneApprover {
                outbox: self.outbox.clone(),
                questions: self.questions.clone(),
                approve_all: AtomicBool::new(false),
            })
        };

        if let Some(living) = &setup.living {
            tokio::spawn(let_rest(Arc::downgrade(living)));
        }

        let effect = setup.effect.clone();

        {
            let usage = setup.usage.clone();
            let effect = effect.clone();

            log.name_with(Box::new(move |session| {
                if let Some(usage) = &usage {
                    usage.set_session(session);
                }

                if let Some(effect) = &effect {
                    effect.set_session(session);
                }
            }));
        }

        let inbox = Arc::new(Inbox::new());
        let name = provider.name().to_string();

        let context = ToolContext {
            workspace,
            approver,
        };

        let mut agent = Agent::new(provider, setup.tools, context, config)
            .with_sink(Arc::new(self.outbox.clone()))
            .with_sink(log.clone())
            .with_stream_observer(Arc::new(self.outbox.clone()))
            .with_inbox(inbox.clone());

        for sink in setup.sinks {
            agent = agent.with_sink(sink);
        }

        for source in setup.context_sources {
            agent = agent.with_context_source(source);
        }

        for adviser in setup.advisers {
            agent = agent.with_adviser(adviser);
        }

        Ok(Running {
            agent,
            inbox,
            log,
            effect,
            provider: name,
            memory: location(),
        })
    }

    /// Carries out an instruction, and after it whatever was said too
    /// late for the run to hear.
    async fn instruct(
        &mut self,
        mut instruction: String,
        mut images: Vec<Image>,
        asks: &mut UnboundedReceiver<Ask>,
    ) {
        loop {
            // Pictures belong to the instruction they came with.
            let sent = std::mem::take(&mut images);

            if self.title.is_none() {
                self.title = Some(titled(&instruction));
            }

            let finished = self
                .carry_out(Start::Instruction(instruction, sent), asks)
                .await;

            let unheard = match &self.running {
                Some(running) => running.inbox.take(),
                None => Vec::new(),
            };

            if !finished || unheard.is_empty() {
                break;
            }

            instruction = unheard.join("\n\n");
        }
    }

    /// Runs to the end, unless a `Stop` arrives first. Returns whether
    /// the run reached its end. What the person says meanwhile is put in
    /// the agent's inbox.
    async fn carry_out(&mut self, start: Start, asks: &mut UnboundedReceiver<Ask>) -> bool {
        let Some(running) = self.running.take() else {
            let reason = match &self.status {
                ChatStatus::Unavailable { reason } => reason.clone(),
                _ => "The agent is not set up yet.".to_string(),
            };

            self.outbox.tell(ChatDown::RunFailed {
                error: reason,
                can_continue: false,
            });

            self.outbox.tell(ChatDown::Idle);

            return false;
        };

        self.busy = true;
        self.publish_state();

        let mut conversation = std::mem::take(&mut self.conversation);
        let mut failed = false;
        let mut waiting = Vec::new();

        let stopped = {
            let run = async {
                match start {
                    Start::Instruction(instruction, images) => {
                        running
                            .agent
                            .run_with_images(&mut conversation, &instruction, &images)
                            .await
                    }

                    Start::CarryOn(progress) => {
                        running.agent.resume(&mut conversation, progress).await
                    }
                }
            };

            tokio::pin!(run);

            loop {
                tokio::select! {
                    result = &mut run => {
                        if let Err(AgentError::Provider { error, progress }) = result {
                            let can_continue = error.is_retryable();

                            self.outbox.tell(ChatDown::RunFailed {
                                error: error.to_string(),
                                can_continue,
                            });

                            self.interrupted = can_continue.then_some(progress);

                            failed = true;
                        }

                        break false;
                    }

                    ask = asks.recv() => match ask {
                        // A closed app stops the run as well.
                        Some(Ask::Chat(ChatUp::Stop)) => {
                            if let Some(effect) = &running.effect {
                                effect.stopped_by_person();
                            }

                            break true;
                        }

                        None => break true,

                        Some(Ask::Chat(ChatUp::Steer { text } | ChatUp::Submit { text, .. })) => {
                            running.inbox.push(text)
                        }

                        Some(Ask::Chat(ChatUp::Approve { id, decision })) => {
                            self.questions.answer(id, decision)
                        }

                        // Everything else waits for the run to end.
                        Some(ask) => waiting.push(ask),
                    },
                }
            }
        };

        if stopped {
            // The run was dropped where it stood. If the model had just
            // asked for tools, those calls need answers before the
            // conversation can be sent again.
            close_unanswered_calls(&mut conversation);

            self.questions.abandon(&self.outbox);
            self.outbox.tell(ChatDown::RunStopped);
        }

        self.conversation = conversation;
        self.busy = false;

        tokio::spawn(report_memory(
            running.memory.path.clone(),
            running.memory.scope.clone(),
            self.outbox.clone(),
        ));

        self.running = Some(running);

        self.outbox.tell(ChatDown::Idle);
        self.publish_state();

        for ask in waiting {
            if matches!(
                ask,
                Ask::Chat(
                    ChatUp::NewSession { .. } | ChatUp::OpenSession { .. } | ChatUp::Continue
                ) | Ask::NewProject(_)
            ) {
                continue;
            }

            self.stale |= self.change(ask);
        }

        !stopped && !failed
    }

    fn forget_conversation(&mut self) {
        self.conversation.clear();
        self.interrupted = None;
        self.title = None;

        if let Some(running) = &self.running {
            running.log.start_new_session();
        }

        self.outbox.tell(ChatDown::Replaced { events: Vec::new() });
        self.publish_state();
    }

    /// Takes up the session logged at `path`: its conversation becomes
    /// the current one, and what follows is added to its log.
    async fn open(&mut self, path: &Path) {
        // A log lies in its project, two folders down. Only the logs of
        // the projects on this phone are taken up.
        let project = path
            .ancestors()
            .nth(3)
            .filter(|project| project.starts_with(projects(&self.folder)));

        let Some(project) = project.map(Path::to_path_buf) else {
            return;
        };

        if project != self.place {
            self.place = project;
            self.build(true).await;
        }

        let Some(running) = &self.running else {
            return;
        };

        let events: Vec<Event> = events(path).collect();

        let mut conversation = replay::conversation(&events);

        // Blocks only their own provider understands (a model's sealed
        // reasoning) cannot be sent to another.
        let written_by = events.iter().rev().find_map(|event| match event {
            Event::RunStarted { provider, .. } => Some(provider.as_str()),
            _ => None,
        });

        if written_by != Some(running.provider.as_str()) {
            for message in &mut conversation {
                message
                    .blocks
                    .retain(|block| !matches!(block, Block::Opaque { .. }));
            }

            conversation.retain(|message| !message.blocks.is_empty());
        }

        running.log.resume(path);

        self.title = first_instruction(path);
        self.conversation = conversation;
        self.interrupted = None;

        self.outbox.tell(ChatDown::Replaced { events });
        self.publish_state();
    }

    fn create_project(&self, name: &str) -> Result<PathBuf> {
        let name = name.trim();

        if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
            bail!("A project's name cannot be empty, start with a dot or hold a slash.");
        }

        let project = projects(&self.folder).join(name);

        std::fs::create_dir_all(&project)
            .with_context(|| format!("cannot create the folder {}", project.display()))?;

        Ok(project)
    }

    /// Tells the screens the chat, the projects and the settings.
    fn publish_state(&self) {
        let settings = lock(&self.settings).view();

        let log = self
            .running
            .as_ref()
            .and_then(|running| running.log.path())
            .map(|path| path.display().to_string());

        let chat = ChatInfo {
            id: CHAT.to_string(),
            title: self.title.clone().unwrap_or_else(|| "New chat".to_string()),
            place: self.place.display().to_string(),
            status: self.status.clone(),
            running: self.busy,
            log,
            settings,
        };

        self.outbox.mirror.publish(Down::State(HostState {
            chats: vec![chat],
            projects: list_projects(&self.folder),
        }));
    }
}

/// Where a run begins.
enum Start {
    Instruction(String, Vec<Image>),

    /// The step a provider failure stopped an earlier run on.
    CarryOn(Progress),
}

pub(super) fn build_provider(settings: &Settings) -> Result<Arc<dyn ModelProvider>> {
    let model = settings.model();

    if model.trim().is_empty() {
        bail!("No model is chosen. Pick one in Settings → Model.");
    }

    let provider: Arc<dyn ModelProvider> = match settings.provider.as_str() {
        "anthropic" => {
            let Some(key) = settings.key() else {
                bail!("No API key for Anthropic. Add one in Settings → Account.");
            };

            Arc::new(AnthropicProvider::new(AnthropicConfig::with_key(
                &model,
                &settings.effort,
                key,
            ))?)
        }

        "hosted" => {
            if settings.address.is_empty() {
                bail!("The hosted provider needs the address of its API. Add it in Settings → Account.");
            }

            Arc::new(OpenAiCompatProvider::new(
                OpenAiCompatConfig::hosted_with_key(
                    &model,
                    &settings.address,
                    settings.key().unwrap_or_default(),
                ),
            )?)
        }

        _ => {
            if cortex_providers::codex::sign_in_status().is_none() {
                bail!("Not signed in to ChatGPT yet.");
            }

            Arc::new(CodexProvider::new(CodexConfig::new(
                &model,
                &settings.effort,
            ))?)
        }
    };

    Ok(provider)
}

impl TrajectorySink for Outbox {
    fn record(&self, event: &Event) {
        self.tell(ChatDown::Event(event.clone()));
    }
}

impl StreamObserver for Outbox {
    fn started(&self) {
        self.tell(ChatDown::WritingStarted);
    }

    fn text(&self, piece: &str) {
        self.tell(ChatDown::Writing(piece.to_string()));
    }

    fn thinking(&self, piece: &str) {
        self.tell(ChatDown::Thinking(piece.to_string()));
    }

    fn calling(&self, tool: &str) {
        self.tell(ChatDown::Calling(tool.to_string()));
    }

    fn retrying(&self, error: &ProviderError, wait: Duration) {
        let reason = match error {
            ProviderError::Http { status: 429, .. } => "Rate limited by the provider".to_string(),
            ProviderError::Http { status, .. } => format!("The provider answered {status}"),
            ProviderError::Network(_) => "The connection failed".to_string(),
            _ => "The request failed".to_string(),
        };

        self.tell(ChatDown::Retrying {
            reason,
            wait_ms: wait.as_millis() as u64,
        });
    }
}

/// The questions tools are waiting on, by the number the screens know
/// each by.
#[derive(Default)]
struct Questions {
    waiting: Mutex<HashMap<u64, oneshot::Sender<Decision>>>,
    next: AtomicU64,
}

impl Questions {
    fn answer(&self, id: u64, decision: Decision) {
        // The tool is gone only if its run was dropped.
        if let Some(sender) = lock(&self.waiting).remove(&id) {
            let _ = sender.send(decision);
        }
    }

    /// The run is over: nothing is waiting for an answer any more.
    fn abandon(&self, outbox: &Outbox) {
        for (id, _) in lock(&self.waiting).drain() {
            outbox.tell(ChatDown::ApprovalClosed { id });
        }
    }
}

struct PhoneApprover {
    outbox: Outbox,
    questions: Arc<Questions>,
    approve_all: AtomicBool,
}

#[async_trait]
impl Approver for PhoneApprover {
    async fn approve(&self, request: &ApprovalRequest) -> bool {
        if self.approve_all.load(Ordering::Relaxed) {
            return true;
        }

        let (sender, answer) = oneshot::channel();
        let id = self.questions.next.fetch_add(1, Ordering::Relaxed) + 1;

        lock(&self.questions.waiting).insert(id, sender);

        self.outbox.tell(ChatDown::Approval {
            id,
            tool: request.tool.clone(),
            summary: request.summary.clone(),
        });

        // An app that closed without answering has not approved.
        let decision = answer.await.unwrap_or(Decision::Deny);

        self.outbox.tell(ChatDown::ApprovalClosed { id });

        match decision {
            Decision::Allow => true,

            Decision::AlwaysAllow => {
                self.approve_all.store(true, Ordering::Relaxed);

                true
            }

            Decision::Deny => false,
        }
    }
}

/// Writes each session to its own file, in the same place and format as
/// the desktop app and the command line. The file is created on the
/// session's first event, so a session in which nothing was asked leaves
/// no empty log.
struct SessionLog {
    directory: PathBuf,
    current: Mutex<Option<(PathBuf, JsonlSink)>>,

    /// Told the name of each session as it is begun or taken up.
    namer: Mutex<Option<Namer>>,
}

type Namer = Box<dyn Fn(&str) + Send + Sync>;

impl SessionLog {
    fn new(project: &Path) -> Self {
        Self {
            directory: project.join(TRAJECTORIES),
            current: Mutex::new(None),
            namer: Mutex::new(None),
        }
    }

    /// Who is told which session is being written. Told at once if one
    /// already is: the agent is rebuilt in the middle of sessions.
    fn name_with(&self, namer: Namer) {
        if let Some(path) = self.path() {
            namer(&stem(&path));
        }

        *lock(&self.namer) = Some(namer);
    }

    fn began(&self, path: &Path) {
        if let Some(namer) = &*lock(&self.namer) {
            namer(&stem(path));
        }
    }

    fn path(&self) -> Option<PathBuf> {
        lock(&self.current).as_ref().map(|(path, _)| path.clone())
    }

    /// Goes on writing to the log of an earlier session.
    fn resume(&self, path: &Path) {
        *lock(&self.current) = JsonlSink::append(path)
            .ok()
            .map(|sink| (path.to_path_buf(), sink));

        if self.path().is_some() {
            self.began(path);
        }
    }

    /// Ends the current file; the next event starts a new one.
    fn start_new_session(&self) {
        *lock(&self.current) = None;
    }
}

impl TrajectorySink for SessionLog {
    fn record(&self, event: &Event) {
        let mut current = lock(&self.current);

        if current.is_none() {
            let path = self
                .directory
                .join(format!("{}-phone.jsonl", unix_millis()));

            let created =
                std::fs::create_dir_all(&self.directory).and_then(|_| JsonlSink::create(&path));

            match created {
                Ok(sink) => {
                    self.began(&path);

                    *current = Some((path, sink));
                }
                Err(_) => return,
            }
        }

        if let Some((_, sink)) = current.as_ref() {
            sink.record(event);
        }
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Lets the growing network go over recent moments whenever nothing is
/// happening, for as long as anything still holds it.
async fn let_rest(living: Weak<Living>) {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;

        let Some(living) = living.upgrade() else {
            return;
        };

        living.rest();
    }
}

async fn report_memory(path: PathBuf, scope: String, outbox: Outbox) {
    let read = tokio::task::spawn_blocking(move || memory::graph(&path, &scope)).await;

    if let Ok(Ok(graph)) = read {
        outbox.tell(ChatDown::Memory(graph));
    }
}

/// Where the projects on this phone are kept.
pub fn projects(folder: &Path) -> PathBuf {
    folder.join("projects")
}

fn list_projects(folder: &Path) -> Vec<Project> {
    let root = projects(folder);

    let _ = std::fs::create_dir_all(root.join(SCRATCH));

    let mut listed: Vec<Project> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?.to_string();

            if !path.is_dir() || name.starts_with('.') {
                return None;
            }

            Some(Project {
                sessions: sessions(&path),
                path: path.display().to_string(),
                scratch: name == SCRATCH,
                name,
            })
        })
        .collect();

    listed.sort_by(|a, b| (a.scratch, &a.name).cmp(&(b.scratch, &b.name)));

    listed
}

/// The newest sessions in `project`, newest first. Logs that hold no run
/// are left out.
fn sessions(project: &Path) -> Vec<PastSession> {
    let mut logs: Vec<(u64, PathBuf)> = std::fs::read_dir(project.join(TRAJECTORIES))
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();

            let updated = std::fs::metadata(&path)
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis() as u64;

            Some((updated, path))
        })
        .collect();

    logs.sort_by(|a, b| b.cmp(a));

    logs.into_iter()
        .filter_map(|(updated_ms, path)| {
            Some(PastSession {
                title: first_instruction(&path)?,
                path: path.display().to_string(),
                updated_ms,
            })
        })
        .take(SESSIONS_LISTED)
        .collect()
}

fn first_instruction(path: &Path) -> Option<String> {
    events(path).find_map(|event| match event {
        Event::RunStarted { instruction, .. } => Some(titled(&instruction)),
        _ => None,
    })
}

/// Every event in a session's log, in order.
fn events(path: &Path) -> impl Iterator<Item = Event> {
    use std::io::BufRead;

    std::fs::File::open(path)
        .ok()
        .into_iter()
        .flat_map(|file| std::io::BufReader::new(file).lines())
        .filter_map(|line| serde_json::from_str(&line.ok()?).ok())
}

fn titled(instruction: &str) -> String {
    let line = instruction.lines().next().unwrap_or_default().trim();

    line.chars().take(80).collect()
}

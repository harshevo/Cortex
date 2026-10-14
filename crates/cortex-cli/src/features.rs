//! Optional capabilities, switched on with `--with`.
//!
//! Each one is off unless asked for, so that an evaluation run can turn on
//! exactly one and compare the result with the plain agent. `run`, `chat`
//! and the app start with [`DEFAULTS`] on top, which `--without` takes away.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use cortex_brain::{Controller, DirectEncoder, Intuition, Mind, StrategyBandit};
use cortex_core::{
    Adviser, CompactConfig, ContextSource, EffectLedger, EffectSink, ExperimentConfig, Extras,
    Ledger, LedgerSink, MaskConfig, PruneConfig, RolloutConfig, ToolRegistry, TrajectorySink,
    Verifier, Workspace, prompt,
};
use cortex_index::{CodeGraph, CodeIndex};
use cortex_living::{Counsel, Living};
use cortex_memory::{HopeClient, MemoryConfig, MemorySystem};
use cortex_retrieval::{EmbeddingClient, SemanticIndex};
use cortex_router::{ContextRouter, LikelyFiles, ProjectMap};
use cortex_skills::{SkillLibrary, SkillSource, UseSkill};
use cortex_tools::BrowserOptions;
use cortex_verify::ProjectVerifier;

use crate::args::{ModelArgs, ProviderKind};

#[derive(ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feature {
    /// Code index tools: `outline` and `read_symbol`.
    Index,

    /// Search code by meaning with EmbeddingGemma 2. Needs the embedding
    /// service from `ml/` to be running.
    Semantic,

    /// Remove tool output that a later, identical call has replaced: a
    /// file read again after an edit, a test command run again.
    Prune,

    /// Show the model a map of the project before it starts: every source
    /// file with its size and the names it defines.
    Map,

    /// Return less text from the basic tools: an outline for a whole long
    /// file, the changed lines after an edit, only the summary of a
    /// passing test run, and one line for a repeated read that has not
    /// changed.
    Lean,

    /// Ask the model to put independent lookups in one response.
    Batch,

    /// Code graph tools: `find_references` and `file_dependencies`.
    Graph,

    /// Memory of earlier tasks in the same repository, shown before each
    /// new task, plus the `remember` and `recall` tools.
    Memory,

    /// Run the project's checks before accepting that a task is done, and
    /// send failures back to the model.
    Gate,

    /// Let the HOPE-style memory model re-score recalled memories. Needs
    /// `--with memory` and the service from `ml/cortex_ml/hope`.
    Hope,

    /// A decision controller that learns, from runs whose checks passed
    /// or failed, which kind of next action pays off, and says so between
    /// steps. Learns only with `--with gate`.
    Controller,

    /// The dendritic network, allowed to speak. It grows a neuron for
    /// each thing it meets, and each neuron grows branches that learn
    /// which combinations of earlier events it follows. From that it
    /// expects what comes next, imagines a few steps on, and compares the
    /// ways forward by how they have turned out. When its own record
    /// says it is sure enough, it adds one line between steps: what has
    /// usually come next here and gone well, or that the usual next step
    /// has usually gone badly. It keeps count of whether its advice was
    /// followed by what it said and grows more cautious when it was not.
    /// One network for all projects, in `~/.cortex/living`, drawn live in
    /// the app. Whether its line helps a model is not measured.
    Brain,

    /// Gather context for the task before the model starts, choosing among
    /// the other capabilities that are switched on.
    Router,

    /// A second brain beside the controller: an associative network that
    /// learns which files, commands and errors went together in passing
    /// runs, and brings them to mind before a task and between steps. On a
    /// long run it also recalls a check left failing or edits left
    /// unchecked. Works with or without `--with brain`.
    Intuition,

    /// Replace tool output older than the last ten steps with one line,
    /// keeping the model's own reasoning and calls. For long runs.
    Mask,

    /// When a long conversation has filled most of the model's context
    /// window, have the model write a summary of the older part and put
    /// that in its place: what was asked, what was decided, which files
    /// changed and how, how the commands ended, what is still open. One
    /// extra model call each time. Acts at 80% of the window
    /// (`--compact-at`), and only for a model whose window is known or
    /// given with `--context-window`. A conversation can also be
    /// compacted by hand at any time, with or without this.
    Compact,

    /// When the model says it is done, hold its changes against the
    /// request and send back, once, whatever looks unmet. One extra model
    /// call per task.
    Review,

    /// Ask the model to reproduce a problem with a failing test or script
    /// before it changes any source file.
    Repro,

    /// After each task, have the model write down what a later task
    /// should know, from failed runs as well as passing ones, and show
    /// the lessons that fit before a new task. Needs `--with memory`. One
    /// extra model call per task.
    Lessons,

    /// Tell the model how often a first answer has passed the checks in
    /// this repository, so it can weigh the cost of testing. Needs
    /// `--with memory`, and speaks once five tasks have been checked.
    Priors,

    /// Show the files the task is probably about: those that use its
    /// words, and those closely tied to them by imports.
    Likely,

    /// The learning brain from the `brain` project, watching every step
    /// and listening to what you say. It learns which actions work here,
    /// keeps your corrections, and says a line when it has one: a habit
    /// it is sure of, something you said before, an action that failed
    /// here before, a choice worth asking you about, or something untried.
    /// The model can also have it imagine how actions would turn out. Its
    /// service is started when needed. On in `run`, `chat` and the app
    /// unless turned off; silent in a share of runs so that `cortex mind`
    /// can measure its effect. Without `--with gate` it learns from
    /// commands and from you, not from checks.
    Mind,

    /// Make up to `--rollouts` attempts, each in its own copy of the
    /// project and each told to go about the task differently, and keep
    /// the best. Stops at the first convincing attempt unless
    /// `--rollouts-parallel` is given.
    Rollouts,

    /// Let the model pick a skill itself: the list of skills goes in the
    /// system prompt and the `skill` tool loads one. Skills are written
    /// procedures such as `code-review`, `security-review` and
    /// `ui-design`; typing `/name` starts one with or without this.
    Skills,

    /// GitHub tools, through the `gh` command: `github` reads pull
    /// requests, diffs, checks, comments, issues and workflow logs, and
    /// `github_write` opens pull requests and posts comments and reviews,
    /// asking first each time.
    Github,

    /// A real web browser the agent can see and use: `browser` opens
    /// pages, clicks, types and runs scripts, asking first each time, and
    /// `browser_view` reads the page as text, takes screenshots and
    /// reports console errors and failed requests. For looking at an
    /// interface, testing it, and automating work on web pages. Needs
    /// Chrome, Chromium, Edge or Brave to be installed.
    Browser,

    /// The same dendritic network as `brain`, only watching: it grows,
    /// rewires, expects, imagines and rests, and nothing it learns
    /// reaches the model. For studying it before letting it speak.
    Living,
}

/// On in `run`, `chat` and the app unless turned off with `--without`.
pub const DEFAULTS: [Feature; 1] = [Feature::Mind];

impl crate::args::ModelArgs {
    /// Switches on the capabilities a session starts with, leaving out
    /// those turned off by name.
    pub fn with_defaults(mut self) -> Self {
        for feature in DEFAULTS {
            if !self.features.contains(&feature) {
                self.features.push(feature);
            }
        }

        let without = self.without.clone();

        self.features.retain(|feature| !without.contains(feature));

        self
    }
}

/// Where memory is kept and what it is about.
pub struct MemoryLocation {
    /// The SQLite file.
    pub path: PathBuf,

    /// Separates repositories that share one file.
    pub scope: String,

    /// Leave out earlier attempts at the very same instruction.
    pub skip_identical: bool,
}

/// What the chosen features add up to.
pub struct Setup {
    pub tools: ToolRegistry,
    pub prune: Option<PruneConfig>,

    /// Consulted before each instruction.
    pub context_sources: Vec<Arc<dyn ContextSource>>,

    /// Receive every event of a run, besides the log file.
    pub sinks: Vec<Arc<dyn TrajectorySink>>,

    pub verifier: Option<Arc<dyn Verifier>>,

    /// May add a note after each step's tool results.
    pub advisers: Vec<Arc<dyn Adviser>>,

    /// Paragraphs added to the system prompt.
    pub prompt_notes: Vec<String>,

    /// The index behind semantic search, for a caller that wants to warm
    /// it ahead of the first task.
    pub semantic: Option<Arc<SemanticIndex>>,

    /// Answer an unchanged repeated read with one line.
    pub fold_repeats: bool,

    /// Masking, the review, lessons and repeated attempts.
    pub extras: Extras,

    /// The model has been told which skills there are.
    pub skill_listing: bool,

    /// The mind, when it is on, for a caller that wants its guess at
    /// what the person will ask for next.
    pub mind: Option<Arc<Mind>>,

    /// The dendritic network, when it is on, for a caller that wants to
    /// draw it or its guess at what will be asked next.
    pub living: Option<Arc<Living>>,

    /// Records the tokens of every model call, for a caller that wants
    /// to name the session or read the totals. It is among `sinks`
    /// already. `None` with `--no-records`, or without a home directory.
    pub usage: Option<Arc<LedgerSink>>,

    /// Records how every run went and which advisers were held out of
    /// it. It is among `sinks` already.
    pub effect: Option<Arc<EffectSink>>,
}

impl Setup {
    /// Tells the records which session what follows belongs to: the stem
    /// of the session's log file. Call it when a session's log is
    /// created and when an earlier session is taken up.
    pub fn name_session(&self, session: &str) {
        if let Some(usage) = &self.usage {
            usage.set_session(session);
        }

        if let Some(effect) = &self.effect {
            effect.set_session(session);
        }
    }

    /// Everything to add to the system prompt for an agent that works in
    /// `workspace`: the capabilities' own notes, the checks the work will
    /// answer to, and the skills the project brings.
    pub fn notes(&self, workspace: &Workspace) -> Vec<String> {
        let mut notes = self.prompt_notes.clone();

        notes.extend(
            self.verifier
                .as_ref()
                .and_then(|verifier| prompt::verification_note(&verifier.commands(workspace))),
        );

        if self.skill_listing {
            notes.extend(SkillLibrary::for_project(workspace.root()).project_listing());
        }

        notes
    }
}

pub async fn setup(args: &ModelArgs, memory_location: MemoryLocation) -> Result<Setup> {
    let has =
        |feature: Feature| args.features.contains(&feature) && !args.without.contains(&feature);

    let mut tools = cortex_tools::default_registry();

    if has(Feature::Lean) {
        cortex_tools::add_lean_tools(&mut tools);
    }

    if has(Feature::Index) {
        cortex_tools::add_code_index_tools(&mut tools);
    }

    let client = EmbeddingClient::new(&args.embedding_url);

    // A packaged program carries the embedding service, so memory always
    // gets it. From a checkout it is used when someone has started it.
    if cortex_core::services::packaged().is_some()
        && let Err(error) = client.wake().await
    {
        eprintln!("warning: {error}");
    }

    let semantic = if has(Feature::Semantic) {
        let index = semantic_index(&client).await?;

        cortex_tools::add_semantic_search(&mut tools, index.clone());

        Some(index)
    } else {
        None
    };

    let graph = if has(Feature::Graph) {
        let graph = Arc::new(CodeGraph::new(Arc::new(CodeIndex::new())));

        cortex_tools::add_code_graph_tools(&mut tools, graph.clone());

        Some(graph)
    } else {
        None
    };

    if has(Feature::Hope) && !has(Feature::Memory) {
        bail!("`--with hope` re-scores memories, so it needs `--with memory` as well");
    }

    if has(Feature::Lessons) && !has(Feature::Memory) {
        bail!(
            "`--with lessons` keeps what it learns in memory, so it needs `--with memory` as well"
        );
    }

    if has(Feature::Priors) && !has(Feature::Memory) {
        bail!(
            "`--with priors` reads how earlier tasks went from memory, so it needs `--with memory` as well"
        );
    }

    let extras = extras(args, &memory_location)?;

    let records = records_folder(args);

    let usage = records
        .as_ref()
        .map(|folder| Ledger::open(folder.join("usage")))
        .map(|ledger| Arc::new(LedgerSink::new(ledger, &memory_location.scope)));

    let effect = records
        .as_ref()
        .map(|folder| EffectLedger::open(folder.join("effect")))
        .map(|ledger| Arc::new(EffectSink::new(ledger, &memory_location.scope)));

    let controller = controller(args, &memory_location).await?;
    let intuition = intuition(args, &memory_location);
    let mind = mind(args, &memory_location).await;

    let memory = if has(Feature::Memory) {
        let memory = open_memory(
            memory_location,
            &client,
            hope_client(args).await?,
            has(Feature::Priors),
            &args.memory_from,
        )
        .await?;

        cortex_tools::add_memory_tools(&mut tools, memory.clone());

        Some(memory)
    } else {
        None
    };

    let verifier: Option<Arc<dyn Verifier>> = if has(Feature::Gate) {
        Some(Arc::new(ProjectVerifier::with_commands(
            args.checks.clone(),
        )))
    } else {
        None
    };

    let mut sinks: Vec<Arc<dyn TrajectorySink>> = Vec::new();

    if let Some(usage) = &usage {
        sinks.push(usage.clone());
    }

    if let Some(effect) = &effect {
        sinks.push(effect.clone());
    }

    if let Some(memory) = &memory {
        sinks.push(memory.clone());
    }

    let living = (has(Feature::Living) || has(Feature::Brain))
        .then(living)
        .flatten();

    if let Some(living) = &living {
        sinks.push(living.clone());
    }

    let mut advisers: Vec<Arc<dyn Adviser>> = Vec::new();

    if let Some(living) = &living
        && has(Feature::Brain)
    {
        advisers.push(Arc::new(Counsel(living.clone())));
    }

    if let Some(controller) = controller {
        sinks.push(controller.clone());
        advisers.push(controller);
    }

    if let Some(intuition) = &intuition {
        sinks.push(intuition.clone());
        advisers.push(intuition.clone());
    }
    let mind_listens = mind.is_some();
    let suggester = mind.clone();

    if let Some(mind) = &mind {
        sinks.push(mind.clone());
        advisers.push(mind.clone());
    }
    if !advisers.is_empty() {
        tools.register(cortex_tools::BrainState(advisers.clone()));
    }

    let mut context_sources = Vec::new();

    // The map is the same for every task in a project, so it goes first:
    // text that does not change is what a prompt cache can reuse.
    if has(Feature::Map) {
        let map: Arc<dyn ContextSource> = Arc::new(ProjectMap::new(Arc::new(CodeIndex::new())));

        context_sources.push(map);
    }

    context_sources.extend(task_context_sources(
        has(Feature::Router),
        memory,
        graph.clone(),
        semantic.clone(),
        &verifier,
    ));

    if has(Feature::Likely) {
        let graph = graph.unwrap_or_else(|| Arc::new(CodeGraph::new(Arc::new(CodeIndex::new()))));

        context_sources.push(Arc::new(LikelyFiles::new(graph)));
    }

    if let Some(mind) = mind {
        context_sources.push(mind);
    }

    if let Some(intuition) = intuition {
        context_sources.push(intuition);
    }

    // Last, so that it reads straight before the task. A task that does
    // not begin with `/name` gets nothing from it, so it is always there.
    context_sources.push(Arc::new(SkillSource));

    let mut prompt_notes = Vec::new();

    if has(Feature::Batch) {
        prompt_notes.push(prompt::batching_note());
    }

    if has(Feature::Repro) {
        prompt_notes.push(prompt::reproduction_note());
    }

    if has(Feature::Skills) {
        tools.register(UseSkill);

        prompt_notes.extend(SkillLibrary::standard().listing());
    }

    if has(Feature::Github) {
        cortex_tools::add_github_tools(&mut tools);

        prompt_notes.push(prompt::github_note());
    }

    if has(Feature::Browser) {
        cortex_tools::add_browser_tools(
            &mut tools,
            BrowserOptions {
                show_window: args.browser_window,
                pictures: args.browser_pictures,
            },
        );

        prompt_notes.push(prompt::browser_note(args.browser_pictures));
    }

    if mind_listens {
        prompt_notes.push(cortex_brain::mind::PROMPT_NOTE.to_string());
    }

    Ok(Setup {
        skill_listing: has(Feature::Skills),
        mind: suggester,
        living,
        usage,
        effect,
        semantic,
        tools,
        prune: prune_config(args)?,
        context_sources,
        sinks,
        verifier,
        advisers,
        prompt_notes,
        fold_repeats: has(Feature::Lean),
        extras,
    })
}

/// The parts of the loop itself that the chosen features switch on.
fn extras(args: &ModelArgs, memory_location: &MemoryLocation) -> Result<Extras> {
    let has = |feature: Feature| args.features.contains(&feature);

    // Masking rewrites earlier messages, as pruning does, and the
    // Anthropic API rejects edited history for the same reason.
    if has(Feature::Mask) && args.provider == ProviderKind::Anthropic {
        bail!("`--with mask` cannot be used with the Anthropic provider");
    }

    let rollouts = if has(Feature::Rollouts) {
        if args.rollouts < 2 {
            bail!("`--rollouts` must be at least 2 for `--with rollouts` to do anything");
        }

        let file = strategies_file(&memory_location.path);

        let bandit = StrategyBandit::new(Some(file.clone()));

        let tried: u32 = bandit.records().values().map(|record| record.tried()).sum();

        eprintln!(
            "rollouts: up to {} attempts, {}; strategies chosen from {tried} earlier attempts ({})",
            args.rollouts,
            if args.rollouts_parallel {
                "made at the same time"
            } else {
                "stopping at the first convincing one"
            },
            file.display()
        );

        Some(RolloutConfig {
            count: args.rollouts,
            parallel: args.rollouts_parallel,
            policy: Arc::new(bandit),
        })
    } else {
        None
    };

    if has(Feature::Compact) && !(args.compact_at > 0.0 && args.compact_at <= 1.0) {
        bail!("`--compact-at` is a share of the context window, above 0 and at most 1");
    }

    // Compacting rewrites earlier messages too, but with the Anthropic
    // provider it replaces all of them and keeps none, which that API
    // accepts. So it is not refused there as masking and pruning are.
    let compact = has(Feature::Compact).then(|| {
        let provider = match args.provider {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Codex => "codex",
            ProviderKind::Local | ProviderKind::Hosted => "openai-compatible",
        };

        CompactConfig {
            keep_recent: args.compact_keep,
            auto_at: Some(args.compact_at),
        }
        .suited_to(provider)
    });

    let experiment =
        (args.holdout > 0.0 || args.holdout_briefings > 0.0).then(|| ExperimentConfig {
            share: args.holdout,
            briefing_share: args.holdout_briefings,
            ..ExperimentConfig::default()
        });

    Ok(Extras {
        mask: has(Feature::Mask).then(MaskConfig::default),
        review: has(Feature::Review),
        reflect: has(Feature::Lessons),
        rollouts,
        compact,
        context_window: args.context_window,
        experiment,
    })
}

/// The folder the records of tokens used and of how runs went are kept
/// under: the one given, or `~/.cortex`, where the dendritic network is
/// kept as well. `None` when no records are wanted.
pub fn records_folder(args: &ModelArgs) -> Option<PathBuf> {
    if args.no_records {
        return None;
    }

    args.records_dir.clone().or_else(default_records_folder)
}

/// `~/.cortex`. `None` on a machine without a home directory.
pub fn default_records_folder() -> Option<PathBuf> {
    Some(std::env::home_dir()?.join(".cortex"))
}

/// Where the record of which attempt strategies worked is kept, for the
/// memory at `memory`.
pub fn strategies_file(memory: &std::path::Path) -> PathBuf {
    memory.with_extension("strategies.json")
}

/// The decision controller, when one is asked for. What it learns is kept
/// beside the memory file.
async fn controller(
    args: &ModelArgs,
    memory_location: &MemoryLocation,
) -> Result<Option<Arc<Controller>>> {
    let has = |feature: Feature| args.features.contains(&feature);

    if !has(Feature::Controller) {
        return Ok(None);
    }

    if !has(Feature::Gate) {
        eprintln!(
            "controller: without `--with gate` no run has a checked result, so it will not learn"
        );
    }

    let file = memory_location.path.with_extension("controller.json");

    let controller = Controller::new(Box::new(DirectEncoder), Some(file.clone()));

    eprintln!(
        "controller: learned from {} checked runs so far ({})",
        controller.runs(),
        file.display()
    );

    Ok(Some(Arc::new(controller)))
}

/// The associative network, when it is asked for. What it learns is kept
/// beside the memory file.
fn intuition(args: &ModelArgs, memory_location: &MemoryLocation) -> Option<Arc<Intuition>> {
    if !args.features.contains(&Feature::Intuition) {
        return None;
    }

    let file = intuition_file(&memory_location.path);

    let intuition = Intuition::new(Some(file.clone()));

    eprintln!(
        "intuition: {} neurons, {} synapses, learned from {} runs so far ({})",
        intuition.neurons(),
        intuition.synapses(),
        intuition.runs(),
        file.display()
    );

    Some(Arc::new(intuition))
}

/// The learning brain, when it is asked for. Its service is started if it
/// is not running; one that cannot be started switches the capability off
/// with a note, and never stops the agent from starting.
async fn mind(args: &ModelArgs, memory_location: &MemoryLocation) -> Option<Arc<Mind>> {
    if !args.features.contains(&Feature::Mind) || args.without.contains(&Feature::Mind) {
        return None;
    }

    // The mind can keep itself silent in a share of runs. When the
    // engine holds advisers out, the mind among them, that is the
    // experiment, and the mind's own is switched off: two draws would
    // silence it more often than either says, and only the engine's is
    // recorded with the run.
    let own_control = if args.holdout > 0.0 {
        0.0
    } else {
        args.mind_control
    };

    let mind = Mind::new(&args.mind_url, &memory_location.scope).with_control(own_control);

    let mind = Arc::new(mind);
    let waking = mind.clone();

    // Starting the service can take seconds the first time.
    let woken = tokio::task::spawn_blocking(move || waking.wake()).await;

    let health = match woken.unwrap_or_else(|error| Err(error.to_string())) {
        Ok(health) => health,

        Err(reason) => {
            eprintln!(
                "mind: off for this session: {reason}. `--without mind` leaves it out without \
                 this note"
            );

            return None;
        }
    };

    eprintln!(
        "mind: {} steps remembered, {} rules, {} things you said ({})",
        health["remembered"],
        health["rules"],
        health["told"].as_u64().unwrap_or_default(),
        args.mind_url
    );

    Some(mind)
}

/// The dendritic network. It is one for all projects, and shared by
/// everything in this process.
fn living() -> Option<Arc<Living>> {
    let file = std::env::home_dir()?
        .join(".cortex")
        .join("living")
        .join("network.json");

    let living = Living::shared(&file);
    let meters = living.snapshot().meters;

    eprintln!(
        "network: {} neurons, {} branches, {} synapses, {} moments lived ({})",
        meters.neurons,
        meters.segments,
        meters.synapses,
        meters.moments,
        file.display()
    );

    Some(living)
}

/// Where the associative network for the memory at `memory` is kept.
pub fn intuition_file(memory: &std::path::Path) -> PathBuf {
    memory.with_extension("intuition.json")
}

/// With the router on, it alone decides what is gathered. Without it,
/// memory is the only source and is always shown.
fn task_context_sources(
    use_router: bool,
    memory: Option<Arc<MemorySystem>>,
    graph: Option<Arc<CodeGraph>>,
    semantic: Option<Arc<SemanticIndex>>,
    verifier: &Option<Arc<dyn Verifier>>,
) -> Vec<Arc<dyn ContextSource>> {
    if !use_router {
        return memory
            .map(|memory| memory as Arc<dyn ContextSource>)
            .into_iter()
            .collect();
    }

    let mut router = ContextRouter::new();

    if let Some(memory) = memory {
        router = router.with_memory(memory);
    }

    if let Some(graph) = graph {
        router = router.with_graph(graph);
    }

    if let Some(semantic) = semantic {
        router = router.with_semantic(semantic);
    }

    if let Some(verifier) = verifier {
        router = router.with_verifier(verifier.clone());
    }

    vec![Arc::new(router)]
}

async fn semantic_index(client: &EmbeddingClient) -> Result<Arc<SemanticIndex>> {
    // Started here when it is not running. Failing now, with the reason,
    // is better than failing on the agent's first search.
    let health = client.wake().await?;

    eprintln!(
        "semantic search: {} ({} dimensions, {})",
        health.model, health.dimension, health.device
    );

    Ok(cortex_tools::semantic_index(client.clone()))
}

/// Memory compares tasks with the embedding model when the service is
/// running, and by shared words when it is not.
/// The learned reranker, when `--with hope` asks for it.
async fn hope_client(args: &ModelArgs) -> Result<Option<HopeClient>> {
    if !args.features.contains(&Feature::Hope) {
        return Ok(None);
    }

    let client = HopeClient::new(&args.hope_url);

    // Started here when it is not running, as the mind is.
    let health = match client.wake().await {
        Ok(health) => health,

        Err(reason) => bail!(
            "the HOPE memory service at {} could not be reached: {reason}",
            args.hope_url
        ),
    };

    let state = if health.trained {
        "trained"
    } else {
        "untrained, so it ranks like plain similarity"
    };

    eprintln!("hope: re-scoring memories ({state})");

    Ok(Some(client))
}

async fn open_memory(
    location: MemoryLocation,
    client: &EmbeddingClient,
    hope: Option<HopeClient>,
    show_track_record: bool,
    others: &[PathBuf],
) -> Result<Arc<MemorySystem>> {
    let config = MemoryConfig {
        skip_identical: location.skip_identical,
        show_track_record,
        ..MemoryConfig::for_scope(location.scope)
    };

    let mut memory = MemorySystem::open(&location.path, config)
        .with_context(|| format!("cannot open the memory at {}", location.path.display()))?;

    let episodes = memory.store().episodes(memory.scope())?.len();

    match client.health().await {
        Ok(health) => {
            eprintln!(
                "memory: {episodes} earlier tasks in {}, compared with {}",
                location.path.display(),
                health.model
            );

            memory = memory.with_embedder(Arc::new(client.clone()));
        }

        Err(_) => {
            eprintln!(
                "memory: {episodes} earlier tasks in {}, compared by shared words (the \
                 embedding service is not running)",
                location.path.display()
            );
        }
    }

    if let Some(hope) = hope {
        memory = memory.with_reranker(Arc::new(hope));
    }

    for folder in others {
        // A project is known to its memory by its real path. One that has
        // no memory yet has nothing to add.
        let Ok(folder) = folder.canonicalize() else {
            continue;
        };

        let file = folder.join(".cortex").join("memory.db");

        if !file.is_file() {
            continue;
        }

        let name = folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.display().to_string());

        match cortex_memory::Store::open(&file) {
            Ok(store) => memory = memory.with_other(store, &folder.display().to_string(), &name),
            Err(error) => eprintln!("memory: {} is left out: {error}", file.display()),
        }
    }

    if memory.others() > 0 {
        eprintln!(
            "memory: also recalling from {} other projects",
            memory.others()
        );
    }

    Ok(Arc::new(memory))
}

fn prune_config(args: &ModelArgs) -> Result<Option<PruneConfig>> {
    if !args.features.contains(&Feature::Prune) {
        return Ok(None);
    }

    // Pruning rewrites earlier messages. The Anthropic API ties a model's
    // reasoning to the exact conversation that produced it and rejects
    // edited history.
    if args.provider == ProviderKind::Anthropic {
        bail!("`--with prune` cannot be used with the Anthropic provider");
    }

    Ok(Some(PruneConfig::default()))
}

//! Optional capabilities, switched on with `--with`.
//!
//! Each one is off unless asked for, so that an evaluation run can turn on
//! exactly one and compare the result with the plain agent. `run`, `chat`
//! and the app start with [`DEFAULTS`] on top, which `--without` takes away.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use clap::ValueEnum;
use cortex_core::budget::Budget;
use cortex_core::learn::PlaybookSource;
use cortex_core::ledger::PriceTable;
use cortex_core::notes::{self, EpisodeSink, NotesSource};
use cortex_core::{
    Adviser, CommandHooks, CompactConfig, ContextSource, EffectLedger, EffectSink,
    ExperimentConfig, Extras, Ledger, LedgerSink, MaskConfig, PruneConfig, RolloutConfig,
    StuckDetector, ToolRegistry, TrajectorySink, Verifier, Workspace, plan, prompt, rules,
};
use cortex_index::{CodeGraph, CodeIndex};
use cortex_retrieval::{EmbeddingClient, SemanticIndex};
use cortex_router::{ContextRouter, LikelyFiles, ProjectMap};
use cortex_skills::{SkillLibrary, SkillSource, UseSkill};
use cortex_tools::BrowserOptions;
use cortex_verify::ProjectVerifier;

use crate::args::{ModelArgs, ProviderKind};
use crate::strategies::StrategyBandit;

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

    /// Run the project's checks before accepting that a task is done, and
    /// send failures back to the model.
    Gate,

    /// Gather context for the task before the model starts, choosing among
    /// the other capabilities that are switched on.
    Router,

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

    /// Show the files the task is probably about: those that use its
    /// words, and those closely tied to them by imports.
    Likely,

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
}

/// On in `run`, `chat` and the app unless turned off with `--without`.
pub const DEFAULTS: [Feature; 0] = [];

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

/// Project identity and folder for harness records.
pub struct SessionLocation {
    pub directory: PathBuf,
    pub scope: String,

    /// The project the agent works in, for what belongs to it: its hooks,
    /// the record of its runs, and the MCP servers started for it. `None`
    /// for an evaluation, which works in copies.
    pub project: Option<PathBuf>,
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

    /// Records the tokens of every model call, for a caller that wants
    /// to name the session or read the totals. It is among `sinks`
    /// already. `None` with `--no-records`, or without a home directory.
    pub usage: Option<Arc<LedgerSink>>,

    /// Records how every run went and which advisers were held out of
    /// it. It is among `sinks` already.
    pub effect: Option<Arc<EffectSink>>,

    /// Commands run before and after tool calls, when any are set.
    pub hooks: Option<Arc<CommandHooks>>,

    /// What starting the extras came to (hooks, MCP servers), a line
    /// each, to tell the person.
    pub report: Vec<String>,

    /// Says when a run is stuck; among `sinks` and `advisers` already.
    /// The agent is given it too, to escalate or stop.
    pub recovery: Option<Arc<StuckDetector>>,
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

        let home = std::env::home_dir();

        notes.extend(rules::note(&rules::load(workspace.root(), home.as_deref())));

        notes
    }
}

pub async fn setup(args: &ModelArgs, location: SessionLocation) -> Result<Setup> {
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

    let extras = extras(args, &location)?;

    let records = records_folder(args);

    let usage = records
        .as_ref()
        .map(|folder| Ledger::open(folder.join("usage")))
        .map(|ledger| Arc::new(LedgerSink::new(ledger, &location.scope)));

    let effect = records
        .as_ref()
        .map(|folder| EffectLedger::open(folder.join("effect")))
        .map(|ledger| Arc::new(EffectSink::new(ledger, &location.scope)));

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

    let mut context_sources = Vec::new();

    // The map is the same for every task in a project, so it goes first:
    // text that does not change is what a prompt cache can reuse.
    if has(Feature::Map) {
        let map: Arc<dyn ContextSource> = Arc::new(ProjectMap::new(Arc::new(CodeIndex::new())));

        context_sources.push(map);
    }

    context_sources.extend(task_context_sources(
        has(Feature::Router),
        graph.clone(),
        semantic.clone(),
        &verifier,
    ));

    if has(Feature::Likely) {
        let graph = graph.unwrap_or_else(|| Arc::new(CodeGraph::new(Arc::new(CodeIndex::new()))));

        context_sources.push(Arc::new(LikelyFiles::new(graph)));
    }

    // What earlier sessions noted about the project.
    context_sources.push(Arc::new(NotesSource));

    // How tasks like this one have gone here, and what they took.
    context_sources.push(Arc::new(PlaybookSource));

    // Last, so that it reads straight before the task. A task that does
    // not begin with `/name` gets nothing from it, so it is always there.
    context_sources.push(Arc::new(SkillSource));

    let mut report = Vec::new();
    let mut hooks = None;

    if let Some(project) = &location.project {
        sinks.push(Arc::new(EpisodeSink::new(project)));

        let home = std::env::home_dir();

        let loaded = CommandHooks::load(project, home.as_deref());

        if !loaded.is_empty() {
            report.push(format!(
                "hooks: {} from {}",
                loaded.count(),
                loaded.sources.join(", ")
            ));

            hooks = Some(Arc::new(loaded));
        }

        if !args.no_mcp
            && let Some(home) = &home
        {
            report.extend(cortex_tools::add_mcp_tools(&mut tools, home, project).await);
        }
    }

    // Notices a run going round in circles. It hears the run as a sink
    // and speaks as an adviser.
    let stuck = Arc::new(StuckDetector::new());

    sinks.push(stuck.clone());

    let advisers: Vec<Arc<dyn Adviser>> = vec![stuck.clone()];

    let mut prompt_notes = vec![plan::note(), notes::note()];

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

    Ok(Setup {
        recovery: Some(stuck),
        hooks,
        report,
        skill_listing: has(Feature::Skills),
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
fn extras(args: &ModelArgs, location: &SessionLocation) -> Result<Extras> {
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

        let file = location.directory.join("strategies.json");

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
        reflect: false,
        rollouts,
        compact,
        context_window: args.context_window,
        experiment,
        checkpoints: !args.no_checkpoints,
        budget: budget(args),
    })
}

/// The run's budget, when any limit is given.
fn budget(args: &ModelArgs) -> Option<Budget> {
    let budget = Budget {
        max_tokens: args.max_tokens.filter(|max| *max > 0),
        max_cost: args.max_cost.filter(|max| *max > 0.0),
        max_duration: args
            .max_minutes
            .filter(|max| *max > 0.0)
            .map(|minutes| std::time::Duration::from_secs_f64(minutes * 60.0)),
        prices: PriceTable::at_home(),
    };

    budget.is_set().then_some(budget)
}

/// The folder the records of tokens used and of how runs went are kept
/// under: the one given, or `~/.cortex`. `None` when no records are wanted.
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

/// Gather code context when the router is enabled.
fn task_context_sources(
    use_router: bool,
    graph: Option<Arc<CodeGraph>>,
    semantic: Option<Arc<SemanticIndex>>,
    verifier: &Option<Arc<dyn Verifier>>,
) -> Vec<Arc<dyn ContextSource>> {
    if !use_router {
        return Vec::new();
    }

    let mut router = ContextRouter::new();

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

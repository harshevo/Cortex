//! Optional capabilities, switched on with `--with`.
//!
//! Each one is off by default so that an evaluation run can turn on exactly
//! one and compare the result with the plain agent.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use cortex_brain::{
    Brain, BrainConfig, Controller, DirectEncoder, Encoder, FlyBrainEncoder, Intuition,
    StrategyBandit,
};
use cortex_core::{
    Adviser, ContextSource, Extras, MaskConfig, PruneConfig, RolloutConfig, ToolRegistry,
    TrajectorySink, Verifier, prompt,
};
use cortex_index::{CodeGraph, CodeIndex};
use cortex_memory::{HopeClient, MemoryConfig, MemorySystem};
use cortex_retrieval::{EmbeddingClient, SemanticIndex};
use cortex_router::{ContextRouter, LikelyFiles, ProjectMap};
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
    /// or failed, which next action pays off, and says so between steps.
    /// This is the plain baseline. Learns only with `--with gate`.
    Controller,

    /// The same controller, with the situation first passed through a
    /// spiking network the size of the fruit fly brain (139,255 neurons,
    /// about 50 million synapses, 250 MB of memory).
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

    /// Make up to `--rollouts` attempts, each in its own copy of the
    /// project and each told to go about the task differently, and keep
    /// the best. Stops at the first convincing attempt unless
    /// `--rollouts-parallel` is given.
    Rollouts,
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
}

pub async fn setup(args: &ModelArgs, memory_location: MemoryLocation) -> Result<Setup> {
    let has = |feature: Feature| args.features.contains(&feature);

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

    let controller = controller(args, &memory_location).await?;
    let intuition = intuition(args, &memory_location);

    let memory = if has(Feature::Memory) {
        let memory = open_memory(
            memory_location,
            &client,
            hope_client(args).await?,
            has(Feature::Priors),
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

    if let Some(memory) = &memory {
        sinks.push(memory.clone());
    }

    let mut advisers: Vec<Arc<dyn Adviser>> = Vec::new();

    if let Some(controller) = controller {
        sinks.push(controller.clone());
        advisers.push(controller);
    }

    if let Some(intuition) = &intuition {
        sinks.push(intuition.clone());
        advisers.push(intuition.clone());
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

    // Last, so that it reads straight before the task.
    if let Some(intuition) = intuition {
        context_sources.push(intuition);
    }

    let mut prompt_notes = Vec::new();

    if has(Feature::Batch) {
        prompt_notes.push(prompt::batching_note());
    }

    if has(Feature::Repro) {
        prompt_notes.push(prompt::reproduction_note());
    }

    Ok(Setup {
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

    Ok(Extras {
        mask: has(Feature::Mask).then(MaskConfig::default),
        review: has(Feature::Review),
        reflect: has(Feature::Lessons),
        rollouts,
    })
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

    if has(Feature::Controller) && has(Feature::Brain) {
        bail!("`--with controller` and `--with brain` are two versions of one thing; pick one");
    }

    if !has(Feature::Controller) && !has(Feature::Brain) {
        return Ok(None);
    }

    if !has(Feature::Gate) {
        eprintln!(
            "controller: without `--with gate` no run has a checked result, so it will not learn"
        );
    }

    let (encoder, extension): (Box<dyn Encoder>, &str) = if has(Feature::Brain) {
        (
            Box::new(FlyBrainEncoder::new({
                let owned = args.clone();
                tokio::task::spawn_blocking(move || fly_brain(&owned)).await??
            })),
            "brain.json",
        )
    } else {
        (Box::new(DirectEncoder), "controller.json")
    };

    let file = memory_location.path.with_extension(extension);

    let controller = Controller::new(encoder, Some(file.clone()));

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

/// Where the associative network for the memory at `memory` is kept.
pub fn intuition_file(memory: &std::path::Path) -> PathBuf {
    memory.with_extension("intuition.json")
}

/// Builds the spiking network: from a measured connectome when one is
/// given, otherwise generated at the fly's scale.
fn fly_brain(args: &ModelArgs) -> Result<Brain> {
    let started = Instant::now();

    let mut brain = match &args.connectome {
        Some(path) => Brain::from_file(BrainConfig::measured(), path)
            .with_context(|| format!("cannot read the connectome {}", path.display()))?,

        None => Brain::generate(BrainConfig::fly()),
    };

    let wiring = if args.connectome.is_some() {
        "measured wiring"
    } else {
        "generated wiring"
    };

    let device = std::env::var("CORTEX_BRAIN_DEVICE").unwrap_or_else(|_| "auto".into());
    match device.as_str() {
        "cpu" => {}
        "auto" if !cfg!(target_os = "macos") => {}
        "auto" | "gpu" => {
            if let Err(error) = brain.enable_gpu() {
                if device == "gpu" {
                    bail!("GPU simulation requested but unavailable: {error}");
                }
                eprintln!("brain: Metal unavailable, using CPU: {error}");
            }
        }
        _ => bail!("CORTEX_BRAIN_DEVICE must be auto, gpu, or cpu"),
    }
    eprintln!(
        "brain: {} neurons, {} connections, {wiring}, {} MB, built in {} ms",
        brain.config().neurons,
        brain.connectome().synapses(),
        brain.connectome().memory_bytes() / 1_000_000,
        started.elapsed().as_millis()
    );

    Ok(brain)
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
    // Fail now, with instructions, rather than on the agent's first search.
    let health = client.health().await?;

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

    let Some(health) = client.health().await else {
        bail!(
            "the HOPE memory service at {} is not reachable. Start it with \
             `uv run --project ml python -m cortex_ml.hope.server`",
            args.hope_url
        );
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

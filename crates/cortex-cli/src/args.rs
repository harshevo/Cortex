//! Command-line arguments.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::features::Feature;

#[derive(Parser)]
#[command(name = "cortex", version, about = "An experimental coding agent.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Carry out one instruction and exit.
    Run(RunArgs),

    /// Start an interactive session.
    Chat(ChatArgs),

    /// Search a project's code by meaning, without a model.
    Search(SearchArgs),

    /// Show what memory and the router would put in front of an
    /// instruction, without calling a model.
    Context(ContextArgs),

    /// Get a project ready ahead of any task: build the code graph, embed
    /// the code, and have the model write a short description of the
    /// project that every later task is shown.
    Prepare(PrepareArgs),

    /// Measure the agent on a suite of tasks.
    #[command(subcommand)]
    Eval(EvalCommand),

    /// Sign in to ChatGPT in the browser, to use a subscription's models
    /// with `--provider codex`.
    Login,
}

/// Where the model runs.
#[derive(ValueEnum, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// The Anthropic API. Needs `ANTHROPIC_API_KEY`.
    Anthropic,

    /// A model served on this machine through an OpenAI-compatible API,
    /// such as Ollama.
    Local,

    /// A hosted service with an OpenAI-compatible API. Needs `--base-url`,
    /// `--model`, and the key in `CORTEX_API_KEY`.
    Hosted,

    /// The models of a ChatGPT subscription, through the Codex service.
    /// Needs `cortex login` once.
    Codex,
}

impl ProviderKind {
    pub fn default_model(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("claude-opus-5-5"),
            Self::Local => Some("qwen2.5-coder:7b"),
            Self::Hosted => None,
            Self::Codex => Some("gpt-6.1-sol"),
        }
    }
}

/// Which model to use and how hard it should work.
#[derive(Args, Clone)]
pub struct ModelArgs {
    #[arg(long, value_enum, default_value = "anthropic")]
    pub provider: ProviderKind,

    /// Model name. Defaults to claude-opus-5-5, qwen2.5-coder:7b with
    /// `--provider local`, or gpt-6.1-sol with `--provider codex`.
    #[arg(long)]
    pub model: Option<String>,

    /// Address of the server's API, for `--provider local` or `hosted`.
    #[arg(long)]
    pub base_url: Option<String>,

    /// Reasoning depth: low, medium, high, xhigh, or max (Anthropic and
    /// Codex only; Codex treats max as xhigh).
    #[arg(long, default_value = "high")]
    pub effort: String,

    /// Turn on an optional capability. Repeat to turn on several.
    #[arg(long = "with", value_enum, value_name = "FEATURE")]
    pub features: Vec<Feature>,

    /// Address of the embedding service used by `--with semantic`.
    #[arg(long, default_value = cortex_retrieval::client::DEFAULT_URL)]
    pub embedding_url: String,

    /// A measured connectome for `--with brain`, as a text file of
    /// `source,target,strength` lines. Without it the wiring is generated.
    #[arg(long, value_name = "FILE")]
    pub connectome: Option<PathBuf>,

    /// Address of the HOPE memory service used by `--with hope`.
    #[arg(long, default_value = cortex_memory::hope::DEFAULT_URL)]
    pub hope_url: String,

    /// A command the verification gate runs instead of detecting the
    /// project's checks. Repeat to run several. Used with `--with gate`.
    #[arg(long = "check", value_name = "COMMAND")]
    pub checks: Vec<String>,

    /// Times the gate may send failing work back to the model before the
    /// run ends unverified.
    #[arg(long, default_value_t = 2)]
    pub max_check_rounds: u32,

    /// Where `--with memory` keeps what it has learned. Defaults to
    /// `.cortex/memory.db` in the project, or a new file per eval run.
    #[arg(long, value_name = "FILE")]
    pub memory_db: Option<PathBuf>,

    /// Attempts made at most with `--with rollouts`.
    #[arg(long, default_value_t = 3, value_name = "COUNT")]
    pub rollouts: u32,

    /// With `--with rollouts`, make all attempts at the same time and
    /// keep the best, instead of stopping at the first convincing one.
    /// Faster, and always costs every attempt.
    #[arg(long)]
    pub rollouts_parallel: bool,

    /// Maximum number of model calls per instruction.
    #[arg(long, default_value_t = 60)]
    pub max_steps: u32,
}

/// Where the agent works and what it may do without asking.
#[derive(Args, Clone)]
pub struct SessionArgs {
    /// Project directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Run commands and edit files without asking first.
    #[arg(long)]
    pub yes: bool,

    #[command(flatten)]
    pub model: ModelArgs,
}

#[derive(Args)]
pub struct RunArgs {
    /// What the agent should do.
    pub instruction: String,

    #[command(flatten)]
    pub session: SessionArgs,
}

#[derive(Args)]
pub struct SearchArgs {
    /// A description of the code or behaviour to find.
    pub query: String,

    /// Project directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    #[arg(long, default_value_t = 8)]
    pub max_results: usize,

    /// Address of the embedding service.
    #[arg(long, default_value = cortex_retrieval::client::DEFAULT_URL)]
    pub embedding_url: String,
}

#[derive(Args)]
pub struct ContextArgs {
    /// The instruction to gather context for.
    pub instruction: String,

    /// Project directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Memory scope to read. Defaults to the project directory; an eval
    /// run's memory uses the suite name.
    #[arg(long)]
    pub scope: Option<String>,

    /// Also run the verification gate's checks and show the result.
    #[arg(long)]
    pub run_checks: bool,

    #[command(flatten)]
    pub model: ModelArgs,
}

#[derive(Args)]
pub struct PrepareArgs {
    /// Project directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Do the parts that need no model and leave the description out.
    #[arg(long)]
    pub no_model: bool,

    #[command(flatten)]
    pub model: ModelArgs,
}

#[derive(Args)]
pub struct ChatArgs {
    #[command(flatten)]
    pub session: SessionArgs,
}

#[derive(Subcommand)]
pub enum EvalCommand {
    /// Run the agent on every task and save a report.
    Run(Box<EvalRunArgs>),

    /// Check, without a model, that each task fails untouched and passes
    /// with its reference solution.
    Validate(EvalValidateArgs),

    /// Compare two saved reports.
    Compare(EvalCompareArgs),

    /// Measure, without a model, whether the context gathered for each
    /// task names the files its solution changes. Choose the context
    /// sources with `--with`.
    Locate(EvalLocateArgs),

    /// Measure whether masking old tool output changes the model's next
    /// step, by asking the model at steps of saved runs with and without
    /// it. Two model calls per step sampled.
    Preserve(EvalPreserveArgs),
}

#[derive(Args)]
pub struct EvalRunArgs {
    /// Suite directory.
    #[arg(long, default_value = "eval/suites/smoke")]
    pub suite: PathBuf,

    /// Name for this configuration, used in the report and its folder name.
    #[arg(long, default_value = "baseline")]
    pub label: String,

    /// Where reports are written.
    #[arg(long, default_value = "eval/results")]
    pub out: PathBuf,

    /// Attempts per task.
    #[arg(long, default_value_t = 1)]
    pub repeats: u32,

    /// Only run tasks whose name contains this text.
    #[arg(long)]
    pub filter: Option<String>,

    #[command(flatten)]
    pub model: ModelArgs,
}

#[derive(Args)]
pub struct EvalLocateArgs {
    /// Suite directory.
    #[arg(long, default_value = "eval/suites/smoke")]
    pub suite: PathBuf,

    /// Only tasks whose name contains this text.
    #[arg(long)]
    pub filter: Option<String>,

    #[command(flatten)]
    pub model: ModelArgs,
}

#[derive(Args)]
pub struct EvalPreserveArgs {
    /// A saved run's directory, or any directory holding trajectory logs.
    pub runs: PathBuf,

    /// Steps asked about per saved run, at most.
    #[arg(long, default_value_t = 2)]
    pub samples: usize,

    /// Steps whose tool output is kept, as `--with mask` keeps ten.
    #[arg(long, default_value_t = 10)]
    pub keep_steps: usize,

    #[command(flatten)]
    pub model: ModelArgs,
}

#[derive(Args)]
pub struct EvalValidateArgs {
    #[arg(long, default_value = "eval/suites/smoke")]
    pub suite: PathBuf,
}

#[derive(Args)]
pub struct EvalCompareArgs {
    /// The earlier report (`report.json`).
    pub baseline: PathBuf,

    /// The report to judge against it.
    pub candidate: PathBuf,
}

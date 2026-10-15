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

    /// List the skills that `/name` starts: the built-in ones, your own
    /// in `~/.cortex/skills`, and the project's in `.cortex/skills`.
    Skills(SkillsArgs),

    /// Sign in to ChatGPT in the browser, to use a subscription's models
    /// with `--provider codex`.
    Login,

    /// Show the tokens used: a calendar of one month with the total of
    /// each day, then the busiest day and hour, the model used most, the
    /// sessions, and what it cost where the price is known.
    Usage(UsageArgs),

    /// Show whether the advisers help: for each one, how runs went with
    /// it speaking and with it held out.
    Effect(EffectArgs),
}

#[derive(Args)]
pub struct UsageArgs {
    /// The month to show, as `YYYY-MM`. Defaults to this month.
    #[arg(long, value_name = "YYYY-MM")]
    pub month: Option<String>,

    /// Print everything as JSON.
    #[arg(long)]
    pub json: bool,

    /// The folder the records are kept under, instead of `~/.cortex`.
    #[arg(long, value_name = "FOLDER")]
    pub records_dir: Option<PathBuf>,

    /// Minutes your clock is ahead of UTC, when it cannot be found out
    /// from the system. Days are counted on your clock.
    #[arg(long, value_name = "MINUTES", allow_hyphen_values = true)]
    pub utc_offset: Option<i32>,
}

#[derive(Args)]
pub struct EffectArgs {
    /// Only runs from this day on, as `YYYY-MM-DD`. Defaults to all.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub since: Option<String>,

    /// Only runs in this project folder.
    #[arg(long, value_name = "FOLDER")]
    pub project: Option<String>,

    /// Only runs of this model.
    #[arg(long)]
    pub model: Option<String>,

    /// Print everything as JSON.
    #[arg(long)]
    pub json: bool,

    /// The folder the records are kept under, instead of `~/.cortex`.
    #[arg(long, value_name = "FOLDER")]
    pub records_dir: Option<PathBuf>,

    /// Minutes your clock is ahead of UTC, when it cannot be found out
    /// from the system.
    #[arg(long, value_name = "MINUTES", allow_hyphen_values = true)]
    pub utc_offset: Option<i32>,
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

    /// Turn off a capability, including one that is on unless turned off:
    /// `run` and `chat` start with `brain`. Repeat to turn off several.
    #[arg(long = "without", value_enum, value_name = "FEATURE")]
    pub without: Vec<Feature>,

    /// Address of the embedding service used by `--with semantic`.
    #[arg(long, default_value = cortex_retrieval::client::DEFAULT_URL)]
    pub embedding_url: String,

    /// Address of the HOPE memory service used by `--with hope`.
    #[arg(long, default_value = cortex_brain::memory::hope::DEFAULT_URL)]
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

    /// Another project whose memory `--with memory` also recalls from:
    /// its tasks most like the one at hand, and its advice that is not
    /// about any one file. Repeat for several.
    #[arg(long = "memory-from", value_name = "FOLDER")]
    pub memory_from: Vec<PathBuf>,

    /// Attempts made at most with `--with rollouts`.
    #[arg(long, default_value_t = 3, value_name = "COUNT")]
    pub rollouts: u32,

    /// With `--with rollouts`, make all attempts at the same time and
    /// keep the best, instead of stopping at the first convincing one.
    /// Faster, and always costs every attempt.
    #[arg(long)]
    pub rollouts_parallel: bool,

    /// With `--with browser`, show the model the screenshots it takes.
    /// Only for a model that takes images; another may refuse the whole
    /// request. Without this a screenshot is only saved in the project.
    #[arg(long)]
    pub browser_pictures: bool,

    /// With `--with browser`, open a browser window you can watch,
    /// instead of running the browser out of sight.
    #[arg(long)]
    pub browser_window: bool,

    /// Run the commands tools start with your own rights. Without this
    /// they run in a sandbox: they can write only in the project and
    /// temporary folders, and cannot read where keys are kept.
    #[arg(long)]
    pub no_sandbox: bool,

    /// The model's context window in tokens. Needed for a model whose
    /// window is not known (any that is not one of Anthropic's), both to
    /// show how full it is and for `--with compact` to act on its own.
    /// A smaller number than the real window makes it compact sooner.
    #[arg(long, value_name = "TOKENS")]
    pub context_window: Option<u64>,

    /// With `--with compact`, the share of the context window at which
    /// the conversation is compacted.
    #[arg(long, default_value_t = cortex_core::compact::DEFAULT_AUTO_AT, value_name = "SHARE")]
    pub compact_at: f32,

    /// With `--with compact`, the latest responses of the model that are
    /// kept as they are. Not used with the Anthropic provider, where the
    /// summary replaces everything.
    #[arg(long, default_value_t = cortex_core::compact::DEFAULT_KEEP_RECENT, value_name = "RESPONSES")]
    pub compact_keep: usize,

    /// The share of runs the brain, as an adviser, is held out of: it watches and learns, and what it would
    /// have said is recorded and not shown to the model, so that
    /// `cortex effect` can compare runs with and without it. 0 for none.
    #[arg(long, default_value_t = cortex_core::effect::DEFAULT_SHARE, value_name = "SHARE")]
    pub holdout: f64,

    /// The same for what is put before a task: memory, lessons, the
    /// router's findings. Off unless given, because a run without its
    /// memory differs more. A skill asked for by name is never held out.
    #[arg(long, default_value_t = 0.0, value_name = "SHARE")]
    pub holdout_briefings: f64,

    /// The folder the records of tokens used and of how runs went are
    /// kept under, instead of `~/.cortex`.
    #[arg(long, value_name = "FOLDER")]
    pub records_dir: Option<PathBuf>,

    /// Keep no record of tokens used or of how runs went.
    #[arg(long)]
    pub no_records: bool,

    /// Maximum number of model calls per instruction.
    #[arg(long, default_value_t = 60)]
    pub max_steps: u32,
}

impl ModelArgs {
    pub fn sandbox(&self) -> cortex_core::Sandbox {
        cortex_core::Sandbox::enabled(!self.no_sandbox)
    }
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
pub struct SkillsArgs {
    /// Project directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Print the full text of this skill instead of the list.
    pub name: Option<String>,
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

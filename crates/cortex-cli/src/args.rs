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

    /// Show what the context router would put in front of an
    /// instruction, without calling a model.
    Context(ContextArgs),

    /// Get a project ready ahead of any task: build the code graph, embed
    /// the code, and have the model write a short description of the
    /// project to show now.
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

    /// Undo the last run's changes to the project. Every run saves the
    /// project before it first changes anything.
    Undo(CheckpointArgs),

    /// List the saved states of the project, or restore one.
    Checkpoints(CheckpointArgs),

    /// Read a run's log: what it did, where it differs from another run,
    /// and whether its tool calls still give the same output.
    Trace(TraceArgs),

    /// Show what the project's runs have taught: how each kind of task
    /// goes, which notes help, and workflows to keep as skills.
    Learned(LearnedArgs),

    /// Background jobs: agents that work in parallel, each on a branch of
    /// its own, and keep going when the window that started them closes.
    Jobs(JobsArgs),
}

#[derive(Args)]
pub struct CheckpointArgs {
    /// The project. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// The checkpoint to restore, by the start of its id. Without it,
    /// `checkpoints` lists them and `undo` restores the latest.
    pub id: Option<String>,
}

#[derive(Args)]
pub struct TraceArgs {
    /// A session log (`.jsonl`).
    pub log: PathBuf,

    /// Which run of the log, from 1. Defaults to the last.
    #[arg(long)]
    pub run: Option<usize>,

    /// Another log to compare with: where the two runs part ways.
    #[arg(long, value_name = "LOG")]
    pub compare: Option<PathBuf>,

    /// Which run of the other log. Defaults to the last.
    #[arg(long)]
    pub other_run: Option<usize>,

    /// List every tool call.
    #[arg(long)]
    pub steps: bool,

    /// Make the run's tool calls again in a copy of the project and show
    /// the first whose output differs.
    #[arg(long)]
    pub replay: bool,

    /// The project, for `--replay`. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
}

#[derive(Args)]
pub struct LearnedArgs {
    /// The project. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Save a candidate skill to `.cortex/skills`.
    #[arg(long, value_name = "NAME")]
    pub accept: Option<String>,

    /// Stop offering a candidate skill.
    #[arg(long, value_name = "NAME")]
    pub dismiss: Option<String>,
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
    /// Repeat to turn off several.
    #[arg(long = "without", value_enum, value_name = "FEATURE")]
    pub without: Vec<Feature>,

    /// Address of the embedding service used by `--with semantic`.
    #[arg(long, default_value = cortex_retrieval::client::DEFAULT_URL)]
    pub embedding_url: String,

    /// A command the verification gate runs instead of detecting the
    /// project's checks. Repeat to run several. Used with `--with gate`.
    #[arg(long = "check", value_name = "COMMAND")]
    pub checks: Vec<String>,

    /// Times the gate may send failing work back to the model before the
    /// run ends unverified.
    #[arg(long, default_value_t = 2)]
    pub max_check_rounds: u32,

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

    /// The share of runs an adviser is held out of: what it would
    /// have said is recorded and not shown to the model, so that
    /// `cortex effect` can compare runs with and without it. 0 for none.
    #[arg(long, default_value_t = cortex_core::effect::DEFAULT_SHARE, value_name = "SHARE")]
    pub holdout: f64,

    /// The same for what is put before a task: the
    /// router's findings. Off unless given, because a run without its
    /// context differs more. A skill asked for by name is never held out.
    #[arg(long, default_value_t = 0.0, value_name = "SHARE")]
    pub holdout_briefings: f64,

    /// The folder the records of tokens used and of how runs went are
    /// kept under, instead of `~/.cortex`.
    #[arg(long, value_name = "FOLDER")]
    pub records_dir: Option<PathBuf>,

    /// Keep no record of tokens used or of how runs went.
    #[arg(long)]
    pub no_records: bool,

    /// Do not save the project before a run changes it. Without
    /// checkpoints a run cannot be undone with `/undo`.
    #[arg(long)]
    pub no_checkpoints: bool,

    /// Do not start the MCP servers listed in `~/.cortex/mcp.json`.
    #[arg(long)]
    pub no_mcp: bool,

    /// A model of the same provider for the questions put beside a run:
    /// the review, compaction summaries and lessons. Usually a cheaper one.
    #[arg(long, value_name = "MODEL")]
    pub helper_model: Option<String>,

    /// A model of the same provider to carry on with when the main one is
    /// rate limited or unavailable. May be given more than once; tried in
    /// order.
    #[arg(long = "fallback-model", value_name = "MODEL")]
    pub fallback_models: Vec<String>,

    /// A stronger model of the same provider to move to when a run keeps
    /// failing after it was told to change approach.
    #[arg(long, value_name = "MODEL")]
    pub escalation_model: Option<String>,

    /// Stop a run once it has cost this many US dollars, at list price.
    /// It is told to wrap up at 80%.
    #[arg(long, value_name = "USD")]
    pub max_cost: Option<f64>,

    /// Stop a run once it has used this many tokens, in and out.
    #[arg(long, value_name = "TOKENS")]
    pub max_tokens: Option<u64>,

    /// Stop a run after this many minutes of work.
    #[arg(long, value_name = "MINUTES")]
    pub max_minutes: Option<f64>,

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

    /// For scripts and CI: write every event of the run to standard
    /// output as one JSON object per line, then a final `result` line,
    /// instead of the console view. Approvals that cannot be asked are
    /// refused unless `--yes` is given.
    #[arg(long)]
    pub json: bool,

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

#[derive(Args)]
pub struct JobsArgs {
    #[command(subcommand)]
    pub command: JobsCommand,
}

/// What `cortex jobs` does.
#[derive(Subcommand)]
pub enum JobsCommand {
    /// List the project's background jobs.
    List(JobsListArgs),

    /// Show one job: its record, what it did lately, and its report.
    Show(JobArgs),

    /// Queue a job, and start a runner for it if none is running.
    Add(JobsAddArgs),

    /// Stop a queued or running job.
    Cancel(JobArgs),

    /// Merge a finished job's branch into the project, after a conflict
    /// check.
    Merge(JobArgs),

    /// Have an agent resolve a conflicted job's conflicts in its own
    /// worktree, then merge it.
    Resolve(JobArgs),

    /// Queue a failed or cancelled job again, carrying on where it
    /// stopped.
    Retry(JobArgs),

    /// Forget a job that is not running, and remove its worktree and
    /// branch. A branch that was never merged is kept unless `--force`.
    Remove(JobsRemoveArgs),

    /// Carry out the project's jobs, several at once, until none are
    /// left. The model arguments after `--` are kept for runners started
    /// later; without them the last ones kept are used.
    Serve(JobsServeArgs),
}

#[derive(Args)]
pub struct JobsListArgs {
    /// The project. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Print the jobs as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct JobArgs {
    /// The job, such as `job-3` or `3`.
    pub id: String,

    /// The project. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
}

#[derive(Args)]
pub struct JobsRemoveArgs {
    #[command(flatten)]
    pub job: JobArgs,

    /// Delete the job's branch even if it was never merged.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct JobsAddArgs {
    /// What the job's agent should do. It does not see any conversation,
    /// so say everything it needs.
    pub instruction: String,

    /// The project. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// A few words naming the job.
    #[arg(long)]
    pub title: Option<String>,

    /// A job that must be finished and merged first. Repeat for several.
    #[arg(long = "after", value_name = "JOB")]
    pub after: Vec<String>,

    /// Leave the finished branch for `cortex jobs merge` instead of
    /// merging it as soon as the job finishes well.
    #[arg(long)]
    pub no_merge: bool,

    /// Model calls the job may make.
    #[arg(long)]
    pub max_steps: Option<u32>,
}

#[derive(Args)]
pub struct JobsServeArgs {
    /// The project. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Jobs carried out at once.
    #[arg(long, value_name = "COUNT")]
    pub parallel: Option<usize>,

    /// Stop after this many seconds with nothing to do.
    #[arg(long, default_value_t = 60, value_name = "SECONDS")]
    pub idle_exit: u64,

    /// The `cortex run` model arguments for the jobs' agents, such as
    /// `--provider anthropic --model claude-opus-5-5 --with gate`.
    #[arg(last = true, value_name = "MODEL ARGS")]
    pub model: Vec<String>,
}

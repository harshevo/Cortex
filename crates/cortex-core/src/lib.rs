//! Core of the Cortex coding agent.
//!
//! This crate owns the agent loop and the interfaces around it. It knows
//! nothing about any specific model provider, tool, or user interface:
//!
//! - [`provider::ModelProvider`] is implemented by `cortex-providers`.
//! - [`tool::Tool`] is implemented by `cortex-tools`.
//! - [`trajectory::TrajectorySink`] is implemented by loggers and by the CLI.
//! - [`approval::Approver`] is implemented by whoever decides what may run.
//! - [`briefing::ContextSource`] is implemented by the context router.
//! - [`verify::Verifier`] is implemented by `cortex-verify`.
//! - [`advice::Adviser`] allows optional between-step guidance.
//! - [`rollout::StrategyPolicy`] is implemented by the strategy bandit in `cortex-cli`.

pub mod advice;
pub mod agent;
pub mod approval;
pub mod briefing;
pub mod budget;
pub mod changes;
pub mod checkpoint;
pub mod compact;
pub mod context;
pub mod effect;
pub mod error;
pub mod fallback;
pub mod hooks;
pub mod inbox;
pub mod jobs;
pub mod judge;
pub mod learn;
pub mod ledger;
pub mod merge;
pub mod message;
pub mod notes;
pub mod plan;
pub mod process;
pub mod prompt;
pub mod provider;
pub mod redact;
pub mod reflect;
pub mod repeat;
pub mod replay;
pub mod review;
pub mod rollout;
pub mod rules;
pub mod sandbox;
pub mod services;
pub mod stuck;
pub mod task_state;
pub mod tool;
pub mod trace;
pub mod trajectory;
pub mod usage;
pub mod verify;
pub mod window;
pub mod workspace;
pub mod worktree;

pub use advice::{Advice, Adviser};
pub use agent::{Agent, AgentConfig, Extras, Outcome, Progress, RunReport};
pub use approval::{ApprovalRequest, Approver, AutoApprove, destructive};
pub use briefing::{Briefing, ContextSource};
pub use checkpoint::{Checkpoint, Checkpoints, Restored};
pub use compact::{CompactConfig, Compaction};
pub use context::{MaskConfig, PruneConfig};
pub use effect::{EffectLedger, EffectSink, ExperimentConfig};
pub use error::{AgentError, ProviderError, ToolError};
pub use fallback::Fallback;
pub use hooks::{CommandHooks, ToolHooks};
pub use inbox::Inbox;
pub use ledger::{Ledger, LedgerSink, PriceTable};
pub use message::{Block, Image, Message, Role, ToolCall, ToolResult};
pub use provider::{
    ModelProvider, ModelRequest, ModelResponse, StopReason, StreamObserver, ToolSpec,
};
pub use reflect::Lesson;
pub use review::{Requirement, Review};
pub use rollout::{FixedStrategies, RolloutConfig, Strategy, StrategyPolicy};
pub use sandbox::Sandbox;
pub use stuck::StuckDetector;
pub use tool::{Access, Tool, ToolContext, ToolOutput, ToolRegistry};
pub use trajectory::{Event, JsonlSink, TrajectorySink};
pub use usage::Usage;
pub use verify::{CheckOutcome, Verification, Verifier};
pub use window::{ContextParts, ContextUse, PlanQuota, QuotaWindow};
pub use workspace::Workspace;

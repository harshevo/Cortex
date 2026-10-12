//! Core of the Cortex coding agent.
//!
//! This crate owns the agent loop and the interfaces around it. It knows
//! nothing about any specific model provider, tool, or user interface:
//!
//! - [`provider::ModelProvider`] is implemented by `cortex-providers`.
//! - [`tool::Tool`] is implemented by `cortex-tools`.
//! - [`trajectory::TrajectorySink`] is implemented by loggers and by the CLI.
//! - [`approval::Approver`] is implemented by whoever decides what may run.
//! - [`briefing::ContextSource`] is implemented by memory and the router.
//! - [`verify::Verifier`] is implemented by `cortex-verify`.
//! - [`advice::Adviser`] is implemented by the decision controllers in `cortex-brain`.
//! - [`rollout::StrategyPolicy`] is implemented by the bandit in `cortex-brain`.

pub mod advice;
pub mod agent;
pub mod approval;
pub mod briefing;
pub mod changes;
pub mod context;
pub mod error;
pub mod inbox;
pub mod judge;
pub mod message;
pub mod process;
pub mod prompt;
pub mod provider;
pub mod reflect;
pub mod repeat;
pub mod replay;
pub mod review;
pub mod rollout;
pub mod tool;
pub mod trajectory;
pub mod usage;
pub mod verify;
pub mod workspace;

pub use advice::{Advice, Adviser};
pub use agent::{Agent, AgentConfig, Extras, Outcome, Progress, RunReport};
pub use approval::{ApprovalRequest, Approver, AutoApprove};
pub use briefing::{Briefing, ContextSource};
pub use context::{MaskConfig, PruneConfig};
pub use error::{AgentError, ProviderError, ToolError};
pub use inbox::Inbox;
pub use message::{Block, Message, Role, ToolCall, ToolResult};
pub use provider::{
    ModelProvider, ModelRequest, ModelResponse, StopReason, StreamObserver, ToolSpec,
};
pub use reflect::Lesson;
pub use review::{Requirement, Review};
pub use rollout::{FixedStrategies, RolloutConfig, Strategy, StrategyPolicy};
pub use tool::{Access, Tool, ToolContext, ToolRegistry};
pub use trajectory::{Event, JsonlSink, TrajectorySink};
pub use usage::Usage;
pub use verify::{CheckOutcome, Verification, Verifier};
pub use workspace::Workspace;

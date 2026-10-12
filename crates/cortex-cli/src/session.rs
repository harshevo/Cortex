//! Building an agent from command-line arguments.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use cortex_core::trajectory::unix_millis;
use cortex_core::{
    Agent, AgentConfig, Approver, AutoApprove, JsonlSink, ModelProvider, ToolContext, Workspace,
    prompt,
};
use cortex_providers::{
    AnthropicConfig, AnthropicProvider, CodexConfig, CodexProvider, OpenAiCompatConfig,
    OpenAiCompatProvider,
};

use crate::approver::InteractiveApprover;
use crate::args::{ModelArgs, ProviderKind, SessionArgs};
use crate::console::ConsoleSink;
use crate::features::{self, MemoryLocation};

/// Folder inside the project where Cortex keeps its own files.
const STATE_DIRECTORY: &str = ".cortex";

pub fn build_provider(args: &ModelArgs) -> Result<Arc<dyn ModelProvider>> {
    let model = match (&args.model, args.provider.default_model()) {
        (Some(model), _) => model.as_str(),
        (None, Some(default)) => default,
        (None, None) => bail!("--model is required with this provider"),
    };

    let provider: Arc<dyn ModelProvider> = match args.provider {
        ProviderKind::Hosted => {
            let Some(base_url) = args.base_url.as_deref() else {
                bail!("--base-url is required with `--provider hosted`");
            };

            let config = OpenAiCompatConfig::hosted(model, base_url)?;

            Arc::new(OpenAiCompatProvider::new(config)?)
        }

        ProviderKind::Anthropic => {
            let config = AnthropicConfig::from_env(model, &args.effort)?;

            Arc::new(AnthropicProvider::new(config)?)
        }

        ProviderKind::Codex => Arc::new(CodexProvider::new(CodexConfig::new(model, &args.effort))?),

        ProviderKind::Local => {
            let config = OpenAiCompatConfig::local(model, args.base_url.as_deref());

            Arc::new(OpenAiCompatProvider::new(config)?)
        }
    };

    Ok(provider)
}

pub async fn build_agent(args: &SessionArgs) -> Result<Agent> {
    let workspace = Workspace::new(&args.dir)
        .with_context(|| format!("cannot open the project directory {}", args.dir.display()))?;

    let provider = build_provider(&args.model)?;

    let approver: Arc<dyn Approver> = if args.yes {
        Arc::new(AutoApprove)
    } else {
        Arc::new(InteractiveApprover::default())
    };

    let trajectory_file = workspace
        .root()
        .join(STATE_DIRECTORY)
        .join("trajectories")
        .join(format!("{}.jsonl", unix_millis()));

    let log = JsonlSink::create(&trajectory_file)
        .with_context(|| format!("cannot create {}", trajectory_file.display()))?;

    let state_directory = workspace.root().join(STATE_DIRECTORY);

    let memory_location = MemoryLocation {
        path: args
            .model
            .memory_db
            .clone()
            .unwrap_or_else(|| state_directory.join("memory.db")),
        scope: workspace.root().display().to_string(),
        skip_identical: false,
    };

    let setup = features::setup(&args.model, memory_location).await?;

    let mut notes = setup.prompt_notes.clone();

    notes.extend(
        setup
            .verifier
            .as_ref()
            .and_then(|verifier| prompt::verification_note(&verifier.commands(&workspace))),
    );

    let config = AgentConfig {
        system_prompt: prompt::system_prompt_with(&workspace, &notes),
        max_steps: args.model.max_steps,
        prune: setup.prune,
        max_verification_rounds: args.model.max_check_rounds,
        fold_repeats: setup.fold_repeats,
        extras: setup.extras,
    };

    let context = ToolContext {
        workspace,
        approver,
    };

    let console = ConsoleSink::new(provider.model());

    let mut agent = Agent::new(provider, setup.tools, context, config)
        .with_sink(Arc::new(console))
        .with_sink(Arc::new(log));

    for sink in setup.sinks {
        agent = agent.with_sink(sink);
    }

    for source in setup.context_sources {
        agent = agent.with_context_source(source);
    }

    if let Some(verifier) = setup.verifier {
        agent = agent.with_verifier(verifier);
    }

    for adviser in setup.advisers {
        agent = agent.with_adviser(adviser);
    }

    Ok(agent)
}

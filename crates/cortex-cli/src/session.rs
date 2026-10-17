//! Building an agent from command-line arguments.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use cortex_core::trajectory::unix_millis;
use cortex_core::{
    Agent, AgentConfig, ApprovalRequest, Approver, AutoApprove, Fallback, JsonlSink, ModelProvider,
    ToolContext, TrajectorySink, Workspace, prompt,
};
use cortex_providers::{
    AnthropicConfig, AnthropicProvider, CodexConfig, CodexProvider, OpenAiCompatConfig,
    OpenAiCompatProvider,
};

use crate::approver::InteractiveApprover;
use crate::args::{ModelArgs, ProviderKind, SessionArgs};
use crate::console::{ConsoleSink, JsonLinesSink};
use crate::features::{self, SessionLocation};

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

/// How [`build_agent_with`] differs from a session at the terminal.
#[derive(Default)]
pub struct BuildOptions {
    /// Decides what may run, in place of the one `args` would choose.
    pub approver: Option<Arc<dyn Approver>>,

    /// Where the run's log is written, in place of a new file under
    /// `.cortex/trajectories`.
    pub log: Option<PathBuf>,

    /// Add to the log at `log` instead of starting it afresh, as when a
    /// run is taken up again.
    pub append_log: bool,

    /// Leave out the console view (or the JSON lines of `--json`).
    pub quiet: bool,
}

pub async fn build_agent(args: &SessionArgs) -> Result<Agent> {
    build_agent_with(args, BuildOptions::default()).await
}

/// As [`build_agent`], changed by `options`: for runs nobody watches at
/// a terminal, such as background jobs.
pub async fn build_agent_with(args: &SessionArgs, options: BuildOptions) -> Result<Agent> {
    // A session starts with the capabilities that are on unless turned off.
    let model = args.model.clone().with_defaults();

    let workspace = Workspace::new(&args.dir)
        .with_context(|| format!("cannot open the project directory {}", args.dir.display()))?
        .with_sandbox(model.sandbox());

    eprintln!("{}", workspace.sandbox().summary(workspace.root()));

    let provider = build_routed_provider(&model)?;

    let approver: Arc<dyn Approver> = if let Some(approver) = options.approver {
        approver
    } else if args.yes {
        Arc::new(AutoApprove)
    } else if args.json {
        // Nobody is at the terminal to ask.
        Arc::new(DenyAll)
    } else {
        Arc::new(InteractiveApprover::default())
    };

    let trajectory_file = options.log.unwrap_or_else(|| {
        workspace
            .root()
            .join(STATE_DIRECTORY)
            .join("trajectories")
            .join(format!("{}.jsonl", unix_millis()))
    });

    let log = match options.append_log && trajectory_file.exists() {
        true => JsonlSink::append(&trajectory_file),
        false => JsonlSink::create(&trajectory_file),
    }
    .with_context(|| format!("cannot create {}", trajectory_file.display()))?;

    let state_directory = workspace.root().join(STATE_DIRECTORY);

    let location = SessionLocation {
        directory: state_directory.clone(),
        scope: workspace.root().display().to_string(),
        project: Some(workspace.root().to_path_buf()),
    };

    let setup = features::setup(&model, location).await?;

    for line in &setup.report {
        eprintln!("{line}");
    }

    // The records name the session as its log does.
    if let Some(stem) = trajectory_file.file_stem() {
        setup.name_session(&stem.to_string_lossy());
    }

    let notes = setup.notes(&workspace);

    let config = AgentConfig {
        system_prompt: prompt::system_prompt_with(&workspace, &notes),
        max_steps: model.max_steps,
        prune: setup.prune,
        max_verification_rounds: model.max_check_rounds,
        fold_repeats: setup.fold_repeats,
        extras: setup.extras,
    };

    let context = ToolContext {
        workspace,
        approver,
    };

    let console: Option<Arc<dyn TrajectorySink>> = match (options.quiet, args.json) {
        (true, _) => None,
        (false, true) => Some(Arc::new(JsonLinesSink)),
        (false, false) => Some(Arc::new(ConsoleSink::new(provider.model()))),
    };

    let mut agent = Agent::new(provider, setup.tools, context, config);

    if let Some(console) = console {
        agent = agent.with_sink(console);
    }

    agent = agent.with_sink(Arc::new(log));

    for sink in setup.sinks {
        agent = agent.with_sink(sink);
    }

    for source in setup.context_sources {
        agent = agent.with_context_source(source);
    }

    if let Some(hooks) = setup.hooks {
        agent = agent.with_hooks(hooks);
    }

    if let Some(helper) = build_helper(&model)? {
        agent = agent.with_helper(helper);
    }

    if let Some(stronger) = build_escalation(&model)? {
        agent = agent.with_escalation(stronger);
    }

    if let Some(recovery) = setup.recovery {
        agent = agent.with_recovery(recovery);
    }

    if let Some(verifier) = setup.verifier {
        agent = agent.with_verifier(verifier);
    }

    for adviser in setup.advisers {
        agent = agent.with_adviser(adviser);
    }

    Ok(agent)
}

/// The main model, with the fallback models behind it.
pub fn build_routed_provider(args: &ModelArgs) -> Result<Arc<dyn ModelProvider>> {
    let mut providers = vec![build_provider(args)?];

    for model in &args.fallback_models {
        let fallback = ModelArgs {
            model: Some(model.clone()),
            ..args.clone()
        };

        providers.push(build_provider(&fallback)?);
    }

    Ok(Fallback::over(providers))
}

/// The model for the questions beside a run, when one is named.
pub fn build_helper(args: &ModelArgs) -> Result<Option<Arc<dyn ModelProvider>>> {
    let Some(model) = &args.helper_model else {
        return Ok(None);
    };

    let helper = ModelArgs {
        model: Some(model.clone()),
        ..args.clone()
    };

    build_provider(&helper).map(Some)
}

/// The stronger model to move to when a run is stuck, when one is named.
pub fn build_escalation(args: &ModelArgs) -> Result<Option<Arc<dyn ModelProvider>>> {
    let Some(model) = &args.escalation_model else {
        return Ok(None);
    };

    let stronger = ModelArgs {
        model: Some(model.clone()),
        ..args.clone()
    };

    build_provider(&stronger).map(Some)
}

/// Refuses every call that needs approval: for runs with nobody to ask.
struct DenyAll;

#[async_trait::async_trait]
impl Approver for DenyAll {
    async fn approve(&self, request: &ApprovalRequest) -> bool {
        eprintln!(
            "refused without --yes: {} ({})",
            request.tool, request.summary
        );

        false
    }
}

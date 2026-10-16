//! Everything needed to build the agent from a [`Launch`].
//!
//! The agent is put together exactly as the `cortex` command does it, by
//! the same [`features::setup`], so every capability works here as it
//! does there.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use cortex_cli::args::{ModelArgs, ProviderKind};
use cortex_cli::features::{self, Feature, SessionLocation, Setup};
use cortex_core::{AgentConfig, ModelProvider, Workspace, prompt};
use cortex_providers::{
    AnthropicConfig, AnthropicProvider, CodexConfig, CodexProvider, OpenAiCompatConfig,
    OpenAiCompatProvider,
};

use super::demo::{self, DemoProvider};
use super::{Info, Launch};
use crate::settings::{Provider, Settings};

/// Folder inside the project where Cortex keeps its own files.
const STATE_DIRECTORY: &str = ".cortex";

pub struct Blueprint {
    pub info: Info,
    pub provider: Arc<dyn ModelProvider>,
    pub workspace: Workspace,
    pub config: AgentConfig,

    /// The arguments the agent was built from.
    pub model: ModelArgs,

    /// Tools, context sources, sinks, verifier and advisers: what the
    /// chosen capabilities add up to.
    pub setup: Setup,

    pub approve_everything: bool,
}

impl Blueprint {
    /// `demo_project` is the stand-in model's throwaway project, when the
    /// launch is a demo.
    pub async fn prepare(launch: &Launch, demo_project: Option<&Path>) -> Result<Self> {
        let mut model = launch.settings.model_args()?;

        let place = match demo_project {
            Some(project) => {
                prepare_demo(&mut model);

                project
            }

            None => launch.place.as_path(),
        };

        let workspace = Workspace::new(place)
            .with_context(|| format!("cannot open the folder {}", place.display()))?
            .with_sandbox(model.sandbox());

        let provider: Arc<dyn ModelProvider> = match demo_project {
            Some(_) => Arc::new(DemoProvider::new(workspace.root())),

            None => {
                let key = provider_key(launch).await;

                build_provider(&launch.settings, key.as_deref())?
            }
        };

        let setup = features::setup(
            &model,
            SessionLocation {
                directory: workspace.root().join(STATE_DIRECTORY),
                scope: workspace.root().display().to_string(),
            },
        )
        .await?;

        // With a verifier, the model is told which checks it answers to.
        let notes = setup.notes(&workspace);

        let config = AgentConfig {
            system_prompt: prompt::system_prompt_with(&workspace, &notes),
            max_steps: model.max_steps,
            prune: setup.prune.clone(),
            max_verification_rounds: model.max_check_rounds,
            fold_repeats: setup.fold_repeats,
            extras: setup.extras.clone(),
        };

        let info = Info {
            place: workspace.root().to_path_buf(),
            model: provider.model().to_string(),
            max_steps: model.max_steps,
            demo: demo_project.is_some(),
        };

        Ok(Self {
            info,
            provider,
            workspace,
            config,
            model,
            setup,
            approve_everything: launch.settings.approve_everything,
        })
    }
}

/// The key the provider is built with: the one the launch brought, or
/// the one looked up now. Asking the keychain can block, and can put a
/// question to the person, so it is done on a thread that may wait.
async fn provider_key(launch: &Launch) -> Option<String> {
    if launch.api_key.is_some() || !launch.look_up_key {
        return launch.api_key.clone();
    }

    let provider = launch.settings.provider;

    tokio::task::spawn_blocking(move || crate::settings::secrets::resolve(provider))
        .await
        .ok()
        .flatten()
}

pub(crate) fn build_provider(
    settings: &Settings,
    api_key: Option<&str>,
) -> Result<Arc<dyn ModelProvider>> {
    let current = settings.current();

    if current.model.trim().is_empty() {
        bail!("No model is chosen. Pick one in Settings.");
    }

    let key = || match (api_key, settings.provider.key_variable()) {
        (Some(key), _) => Ok(key),

        (None, Some(variable)) => bail!(
            "No API key for {}. Add one in Settings, or set {variable}.",
            settings.provider.label()
        ),

        (None, None) => Ok(""),
    };

    let provider: Arc<dyn ModelProvider> = match settings.provider {
        Provider::Anthropic => {
            let config = AnthropicConfig::with_key(&current.model, &model_effort(settings), key()?);

            Arc::new(AnthropicProvider::new(config)?)
        }

        Provider::Hosted => {
            if current.base_url.trim().is_empty() {
                bail!("The hosted provider needs the address of its API. Add it in Settings.");
            }

            let config =
                OpenAiCompatConfig::hosted_with_key(&current.model, &current.base_url, key()?);

            Arc::new(OpenAiCompatProvider::new(config)?)
        }

        Provider::Codex => {
            let config = CodexConfig::new(&current.model, &model_effort(settings));

            Arc::new(CodexProvider::new(config)?)
        }

        Provider::Local => {
            let address = Some(current.base_url.as_str()).filter(|address| !address.is_empty());

            let config = OpenAiCompatConfig::local(&current.model, address);

            Arc::new(OpenAiCompatProvider::new(config)?)
        }
    };

    Ok(provider)
}

/// The effort as the API spells it. The settings spell it the same way
/// the `cortex` command does, so it is read back from there.
fn model_effort(settings: &Settings) -> String {
    settings
        .model_args()
        .map(|model| model.effort)
        .unwrap_or_else(|_| "high".to_string())
}

/// The demo answers to its project's own test, and needs no provider.
fn prepare_demo(model: &mut ModelArgs) {
    // The stand-in model has no history to protect, unlike the default
    // provider, which some capabilities refuse to run with.
    model.provider = ProviderKind::Local;

    switch_on(model, Feature::Gate);

    if model.checks.is_empty() {
        model.checks = vec![demo::CHECK.to_string()];
    }
}

fn switch_on(model: &mut ModelArgs, feature: Feature) {
    if !model.features.contains(&feature) {
        model.features.push(feature);
    }
}

//! What the person has chosen: provider, models, permissions, projects.
//!
//! Kept in `~/.cortex/app.json`. API keys are not in that file; they are
//! in the system keychain (see [`secrets`]).

pub mod secrets;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::parser::ValueSource;
use clap::{ArgMatches, Parser, ValueEnum};
use cortex_cli::args::{ModelArgs, ProviderKind, SessionArgs};
use cortex_cli::features::Feature;
use serde::{Deserialize, Serialize};

/// Folder in the home directory where the app keeps its own files.
const APP_DIRECTORY: &str = ".cortex";

const SETTINGS_FILE: &str = "app.json";

/// Where sessions that belong to no project do their work.
const SCRATCH_DIRECTORY: &str = "sessions";

/// Where the model runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Anthropic,

    /// A hosted service with an OpenAI-compatible API.
    Hosted,

    /// A model served on this machine, such as through Ollama.
    Local,

    /// The models of a ChatGPT subscription, through the Codex service.
    Codex,
}

impl Provider {
    pub const ALL: [Self; 4] = [Self::Anthropic, Self::Hosted, Self::Local, Self::Codex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::Hosted => "Hosted (OpenAI-compatible)",
            Self::Local => "On this machine",
            Self::Codex => "ChatGPT subscription",
        }
    }

    /// The name the `cortex` command knows the provider by.
    fn argument(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Hosted => "hosted",
            Self::Local => "local",
            Self::Codex => "codex",
        }
    }

    /// The others need none, or are signed in to instead.
    pub fn needs_key(self) -> bool {
        matches!(self, Self::Anthropic | Self::Hosted)
    }

    pub fn has_address(self) -> bool {
        matches!(self, Self::Hosted | Self::Local)
    }

    /// The environment variable the key is read from when none is saved.
    pub fn key_variable(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Hosted => Some("CORTEX_API_KEY"),
            Self::Local | Self::Codex => None,
        }
    }

    /// Reasoning depth is something only these providers' APIs take.
    pub fn takes_effort(self) -> bool {
        matches!(self, Self::Anthropic | Self::Codex)
    }
}

/// How hard the model should think. Anthropic and Codex only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,

    #[default]
    High,

    Xhigh,
    Max,
}

impl Effort {
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Xhigh => "Extra high",
            Self::Max => "Max",
        }
    }

    fn argument(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn position(self) -> usize {
        Self::ALL
            .iter()
            .position(|effort| *effort == self)
            .unwrap_or(0)
    }
}

/// The choices that belong to one provider.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderSettings {
    /// The model in use.
    pub model: String,

    /// The models offered in the model selector.
    pub models: Vec<String>,

    /// Address of the API. Unused by Anthropic.
    pub base_url: String,
}

impl ProviderSettings {
    /// The models to offer: the saved list, with the one in use included.
    pub fn choices(&self) -> Vec<String> {
        let mut choices = self.models.clone();

        if !self.model.is_empty() && !choices.contains(&self.model) {
            choices.insert(0, self.model.clone());
        }

        choices
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: Provider,
    pub anthropic: ProviderSettings,
    pub hosted: ProviderSettings,
    pub local: ProviderSettings,
    pub codex: ProviderSettings,
    pub effort: Effort,

    /// Run commands and edit files without asking first.
    pub approve_everything: bool,

    /// Maximum number of model calls per instruction.
    pub max_steps: u32,

    /// Show the model's reasoning in the conversation, when the provider
    /// streams it.
    pub show_thinking: bool,

    /// Optional capabilities that are switched on, by their `--with` names.
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub brain_integration_version: u8,

    /// Attempts made at most when the `rollouts` capability is on.
    pub rollouts: u32,

    /// Make those attempts at the same time instead of stopping at the
    /// first convincing one.
    pub rollouts_parallel: bool,

    /// The project folders listed in the sidebar.
    pub projects: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        let models = |names: &[&str]| names.iter().map(|name| name.to_string()).collect();

        Self {
            provider: Provider::default(),

            anthropic: ProviderSettings {
                model: "claude-opus-5-5".to_string(),
                models: models(&[
                    "claude-opus-5-5",
                    "claude-sonnet-5-5",
                    "claude-haiku-5-5",
                    "claude-fable-5-1",
                ]),
                base_url: String::new(),
            },

            hosted: ProviderSettings::default(),

            local: ProviderSettings {
                model: "qwen2.5-coder:7b".to_string(),
                models: models(&["qwen2.5-coder:7b"]),
                base_url: "http://127.0.0.1:11434/v1".to_string(),
            },

            codex: ProviderSettings {
                model: "gpt-6.1-sol".to_string(),
                models: models(&["gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna"]),
                base_url: String::new(),
            },

            effort: Effort::default(),
            approve_everything: false,
            max_steps: 60,
            show_thinking: true,
            capabilities: [
                Feature::Memory,
                Feature::Brain,
                Feature::Intuition,
                Feature::Gate,
            ]
            .into_iter()
            .map(capability_name)
            .collect(),
            brain_integration_version: 1,
            rollouts: 3,
            rollouts_parallel: false,
            projects: Vec::new(),
        }
    }
}

impl Settings {
    /// Reads the saved settings. A missing file means first use; a file
    /// that cannot be read is reported and replaced by the defaults.
    pub fn load() -> Self {
        let Some(path) = settings_file() else {
            return Self::default();
        };

        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };

        let mut settings: Self = serde_json::from_str(&text).unwrap_or_else(|error| {
            eprintln!(
                "warning: {} is not valid, using defaults: {error}",
                path.display()
            );

            Self::default()
        });
        settings.migrate_brain();
        settings
    }

    fn migrate_brain(&mut self) {
        if self.brain_integration_version == 0 {
            if self.capabilities == [capability_name(Feature::Memory)] {
                for feature in [Feature::Brain, Feature::Intuition, Feature::Gate] {
                    self.set(feature, true);
                }
            }
            self.brain_integration_version = 1;
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_file().context("this machine has no home directory")?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn current(&self) -> &ProviderSettings {
        match self.provider {
            Provider::Anthropic => &self.anthropic,
            Provider::Hosted => &self.hosted,
            Provider::Local => &self.local,
            Provider::Codex => &self.codex,
        }
    }

    pub fn current_mut(&mut self) -> &mut ProviderSettings {
        match self.provider {
            Provider::Anthropic => &mut self.anthropic,
            Provider::Hosted => &mut self.hosted,
            Provider::Local => &mut self.local,
            Provider::Codex => &mut self.codex,
        }
    }

    pub fn has(&self, feature: Feature) -> bool {
        self.capabilities.contains(&capability_name(feature))
    }

    pub fn set(&mut self, feature: Feature, on: bool) {
        if on && matches!(feature, Feature::Brain | Feature::Controller) {
            let other = if feature == Feature::Brain {
                Feature::Controller
            } else {
                Feature::Brain
            };
            self.capabilities
                .retain(|name| *name != capability_name(other));
        }
        let name = capability_name(feature);

        self.capabilities.retain(|existing| *existing != name);

        if on {
            self.capabilities.push(name);
        }
    }

    /// Adds a project folder, if it is not listed already.
    pub fn add_project(&mut self, folder: &Path) {
        if !self.projects.iter().any(|project| project == folder) {
            self.projects.push(folder.to_path_buf());
        }
    }

    /// Takes over whatever was given explicitly on the command line, so
    /// that `cortex-gui --provider hosted --model x` sets the app up the
    /// same way the settings page would.
    pub fn apply_command_line(&mut self, matches: &ArgMatches, session: &SessionArgs) {
        let given = |name: &str| matches.value_source(name) == Some(ValueSource::CommandLine);

        if given("provider") {
            self.provider = match session.model.provider {
                ProviderKind::Anthropic => Provider::Anthropic,
                ProviderKind::Hosted => Provider::Hosted,
                ProviderKind::Local => Provider::Local,
                ProviderKind::Codex => Provider::Codex,
            };
        }

        if let Some(model) = &session.model.model {
            let current = self.current_mut();

            current.model = model.clone();

            if !current.models.contains(model) {
                current.models.push(model.clone());
            }
        }

        if let Some(base_url) = &session.model.base_url {
            self.current_mut().base_url = base_url.clone();
        }

        if given("effort")
            && let Some(effort) = Effort::ALL
                .into_iter()
                .find(|effort| effort.argument() == session.model.effort)
        {
            self.effort = effort;
        }

        if given("max_steps") {
            self.max_steps = session.model.max_steps;
        }

        for feature in &session.model.features {
            self.set(*feature, true);
        }

        if session.yes {
            self.approve_everything = true;
        }

        if given("dir")
            && let Ok(folder) = session.dir.canonicalize()
        {
            self.add_project(&folder);
        }
    }

    /// The same settings as the `cortex` command's own arguments, so the
    /// agent is built exactly as the command line would build it.
    pub fn model_args(&self) -> Result<ModelArgs> {
        let current = self.current();

        let mut arguments: Vec<String> = vec![
            "cortex".to_string(),
            "--provider".to_string(),
            self.provider.argument().to_string(),
            "--effort".to_string(),
            self.effort.argument().to_string(),
            "--max-steps".to_string(),
            self.max_steps.to_string(),
        ];

        if !current.model.is_empty() {
            arguments.extend(["--model".to_string(), current.model.clone()]);
        }

        if self.provider.has_address() && !current.base_url.is_empty() {
            arguments.extend(["--base-url".to_string(), current.base_url.clone()]);
        }

        arguments.extend(["--rollouts".to_string(), self.rollouts.max(2).to_string()]);

        if self.rollouts_parallel {
            arguments.push("--rollouts-parallel".to_string());
        }

        for capability in &self.capabilities {
            arguments.extend(["--with".to_string(), capability.clone()]);
        }

        let parsed = ModelOnly::try_parse_from(arguments)
            .context("the saved settings are not ones the agent accepts")?;

        Ok(parsed.model)
    }
}

/// Lets the `cortex` command's model arguments be parsed on their own.
#[derive(Parser)]
struct ModelOnly {
    #[command(flatten)]
    model: ModelArgs,
}

/// A capability's `--with` name.
pub fn capability_name(feature: Feature) -> String {
    feature
        .to_possible_value()
        .map(|value| value.get_name().to_string())
        .unwrap_or_default()
}

/// What a capability does, in the `cortex` command's own words.
pub fn capability_help(feature: Feature) -> String {
    feature
        .to_possible_value()
        .and_then(|value| value.get_help().map(|help| help.to_string()))
        .unwrap_or_default()
}

fn app_directory() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(APP_DIRECTORY))
}

fn settings_file() -> Option<PathBuf> {
    Some(app_directory()?.join(SETTINGS_FILE))
}

/// The folder that sessions outside any project work in. Created on first
/// use.
pub fn scratch_folder() -> Result<PathBuf> {
    let folder = app_directory()
        .context("this machine has no home directory")?
        .join(SCRATCH_DIRECTORY);

    fs::create_dir_all(&folder).with_context(|| format!("cannot create {}", folder.display()))?;

    Ok(folder)
}

#[cfg(test)]
mod tests {
    #[test]
    fn brain_defaults_migrate_once_and_preserve_later_opt_outs() {
        let mut settings = Settings {
            capabilities: vec!["memory".into()],
            brain_integration_version: 0,
            ..Settings::default()
        };
        settings.migrate_brain();
        assert!(
            settings.has(Feature::Brain)
                && settings.has(Feature::Intuition)
                && settings.has(Feature::Gate)
        );
        settings.set(Feature::Brain, false);
        settings.migrate_brain();
        assert!(!settings.has(Feature::Brain));
        settings.set(Feature::Controller, true);
        settings.set(Feature::Brain, true);
        assert!(!settings.has(Feature::Controller));
    }
    use super::*;

    #[test]
    fn settings_become_the_command_lines_own_arguments() {
        let mut settings = Settings {
            provider: Provider::Hosted,
            effort: Effort::Xhigh,
            max_steps: 25,
            ..Settings::default()
        };

        settings.hosted.model = "deepseek-v4.1-flash:free".to_string();
        settings.hosted.base_url = "https://example.test/v1".to_string();
        settings.set(Feature::Gate, true);

        let arguments = settings.model_args().unwrap();

        assert!(arguments.provider == ProviderKind::Hosted);
        assert_eq!(arguments.model.as_deref(), Some("deepseek-v4.1-flash:free"));
        assert_eq!(
            arguments.base_url.as_deref(),
            Some("https://example.test/v1")
        );
        assert_eq!(arguments.effort, "xhigh");
        assert_eq!(arguments.max_steps, 25);
        assert_eq!(
            arguments.features,
            [
                Feature::Memory,
                Feature::Brain,
                Feature::Intuition,
                Feature::Gate
            ]
        );
    }

    #[test]
    fn anthropic_is_never_given_an_address() {
        let mut settings = Settings::default();

        settings.anthropic.base_url = "https://example.test".to_string();

        assert_eq!(settings.model_args().unwrap().base_url, None);
    }

    #[test]
    fn capabilities_switch_on_and_off() {
        let mut settings = Settings::default();

        assert!(settings.has(Feature::Memory));

        settings.set(Feature::Router, true);
        settings.set(Feature::Router, true);
        settings.set(Feature::Memory, false);

        assert_eq!(
            settings.capabilities,
            ["brain", "intuition", "gate", "router"]
        );
    }

    #[test]
    fn settings_survive_being_saved_and_missing_fields_take_defaults() {
        let mut settings = Settings::default();

        settings.add_project(Path::new("/tmp/a"));
        settings.add_project(Path::new("/tmp/a"));

        let text = serde_json::to_string(&settings).unwrap();

        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), settings);
        assert_eq!(settings.projects.len(), 1);

        let sparse: Settings = serde_json::from_str(r#"{ "provider": "local" }"#).unwrap();

        assert_eq!(sparse.provider, Provider::Local);
        assert_eq!(sparse.max_steps, 60);
    }

    #[test]
    fn the_model_in_use_is_always_offered() {
        let settings = ProviderSettings {
            model: "b".to_string(),
            models: vec!["a".to_string()],
            base_url: String::new(),
        };

        assert_eq!(settings.choices(), ["b", "a"]);
    }
}

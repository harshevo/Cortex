//! The choices the agent on this phone is built from, and where they are
//! kept.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use cortex_cli::args::ModelArgs;
use cortex_cli::features::Feature;
use cortex_remote::wire::{Capability, ModelChoice, SettingsView};
use serde::{Deserialize, Serialize};

/// The file the choices are kept in, inside the app's own folder. It
/// holds API keys: the app's folder is private to the app on both
/// systems, and the file is readable by nothing else.
const FILE: &str = "local.json";

pub const PROVIDERS: [(&str, &str); 3] = [
    ("codex", "ChatGPT"),
    ("anthropic", "Anthropic"),
    ("hosted", "Hosted"),
];

const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The capabilities that work on a phone, each with the heading it is
/// listed under and its name as a person would say it. The others need a
/// program a phone cannot run: the project's own checks, `gh`, a desktop
/// browser, or the Python services.
const CAPABILITIES: [(Feature, &str, &str); 17] = [
    (Feature::Memory, "Memory", "Memory"),
    (Feature::Lessons, "Memory", "Lessons"),
    (Feature::Priors, "Memory", "Priors"),
    (Feature::Intuition, "Learning", "Intuition"),
    (Feature::Controller, "Learning", "Decision controller"),
    (Feature::Brain, "Learning", "Dendritic network"),
    (Feature::Living, "Learning", "Living network"),
    (Feature::Index, "Understanding the project", "Code index"),
    (Feature::Map, "Understanding the project", "Project map"),
    (Feature::Graph, "Understanding the project", "Code graph"),
    (
        Feature::Router,
        "Understanding the project",
        "Context router",
    ),
    (Feature::Likely, "Understanding the project", "Likely files"),
    (Feature::Lean, "Spending less", "Lean tool output"),
    (Feature::Batch, "Spending less", "Batched lookups"),
    (
        Feature::Review,
        "Checking the work",
        "Review against the request",
    ),
    (Feature::Repro, "Checking the work", "Reproduce first"),
    (Feature::Skills, "Tools", "Skills"),
];

/// On until switched off. The fly brain is left off: it holds 300 MB.
const DEFAULTS: [Feature; 5] = [
    Feature::Memory,
    Feature::Intuition,
    Feature::Living,
    Feature::Index,
    Feature::Map,
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: String,

    /// The model chosen for each provider.
    pub models: BTreeMap<String, String>,

    /// Each provider's API key. ChatGPT has none: it is signed in to.
    pub keys: BTreeMap<String, String>,

    /// The hosted service's address.
    pub address: String,

    pub effort: String,
    pub approve_everything: bool,

    /// As `--with` spells them.
    pub capabilities: Vec<String>,

    /// The folder worked in last.
    pub project: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: "codex".to_string(),
            models: BTreeMap::new(),
            keys: BTreeMap::new(),
            address: String::new(),
            effort: "high".to_string(),
            approve_everything: false,
            capabilities: DEFAULTS.iter().map(|feature| name(*feature)).collect(),
            project: None,
        }
    }
}

impl Settings {
    pub fn load(folder: &Path) -> Self {
        std::fs::read_to_string(folder.join(FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, folder: &Path) -> Result<()> {
        let file = folder.join(FILE);
        let text = serde_json::to_string_pretty(self)?;

        std::fs::write(&file, text).with_context(|| format!("cannot write {}", file.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        }

        Ok(())
    }

    pub fn model(&self) -> String {
        self.models
            .get(&self.provider)
            .cloned()
            .or_else(|| {
                choices(&self.provider)
                    .first()
                    .map(|model| model.to_string())
            })
            .unwrap_or_default()
    }

    pub fn key(&self) -> Option<&str> {
        self.keys
            .get(&self.provider)
            .map(String::as_str)
            .filter(|key| !key.trim().is_empty())
    }

    pub fn has(&self, feature: Feature) -> bool {
        self.capabilities.contains(&name(feature))
    }

    pub fn set_capability(&mut self, capability: &str, on: bool) {
        self.capabilities.retain(|held| held != capability);

        if on
            && CAPABILITIES
                .iter()
                .any(|(feature, ..)| name(*feature) == capability)
        {
            self.capabilities.push(capability.to_string());
        }
    }

    /// The same arguments the `cortex` command takes, so the agent is
    /// built here exactly as it is there.
    pub fn model_args(&self) -> Result<ModelArgs> {
        let mut arguments: Vec<String> = vec![
            "cortex".to_string(),
            "--provider".to_string(),
            self.provider.clone(),
            "--effort".to_string(),
            self.effort.clone(),
            // Nothing a phone can start is confined by the desktop's sandbox.
            "--no-sandbox".to_string(),
        ];

        let model = self.model();

        if !model.is_empty() {
            arguments.extend(["--model".to_string(), model]);
        }

        if self.provider == "hosted" && !self.address.is_empty() {
            arguments.extend(["--base-url".to_string(), self.address.clone()]);
        }

        // The memory space shows the agent's memory, so it is always kept.
        let mut capabilities = self.capabilities.clone();

        if !self.has(Feature::Memory) {
            capabilities.push(name(Feature::Memory));
        }

        for (feature, ..) in CAPABILITIES {
            if capabilities.contains(&name(feature)) {
                arguments.extend(["--with".to_string(), name(feature)]);
            }
        }

        let parsed = ModelOnly::try_parse_from(arguments)
            .context("the saved settings are not ones the agent accepts")?;

        Ok(parsed.model)
    }

    pub fn view(&self) -> SettingsView {
        let models = PROVIDERS
            .iter()
            .flat_map(|(provider, label)| {
                let mut models: Vec<String> = choices(provider)
                    .iter()
                    .map(|model| model.to_string())
                    .collect();

                // One typed in by hand is offered beside the known ones.
                if let Some(chosen) = self.models.get(*provider)
                    && !models.contains(chosen)
                {
                    models.insert(0, chosen.clone());
                }

                models.into_iter().map(move |model| ModelChoice {
                    provider: provider.to_string(),
                    provider_label: label.to_string(),
                    model,
                })
            })
            .collect();

        let capabilities = CAPABILITIES
            .iter()
            .map(|(feature, group, label)| Capability {
                name: name(*feature),
                label: label.to_string(),
                help: help(*feature),
                on: *feature == Feature::Memory || self.has(*feature),
                group: group.to_string(),
            })
            .collect();

        SettingsView {
            provider: self.provider.clone(),
            model: self.model(),
            models,
            effort: self.effort.clone(),
            efforts: EFFORTS.iter().map(|effort| effort.to_string()).collect(),
            takes_effort: self.provider != "hosted",
            approve_everything: self.approve_everything,
            capabilities,
        }
    }
}

/// The models offered for a provider without typing a name.
fn choices(provider: &str) -> &'static [&'static str] {
    match provider {
        "codex" => &["gpt-6.1-sol"],
        "anthropic" => &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
        _ => &[],
    }
}

pub fn is_effort(effort: &str) -> bool {
    EFFORTS.contains(&effort)
}

pub fn is_provider(provider: &str) -> bool {
    PROVIDERS.iter().any(|(known, _)| *known == provider)
}

fn name(feature: Feature) -> String {
    feature
        .to_possible_value()
        .map(|value| value.get_name().to_string())
        .unwrap_or_default()
}

fn help(feature: Feature) -> String {
    let help = feature
        .to_possible_value()
        .and_then(|value| value.get_help().map(|help| help.to_string()))
        .unwrap_or_default();

    // Two switches, one network: said first under both, unless the
    // engine's own words say it already.
    let same = match feature {
        Feature::Brain => {
            "The same network as the Living network: this switch lets it advise the agent, \
             that one only lets it watch and grow."
        }

        Feature::Living => {
            "The same network as the Dendritic network: this switch only lets it watch and \
             grow, that one lets it advise the agent."
        }

        _ => return help,
    };

    let lower = help.to_lowercase();

    if lower.contains("same network as") || lower.contains("same dendritic network") {
        return help;
    }

    if help.is_empty() {
        same.to_string()
    } else {
        format!("{same} {help}")
    }
}

/// Lets the `cortex` command's model arguments be parsed on their own.
#[derive(Parser)]
struct ModelOnly {
    #[command(flatten)]
    model: ModelArgs,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_settings_build_arguments_the_agent_accepts() {
        let arguments = Settings::default().model_args().unwrap();

        assert!(arguments.features.contains(&Feature::Memory));
        assert!(arguments.features.contains(&Feature::Intuition));
        assert!(arguments.no_sandbox);
    }

    #[test]
    fn the_two_switches_of_the_one_network_say_so() {
        let view = Settings::default().view();

        for name in ["brain", "living"] {
            let capability = view.capabilities.iter().find(|it| it.name == name).unwrap();
            let help = capability.help.to_lowercase();

            assert!(
                help.contains("same network as") || help.contains("same dendritic network"),
                "{name}: {}",
                capability.help
            );
        }
    }

    #[test]
    fn a_capability_a_phone_cannot_run_is_not_switched_on() {
        let mut settings = Settings::default();

        settings.set_capability("gate", true);
        settings.set_capability("graph", true);

        assert!(!settings.capabilities.contains(&"gate".to_string()));
        assert!(settings.has(Feature::Graph));
    }
}

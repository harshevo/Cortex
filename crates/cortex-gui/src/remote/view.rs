//! A chat's settings as a phone sees and changes them.

use clap::ValueEnum;
use cortex_cli::features::Feature;
use cortex_remote::wire::{Capability, ModelChoice, SettingsView};

use crate::settings::{self, Effort, Provider, Settings};

pub fn settings_view(settings: &Settings) -> SettingsView {
    let models = Provider::ALL
        .into_iter()
        .flat_map(|provider| {
            settings
                .of(provider)
                .choices()
                .into_iter()
                .map(move |model| ModelChoice {
                    provider: provider.argument().to_string(),
                    provider_label: provider.label().to_string(),
                    model,
                })
        })
        .collect();

    let capabilities = Feature::value_variants()
        .iter()
        .map(|feature| {
            let about = settings::capabilities::about(*feature);

            Capability {
                name: about.name,
                label: about.label.into_owned(),
                help: settings::capability_help(*feature),

                // The memory space shows the agent's memory, so it is always kept.
                on: *feature == Feature::Memory || settings.has(*feature),
                group: about.group.title().to_string(),
            }
        })
        .collect();

    SettingsView {
        provider: settings.provider.argument().to_string(),
        model: settings.current().model.clone(),
        models,
        effort: settings.effort.argument().to_string(),
        efforts: Effort::ALL
            .iter()
            .map(|effort| effort.argument().to_string())
            .collect(),
        takes_effort: settings.provider.takes_effort(),
        approve_everything: settings.approve_everything,
        capabilities,
    }
}

pub fn provider(name: &str) -> Option<Provider> {
    Provider::ALL
        .into_iter()
        .find(|provider| provider.argument() == name)
}

pub fn effort(name: &str) -> Option<Effort> {
    Effort::ALL
        .into_iter()
        .find(|effort| effort.argument() == name)
}

pub fn feature(name: &str) -> Option<Feature> {
    Feature::value_variants()
        .iter()
        .copied()
        .find(|feature| settings::capability_name(*feature) == name)
}

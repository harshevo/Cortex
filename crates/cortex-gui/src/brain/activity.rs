//! What the learning networks are doing, for the inspector beside the
//! brain picture: the dendritic network's own account of itself, and the
//! intuition's.

use cortex_brain::intuition::Picture;
use cortex_living::{HunchKind, Snapshot};
use eframe::egui;
use serde_json::Value;

use crate::theme::palette;

#[derive(Default)]
pub struct ActivityView {
    /// The engine's answer to the last question asked of an adviser.
    pub inspection: Vec<Value>,

    /// A question for the engine's advisers, waiting to be sent.
    pub request: Option<Value>,

    /// The dendritic network as it was when last looked at.
    pub network: Option<Snapshot>,

    pub performance: String,
    pub intuition: Picture,
    pub active: Vec<Value>,
    pub hunches: Vec<Value>,
    pub revision: u64,
}

impl ActivityView {
    /// Takes in the engine's answer to a request made here.
    pub fn inspected(&mut self, states: Vec<Value>) {
        self.inspection = states;
    }

    /// Takes in the network as it is now. Returns whether anything in it
    /// has changed since it was last looked at.
    pub fn look(&mut self, snapshot: Snapshot) -> bool {
        let same = self.network.as_ref().is_some_and(|known| {
            known.moment == snapshot.moment
                && known.resting == snapshot.resting
                && known.meters.replays == snapshot.meters.replays
        });

        if same {
            return false;
        }

        self.network = Some(snapshot);
        self.revision += 1;

        true
    }

    pub fn observe(&mut self, source: &str, detail: &Value) {
        if source == "intuition"
            && let Ok(picture) = serde_json::from_value::<Picture>(detail["picture"].clone())
            && picture.neurons.len() <= 60
            && picture.synapses.len() <= 2880
            && picture.synapses.iter().all(|(a, b, w)| {
                *a < picture.neurons.len() && *b < picture.neurons.len() && w.is_finite()
            })
        {
            self.intuition = picture;
            self.active = detail["active"]
                .as_array()
                .map(|active| active.iter().take(96).cloned().collect())
                .unwrap_or_default();
            self.hunches = detail["hunches"]
                .as_array()
                .map(|hunches| hunches.iter().take(3).cloned().collect())
                .unwrap_or_default();
            self.revision += 1;
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("Dendritic network");

        if !self.performance.is_empty() {
            ui.small(&self.performance);
        }

        match &self.network {
            None => {
                ui.label(
                    "Turn on Brain (it advises) or Living (it only watches) under Settings → \
                     Capabilities to see it grow.",
                );
            }

            Some(snapshot) => network(ui, snapshot),
        }

        ui.separator();

        ui.label(format!(
            "Intuition: {} concepts · {} synapses · {} runs",
            self.intuition.total_neurons, self.intuition.total_synapses, self.intuition.runs
        ));

        ui.small(format!(
            "{} active concepts · {} hunches",
            self.active.len(),
            self.hunches.len()
        ));

        for hunch in self.hunches.iter().take(3) {
            ui.label(format!(
                "{} ({:.2})",
                hunch["text"].as_str().unwrap_or("association"),
                hunch["strength"].as_f64().unwrap_or_default()
            ));
        }
    }
}

fn network(ui: &mut egui::Ui, snapshot: &Snapshot) {
    let meters = &snapshot.meters;

    ui.label(format!(
        "{} neurons · {} branches · {} synapses",
        meters.neurons, meters.segments, meters.synapses
    ));

    ui.small(format!(
        "{} moments lived · {} replayed at rest{}",
        meters.moments,
        meters.replays,
        if snapshot.resting {
            " · resting now"
        } else {
            ""
        }
    ));

    ui.small(format!(
        "Expected {} of each moment (guessing the commonest: {}) · {} of what it expected came",
        percent(meters.expected_right),
        percent(meters.baseline_right),
        percent(meters.precision),
    ));

    ui.small(format!(
        "Set by itself: learning rate ×{:.2} · a branch counts from {} sure · advises from {} sure",
        meters.plasticity,
        percent(meters.threshold),
        percent(meters.caution),
    ));

    if meters.advised > 0 || meters.held_back > 0 {
        ui.small(format!(
            "Advised {} times, {} followed by what it said · kept quiet {} times where it \
             expected to be wrong",
            meters.advised,
            percent(meters.advice_right),
            meters.held_back,
        ));
    }

    section(ui, "Expects next");

    if snapshot.reasons.is_empty() {
        ui.small("Nothing it is sure enough of.");
    }

    for reason in &snapshot.reasons {
        ui.label(format!(
            "{} · {} sure",
            reason.expected,
            percent(reason.confidence)
        ));

        ui.small(format!(
            "because of {} · right {} of {} times",
            reason.because.join(" + "),
            reason.hits,
            reason.hits + reason.misses
        ));
    }

    section(ui, "Imagines from here");

    if snapshot.imagined.is_empty() {
        ui.small("No further than the next moment.");
    }

    for (step, thought) in snapshot.imagined.iter().enumerate() {
        let worth = if thought.worth > 0.05 {
            " · good news"
        } else if thought.worth < -0.05 {
            " · bad news"
        } else {
            ""
        };

        ui.small(format!(
            "{}. {} · {} sure{worth}",
            step + 1,
            thought.labels.join(", "),
            percent(thought.confidence)
        ));
    }

    if let Some(goal) = &snapshot.goal
        && !snapshot.plan.is_empty()
    {
        section(ui, &format!("How `{goal}` has been reached"));

        for (step, labels) in snapshot.plan.iter().enumerate() {
            ui.small(format!("{}. {}", step + 1, labels.join(", ")));
        }
    }

    if let Some(hunch) = &snapshot.hunch {
        section(
            ui,
            match hunch.kind {
                HunchKind::Next => "Last said: a next step",
                HunchKind::Warning => "Last said: a warning",
            },
        );

        ui.small(cortex_living::say(hunch));
    }

    section(ui, "In mind");

    ui.small(if snapshot.workspace.is_empty() {
        "Nothing held.".to_string()
    } else {
        snapshot.workspace.join(" · ")
    });

    ui.add_space(4.0);

    ui.small(
        egui::RichText::new(
            "Each number measures the mechanism named. None of it is evidence of experience \
             or awareness, and whether its advice helps a model has not been measured.",
        )
        .color(palette::text_muted()),
    );
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong());
}

fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.0)
}

#[cfg(test)]
mod tests {
    use cortex_living::Network;

    use super::*;

    #[test]
    fn the_network_is_taken_in_only_when_it_has_changed() {
        let mut network = Network::new();
        let mut view = ActivityView::default();

        network.live(&["does: edit_file".to_string()]);

        assert!(view.look(network.snapshot()));
        assert!(!view.look(network.snapshot()));

        network.live(&["result: worked".to_string()]);

        assert!(view.look(network.snapshot()));
        assert_eq!(view.revision, 2);
    }

    #[test]
    fn a_malformed_picture_of_the_intuition_is_ignored() {
        let mut view = ActivityView::default();

        view.observe(
            "intuition",
            &serde_json::json!({"picture": {"neurons": [], "synapses": [[0, 99, 1.0]]}}),
        );

        assert_eq!(view.revision, 0);
    }
}

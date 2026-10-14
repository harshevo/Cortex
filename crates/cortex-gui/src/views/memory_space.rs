//! What the app keeps of the agent's memory now that it is no longer
//! drawn on a screen of its own: how many tasks are remembered and how
//! they went, and the shared brain as last copied for the network view.

use crate::memory::Remembered;

#[derive(Default)]
pub struct MemoryView {
    /// How many episodes the engine holds.
    episodes: usize,

    /// One line on the remembered tasks, for the network view.
    pub performance: String,

    /// The shared brain as it was when last looked at.
    pub network: Option<cortex_living::Snapshot>,

    /// Why the engine's memory could not be read.
    pub problem: Option<String>,
}

impl MemoryView {
    pub fn episode_count(&self) -> usize {
        self.episodes
    }

    /// Takes in what the engine remembers now.
    pub fn load(&mut self, remembered: Remembered) {
        self.episodes = remembered.total_episodes.max(remembered.episodes.len());
        let checked = remembered
            .episodes
            .iter()
            .filter(|e| e.verified.is_some())
            .count();
        let passed = remembered
            .episodes
            .iter()
            .filter(|e| e.verified == Some(true))
            .count();
        self.performance = if checked == 0 {
            format!("{} remembered tasks, none checked yet", self.episodes)
        } else {
            format!(
                "{} remembered tasks; checks passed in {passed} of {checked} checked",
                self.episodes
            )
        };
        self.problem = None;
    }

    /// Takes in the shared brain as it is now, or that there is none.
    pub fn look_at_network(&mut self, snapshot: Option<cortex_living::Snapshot>) {
        self.network = snapshot;
    }
}

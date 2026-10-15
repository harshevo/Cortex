//! What the app keeps of the agent's memory now that it is no longer
//! drawn on a screen of its own: how many tasks are remembered.

use crate::memory::Remembered;

#[derive(Default)]
pub struct MemoryView {
    /// How many episodes the engine holds.
    episodes: usize,

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
        self.problem = None;
    }
}

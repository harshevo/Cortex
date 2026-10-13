//! Experimental: a network that grows and rewires itself as it watches
//! the agent work.
//!
//! It is fed the same events as the rest of the agent and nothing it
//! learns reaches the model: it is a [`TrajectorySink`] and nothing else,
//! so it can be as wrong as it likes while it is being studied. What it
//! holds is kept in one file for all projects, and drawn live by the app.
//!
//! - [`network`] is the network itself and says what each mechanism is.
//! - [`percept`] reads a run's events as the moments the network lives.

pub mod network;
pub mod percept;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use cortex_core::{Event, TrajectorySink};

pub use network::{Id, Kind, Meters, Network, NeuronView, Sample, Snapshot};

/// How long nothing must have happened before the network rests.
const QUIET: Duration = Duration::from_secs(3);

pub struct Living {
    /// Where the network is kept. `None` keeps it in memory only.
    file: Option<PathBuf>,

    state: Mutex<State>,
}

struct State {
    network: Network,
    last_event: Instant,

    /// Rests since the network was last saved.
    unsaved_rests: u32,
}

impl Living {
    /// A network of its own, loaded from `file` when there is one.
    pub fn open(file: Option<PathBuf>) -> Self {
        // A default network has not been given its own states yet.
        #[allow(clippy::unwrap_or_default)]
        let network = file.as_deref().and_then(load).unwrap_or_else(Network::new);

        Self {
            file,
            state: Mutex::new(State {
                network,
                last_event: Instant::now(),
                unsaved_rests: 0,
            }),
        }
    }

    /// The one network kept in `file`, shared by everything in this
    /// process that asks for it, so that several chats feed and show the
    /// same one.
    pub fn shared(file: &Path) -> Arc<Self> {
        static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Living>>>> = OnceLock::new();

        let mut open = OPEN
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(living) = open.get(file).and_then(Weak::upgrade) {
            return living;
        }

        let living = Arc::new(Self::open(Some(file.to_path_buf())));

        open.insert(file.to_path_buf(), Arc::downgrade(&living));

        living
    }

    /// The network as it is now, for drawing.
    pub fn snapshot(&self) -> Snapshot {
        self.state().network.snapshot()
    }

    /// Lets the network go over recent moments, if nothing has happened
    /// for a while. Returns whether it did.
    pub fn rest(&self) -> bool {
        let mut state = self.state();

        if state.last_event.elapsed() < QUIET || !state.network.rest() {
            return false;
        }

        state.unsaved_rests += 1;

        if state.unsaved_rests >= 10 {
            state.unsaved_rests = 0;

            self.save(&state.network);
        }

        true
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, network: &Network) {
        let Some(file) = &self.file else {
            return;
        };

        if let Err(error) = store(file, network) {
            eprintln!("warning: cannot save {}: {error}", file.display());
        }
    }
}

impl TrajectorySink for Living {
    fn record(&self, event: &Event) {
        let moments = percept::moments(event);

        if moments.is_empty() {
            return;
        }

        let mut state = self.state();

        for percepts in &moments {
            state.network.live(percepts);
        }

        state.last_event = Instant::now();

        if matches!(event, Event::RunFinished { .. }) {
            self.save(&state.network);
        }
    }
}

fn load(file: &Path) -> Option<Network> {
    let text = fs::read_to_string(file).ok()?;

    let mut network: Network = serde_json::from_str(&text)
        .map_err(|error| {
            eprintln!(
                "warning: {} is not a network, starting a new one: {error}",
                file.display()
            );
        })
        .ok()?;

    network.wake();

    Some(network)
}

/// Written beside the file and moved over it, so that a crash half-way
/// leaves the old network and not half of a new one.
fn store(file: &Path, network: &Network) -> std::io::Result<()> {
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }

    let partial = file.with_extension("partial");

    fs::write(&partial, serde_json::to_string(network)?)?;

    fs::rename(&partial, file)
}

#[cfg(test)]
mod tests {
    use cortex_core::{Outcome, Usage};

    use super::*;

    fn started(instruction: &str) -> Event {
        Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
        }
    }

    fn finished() -> Event {
        Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 1,
            usage: Usage::default(),
            duration_ms: 1,
            verified: Some(true),
        }
    }

    #[test]
    fn a_run_grows_the_network_and_it_is_kept() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("living").join("network.json");

        let living = Living::open(Some(file.clone()));

        assert_eq!(living.snapshot().meters.grown, 0);

        living.record(&started("fix the rounding of commission fees"));
        living.record(&finished());

        let lived = living.snapshot();

        assert!(lived.meters.grown >= 4, "{:?}", lived.meters);
        assert!(lived.synapses.iter().any(|(_, _, weight)| *weight > 0.0));

        let again = Living::open(Some(file)).snapshot();

        assert_eq!(again.moment, lived.moment);
        assert_eq!(again.neurons.len(), lived.neurons.len());
    }

    #[test]
    fn everything_in_one_process_shares_the_network_of_a_file() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("network.json");

        let one = Living::shared(&file);
        let other = Living::shared(&file);

        one.record(&started("fix the rounding"));

        assert_eq!(other.snapshot().moment, 1);
    }

    #[test]
    fn it_does_not_rest_while_things_are_happening() {
        let living = Living::open(None);

        living.record(&started("fix the rounding"));

        assert!(!living.rest());
    }
}

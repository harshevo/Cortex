//! Native desktop connection to the continuously running circuit over Unix IPC.

use cortex_brain::circuit::ipc::{self, Request};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

/// How often the live record is read.
const LIVE: Duration = Duration::from_millis(100);

/// How often the full state is read again, between questions.
const FULL: Duration = Duration::from_secs(5);

/// How many steps are kept for the view to play back.
const KEPT_FRAMES: usize = 4096;

/// How long a question may take: generating English runs the circuit
/// once per token.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// The circuit as the brain view shows it.
#[derive(Debug, Clone, Default)]
pub struct CircuitState {
    pub adaptation: Value,
    pub self_monitor: Value,
    pub topology: Topology,
    pub core_trained: bool,
    pub training_steps: u64,
    pub updates: u64,
    pub synapses: u64,
    pub plastic_weights: u64,
    pub learned_decoder_parameters: u64,
    pub language_tokens: Option<u64>,
    pub threshold: f32,
    pub programs: Vec<String>,
    pub sources: Vec<(String, u64)>,
    pub curiosity: Vec<String>,
    pub observations: Vec<Observation>,
    pub method: Option<String>,
    pub device: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Topology {
    pub neurons: u64,
    pub dendrites: u64,
    pub branches_per_excitatory_neuron: u64,
    pub branches_min: u64,
    pub branches_max: u64,
    pub recurrent_synapses: u64,
    pub input_synapses: u64,
    pub recurrent_fan_in: u64,
    pub input_fan_in: u64,
    pub structure: String,
    pub online_learning: String,
    pub growth: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Activity {
    #[serde(default)]
    pub spikes: u64,
    #[serde(default)]
    pub dropped_events: u64,
    #[serde(default)]
    pub active_branches: u64,
    #[serde(default)]
    pub dendrites: u64,
    #[serde(default)]
    pub branch_voltage: Vec<f32>,
    #[serde(default)]
    pub branch_gain: Vec<f32>,
    #[serde(default)]
    pub branch_plateaus: Vec<u64>,
    #[serde(default)]
    pub active_neurons: u64,
    #[serde(default)]
    pub mean_voltage: f32,
    #[serde(default)]
    pub max_voltage: f32,
    #[serde(default)]
    pub pending_events: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Observation {
    pub question: String,
    pub response: Answer,
}

/// The circuit's answer to a question.
#[derive(Debug, Clone, Deserialize)]
pub struct Answer {
    pub text: String,
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub abstained: bool,
    #[serde(default)]
    pub candidates: Vec<(String, f32)>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// What came back from a request the person made.
#[derive(Debug, Clone)]
pub enum Reply {
    Answer(Answer),

    /// English generated token by token by the circuit's language head.
    English {
        text: String,
        note: String,
        method: String,
        feedback_supported: bool,
    },

    /// A lesson saved or memories replayed.
    Done(String),
}

/// What the person asks the circuit to do.
#[derive(Debug, Clone)]
pub enum Ask {
    Chat(String),
    English(String),
    Teach { text: String, program: String },
    Replay,
    Feedback(f32),
    Activity(bool),
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Background {
    pub enabled: bool,
    pub cycles: u64,
    pub phase: String,
    pub error: Option<String>,
}

/// What the circuit did on its latest steps, as read from its live record.
#[derive(Debug, Clone, Default)]
pub struct Pulse {
    /// The latest step read.
    pub frame: u64,

    /// Each neuron's membrane voltage after it.
    pub voltages: Vec<f32>,

    /// Each neuron's population, an index into `populations`.
    pub population: Vec<u8>,
    pub populations: Vec<String>,
    pub activity: Activity,
    pub background: Background,
}

#[derive(Default)]
struct Shared {
    state: Option<CircuitState>,
    pulse: Option<Pulse>,

    /// Recent steps: their number and the neurons that spiked.
    frames: std::collections::VecDeque<(u64, Vec<u32>)>,
    problem: Option<String>,
    read: bool,
}

/// The app's handle on the brain's service.
pub struct Circuit {
    shared: Arc<Mutex<Shared>>,
    started: AtomicBool,

    /// Set to read the full state again at once, after a question.
    refresh: Arc<AtomicBool>,
}

impl Default for Circuit {
    fn default() -> Self {
        Self {
            shared: Arc::default(),
            started: AtomicBool::new(false),
            refresh: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl Circuit {
    /// Starts reading the brain in the background, once.
    pub fn start(&self, context: &eframe::egui::Context) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }

        let shared = Arc::downgrade(&self.shared);
        let refresh = Arc::clone(&self.refresh);
        let context = context.clone();

        let _ = std::thread::Builder::new()
            .name("brain-live".into())
            .spawn(move || {
                let mut full_at: Option<Instant> = None;
                let mut since = 0;
                while let Some(shared) = shared.upgrade() {
                    let live =
                        call(&Request::Live { since }, Duration::from_secs(2)).and_then(pulse);
                    let mut changed;

                    match live {
                        Ok((pulse, frames)) => {
                            // A restarted service counts its steps from zero.
                            if pulse.frame < since {
                                shared
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .frames
                                    .clear();
                                since = 0;
                            }
                            changed = !frames.is_empty() || since == 0;
                            since = pulse.frame;
                            let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
                            shared.frames.extend(frames);
                            while shared.frames.len() > KEPT_FRAMES {
                                shared.frames.pop_front();
                            }
                            shared.pulse = Some(pulse);
                            shared.problem = None;
                            shared.read = true;
                        }
                        Err(problem) => {
                            let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
                            shared.problem = Some(problem);
                            shared.read = true;
                            changed = true;
                        }
                    }

                    // The full state, between questions or right after one.
                    if refresh.swap(false, Ordering::Relaxed)
                        || full_at.is_none_or(|at| at.elapsed() >= FULL)
                    {
                        full_at = Some(Instant::now());
                        if let Ok(state) =
                            call(&Request::State, Duration::from_secs(10)).and_then(boil_down)
                        {
                            shared.lock().unwrap_or_else(|e| e.into_inner()).state = Some(state);
                            changed = true;
                        }
                    }

                    if changed {
                        context.request_repaint();
                    }
                    drop(shared);
                    std::thread::sleep(LIVE);
                }
            });
    }

    /// The latest full state and live reading, and why the brain could not
    /// be reached if it could not.
    pub fn state(&self) -> (Option<CircuitState>, Option<Pulse>, Option<String>) {
        let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());

        (
            shared.state.clone(),
            shared.pulse.clone(),
            shared.problem.clone(),
        )
    }

    /// The steps read after step `after`, oldest first.
    pub fn frames_after(&self, after: u64) -> Vec<(u64, Vec<u32>)> {
        let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());

        shared
            .frames
            .iter()
            .filter(|(n, _)| *n > after)
            .cloned()
            .collect()
    }

    /// Whether the brain has been tried yet.
    pub fn has_read(&self) -> bool {
        self.shared.lock().map(|s| s.read).unwrap_or(false)
    }

    /// Sends a request on a thread of its own; the reply arrives on the
    /// returned channel, and the full state is read again after it.
    pub fn ask(&self, ask: Ask) -> Receiver<Result<Reply, String>> {
        let (send, receive) = channel();
        let refresh = Arc::clone(&self.refresh);

        let spawned = std::thread::Builder::new().name("brain-ask".into()).spawn({
            let send = send.clone();
            move || {
                let reply = run(ask);
                refresh.store(true, Ordering::Relaxed);
                let _ = send.send(reply);
            }
        });

        if let Err(error) = spawned {
            let _ = send.send(Err(error.to_string()));
        }

        receive
    }
}

/// Reads a reply from the live record.
type SpikeFrames = Vec<(u64, Vec<u32>)>;

fn pulse(value: Value) -> Result<(Pulse, SpikeFrames), String> {
    #[derive(Deserialize)]
    struct Reading {
        frame: u64,
        #[serde(default)]
        background_activity: Background,
        #[serde(default)]
        frames: Vec<(u64, Vec<u32>)>,
        #[serde(default)]
        voltages: Vec<i32>,
        #[serde(default)]
        activity: Activity,
        #[serde(default)]
        populations: Vec<String>,
    }

    let reading: Reading =
        serde_json::from_value(value).map_err(|e| format!("Unexpected live reading: {e}"))?;
    let mut names: Vec<String> = Vec::new();
    let population = reading
        .populations
        .into_iter()
        .map(|name| {
            let at = names.iter().position(|n| *n == name).unwrap_or_else(|| {
                names.push(name);
                names.len() - 1
            });
            at.min(u8::MAX as usize) as u8
        })
        .collect();

    Ok((
        Pulse {
            frame: reading.frame,
            voltages: reading
                .voltages
                .into_iter()
                .map(|v| v as f32 / 1000.0)
                .collect(),
            population,
            populations: names,
            activity: reading.activity,
            background: reading.background_activity,
        },
        reading.frames,
    ))
}

fn run(ask: Ask) -> Result<Reply, String> {
    match ask {
        Ask::Activity(enabled) => {
            call(&Request::Activity { enabled }, ANSWER_TIMEOUT)?;
            Ok(Reply::Done(
                if enabled {
                    "Background activity resumed."
                } else {
                    "Background activity paused."
                }
                .into(),
            ))
        }
        Ask::Chat(text) => {
            let value = call(&Request::Chat { text }, ANSWER_TIMEOUT)?;
            serde_json::from_value(value)
                .map(Reply::Answer)
                .map_err(|e| e.to_string())
        }
        Ask::English(text) => {
            let value = call(&Request::Dialogue { text }, ANSWER_TIMEOUT)?;
            Ok(Reply::English {
                text: value["text"].as_str().unwrap_or_default().to_string(),
                note: value["scope"].as_str().unwrap_or_default().to_string(),
                method: value["method"].as_str().unwrap_or_default().to_string(),
                feedback_supported: value["feedback_supported"].as_bool().unwrap_or(true),
            })
        }
        Ask::Teach { text, program } => {
            let value = call(
                &Request::Teach {
                    text,
                    label: program,
                },
                ANSWER_TIMEOUT,
            )?;
            Ok(Reply::Done(format!(
                "Learned. {} updates in all.",
                value["updates"].as_u64().unwrap_or(0)
            )))
        }
        Ask::Feedback(reward) => {
            call(&Request::Feedback { reward }, ANSWER_TIMEOUT)?;
            Ok(Reply::Done("Feedback learned and saved.".into()))
        }
        Ask::Replay => {
            let value = call(&Request::Replay, ANSWER_TIMEOUT)?;
            Ok(Reply::Done(format!(
                "Replayed {} lessons.",
                value["replayed"].as_u64().unwrap_or(0)
            )))
        }
    }
}

fn call(request: &Request, timeout: Duration) -> Result<Value, String> {
    ipc::call(&ipc::default_socket(), request, timeout)
}

/// Keeps what the view draws from the full state.
fn boil_down(value: Value) -> Result<CircuitState, String> {
    #[derive(Deserialize)]
    struct Full {
        #[serde(default)]
        adaptation: Value,
        #[serde(default)]
        self_monitor: Value,
        #[serde(default)]
        topology: Topology,
        #[serde(default)]
        updates: u64,
        #[serde(default)]
        synapses: u64,
        #[serde(default)]
        plastic_weights: u64,
        #[serde(default)]
        learned_decoder_parameters: u64,
        #[serde(default)]
        language_tokens: Option<u64>,
        #[serde(default)]
        threshold: f32,
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default)]
        sources: std::collections::BTreeMap<String, u64>,
        #[serde(default)]
        curiosity: Vec<String>,
        #[serde(default)]
        observations: Vec<Observation>,
        #[serde(default)]
        plasticity: Option<Value>,
    }

    let full: Full =
        serde_json::from_value(value).map_err(|e| format!("Unexpected circuit state: {e}"))?;
    let mut sources: Vec<_> = full.sources.into_iter().collect();
    sources.sort_by_key(|a| std::cmp::Reverse(a.1));

    Ok(CircuitState {
        adaptation: full.adaptation,
        self_monitor: full.self_monitor,
        topology: full.topology,
        core_trained: full.plasticity.as_ref().is_some_and(|p| {
            p["recurrent_trained"].as_bool() == Some(true)
                || p["method"].as_str() == Some("surrogate_bptt")
        }),
        training_steps: full
            .plasticity
            .as_ref()
            .and_then(|p| p["steps"].as_u64())
            .unwrap_or(0),
        updates: full.updates,
        synapses: full.synapses,
        plastic_weights: full.plastic_weights,
        learned_decoder_parameters: full.learned_decoder_parameters,
        language_tokens: full.language_tokens,
        threshold: full.threshold,
        programs: full.labels,
        sources,
        curiosity: full.curiosity,
        observations: full.observations,
        method: full
            .plasticity
            .as_ref()
            .and_then(|p| p["method"].as_str())
            .map(str::to_string),
        device: full
            .plasticity
            .as_ref()
            .and_then(|p| p["device"].as_str())
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_is_boiled_down_to_what_is_drawn() {
        let state = boil_down(serde_json::json!({
            "updates": 7,
            "plasticity": {"method":"readout_only", "recurrent_trained":true, "steps":800},
            "topology": {"neurons":1792,"dendrites":5888,"recurrent_synapses":86016,"input_synapses":14336,"branches_per_excitatory_neuron":4,"structure":"Fixed topology"},
            "labels": ["(a+b)", "unknown"],
            "sources": {"gsm8k": 3, "dolly": 9},
            "activity": {"active_neurons": 2, "ticks": 80},
            "neurons": [
                {"population": "Context", "voltage": 0.5},
                {"population": "Value", "voltage": -0.2},
                {"population": "Context", "voltage": 1.0}
            ],
            "observations": [{"question": "1 + 3?", "response": {"text": "4", "program": "(a+b)", "confidence": 0.9, "abstained": false}}]
        }))
        .unwrap();

        assert!(state.core_trained);
        assert_eq!(state.method.as_deref(), Some("readout_only"));
        assert_eq!(state.training_steps, 800);
        assert_eq!(state.topology.dendrites, 5888);
        assert_eq!(
            state.topology.recurrent_synapses + state.topology.input_synapses,
            100352
        );
        assert_eq!(state.updates, 7);
        assert_eq!(state.programs, ["(a+b)", "unknown"]);
        assert_eq!(state.sources[0], ("dolly".to_string(), 9));
        assert_eq!(state.observations[0].response.text, "4");
    }

    #[test]
    fn a_live_reading_gives_steps_voltages_and_populations() {
        let (pulse, frames) = pulse(serde_json::json!({
            "frame": 12,
            "frames": [[11, [0, 2]], [12, []]],
            "voltages": [500, -200, 1000],
            "activity": {"spikes": 2,"dendrites":8,"branch_voltage":[0.1,0.2,0.3,0.4],"branch_plateaus":[1,0,2,0],"branch_gain":[1.0,1.0,1.2,1.0]},
            "populations": ["Context", "Value", "Context"]
        }))
        .unwrap();

        assert_eq!(pulse.activity.dendrites, 8);
        assert_eq!(pulse.activity.branch_plateaus, [1, 0, 2, 0]);
        assert_eq!(pulse.activity.branch_voltage, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(pulse.frame, 12);
        assert_eq!(frames, [(11, vec![0, 2]), (12, vec![])]);
        assert_eq!(pulse.voltages, [0.5, -0.2, 1.0]);
        assert_eq!(pulse.population, [0, 1, 0]);
        assert_eq!(pulse.populations, ["Context", "Value"]);
    }
}

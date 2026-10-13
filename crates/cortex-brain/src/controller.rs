//! The decision controller: watches a run, learns from its outcome, and
//! advises on the next action.
//!
//! One run goes like this:
//!
//! 1. Before each model call the controller encodes the situation.
//! 2. When the model responds, the action it chose is recorded beside that
//!    code.
//! 3. When the run ends with a checked result, every recorded action is
//!    rewarded: 1 for a run whose checks passed, 0 for one whose checks
//!    failed, discounted by how many steps were still to come. An action
//!    that was followed quickly by passing checks is worth the most.
//! 4. On later runs, when one action's expected reward clearly beats the
//!    rest, the controller says so.
//!
//! Without a checked result (no verification gate) a run teaches nothing,
//! because there is no reward to learn from.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use cortex_core::{Advice, Adviser, Event, TrajectorySink};
use serde_json::json;

use crate::accumulator::{self, Decision};
use crate::learner::{self, Learner};
use crate::situation::{Action, FEATURES, Situation};
use crate::spiking::Brain;

/// Each further step before the end multiplies an action's reward by this.
const DISCOUNT: f32 = 0.93;

/// Checked runs the controller must have learned from before it speaks.
const MIN_RUNS: u32 = 5;

/// A race that takes longer than this was too close to act on.
const MAX_DECISION_STEPS: u32 = 60;

/// Evidence taken from an action for each step the run has been stuck on
/// it, so that a loop loses the race even if the action is usually good.
const REPEAT_PENALTY: f32 = 0.15;

/// Turns a situation's features into the code the learner sees.
pub trait Encoder: Send {
    fn inspect(&mut self, _query: &serde_json::Value) -> Option<serde_json::Value> {
        None
    }
    fn telemetry(&self) -> Option<serde_json::Value> {
        None
    }
    fn name(&self) -> &'static str;

    fn dimension(&self) -> usize;

    fn encode(&mut self, features: &[f32; FEATURES]) -> Vec<f32>;
}

/// The baseline: the features themselves.
pub struct DirectEncoder;

impl Encoder for DirectEncoder {
    fn name(&self) -> &'static str {
        "baseline controller"
    }

    fn dimension(&self) -> usize {
        FEATURES
    }

    fn encode(&mut self, features: &[f32; FEATURES]) -> Vec<f32> {
        features.to_vec()
    }
}

/// The fly-scale spiking network: the code is the activity of its
/// read-out neurons.
pub struct FlyBrainEncoder {
    brain: Brain,
}

impl FlyBrainEncoder {
    pub fn new(brain: Brain) -> Self {
        Self { brain }
    }
}

impl Encoder for FlyBrainEncoder {
    fn inspect(&mut self, query: &serde_json::Value) -> Option<serde_json::Value> {
        if let Some(limit) = query["sample_count"].as_u64() {
            if let Err(error) = self.brain.set_sample_limit(limit as usize) {
                return Some(json!({"error":error}));
            }
            return Some(json!({"source":"fly-brain", "trace":self.brain.trace()}));
        }
        Some(self.brain.inspect(query))
    }
    fn telemetry(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self.brain.trace()).ok()
    }
    fn name(&self) -> &'static str {
        "fly-brain controller"
    }

    fn dimension(&self) -> usize {
        self.brain.code_length()
    }

    fn encode(&mut self, features: &[f32; FEATURES]) -> Vec<f32> {
        self.brain.respond(features)
    }
}

struct State {
    encoder: Box<dyn Encoder>,
    learner: Learner,
    situation: Situation,

    /// The situation's code as of the last tool results, waiting for the
    /// model's next choice.
    current: Option<Vec<f32>>,

    /// Each step of the run so far: the code and the action chosen.
    choices: Vec<(Vec<f32>, Action)>,

    /// The last action advised, so the same advice is not repeated on
    /// every step.
    last_advised: Option<Action>,

    step: u32,
    max_steps: u32,
}

pub struct Controller {
    state: Mutex<State>,

    /// Where what has been learned is kept between runs.
    file: Option<PathBuf>,
}

impl Controller {
    /// A controller that starts from what was learned before, if `file`
    /// holds something learned with the same encoder.
    pub fn new(encoder: Box<dyn Encoder>, file: Option<PathBuf>) -> Self {
        let dimension = encoder.dimension();

        let learner = match &file {
            Some(file) => Learner::load_or_new(file, dimension),
            None => Learner::new(dimension),
        };

        Self {
            state: Mutex::new(State {
                encoder,
                learner,
                situation: Situation::default(),
                current: None,
                choices: Vec::new(),
                last_advised: None,
                step: 0,
                max_steps: 60,
            }),
            file,
        }
    }

    /// Checked runs learned from so far.
    pub fn runs(&self) -> u32 {
        self.state().learner.runs()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Rewards the run's choices and saves what was learned.
    fn learn(&self, state: &mut State, passed: bool) {
        let _lock = match self
            .file
            .as_ref()
            .map(|file| crate::persistence::lock(file))
            .transpose()
        {
            Ok(lock) => lock,
            Err(error) => {
                eprintln!("warning: cannot lock controller learning: {error}");
                return;
            }
        };
        if let Some(file) = &self.file {
            state.learner = Learner::load_or_new(file, state.encoder.dimension());
        }
        let reward = if passed { 1.0 } else { 0.0 };

        let choices = std::mem::take(&mut state.choices);
        let last = choices.len().saturating_sub(1);

        for (position, (code, action)) in choices.iter().enumerate() {
            let steps_to_go = (last - position) as i32;

            state
                .learner
                .learn(code, *action, reward * DISCOUNT.powi(steps_to_go));
        }

        state.learner.finish_run();

        if let Some(file) = &self.file
            && let Err(error) = state.learner.save(file)
        {
            eprintln!("warning: the controller could not save what it learned: {error}");
        }
    }
}

impl State {
    fn encode(&mut self) -> Vec<f32> {
        let features = self.situation.features(self.step, self.max_steps);

        let mut code = self.encoder.encode(&features);

        learner::normalize(&mut code);

        code
    }

    /// The race between actions for the current code.
    fn decide(&self, code: &[f32]) -> ([f32; 5], Decision) {
        let values = self.learner.values(code);

        let mut evidence = values;

        // Finishing is the last step of every run, so in runs that passed
        // it always earned the full reward, wherever it is tried. Its
        // value says that runs end, not when to end one, and whether the
        // work is done is for the gate to judge. It never enters the race.
        evidence[Action::Finish.index()] = 0.0;

        if let Some(action) = self.situation.last_action() {
            evidence[action.index()] -= REPEAT_PENALTY * self.situation.stuck_repeats() as f32;
        }

        (values, accumulator::compete(evidence))
    }
}

impl TrajectorySink for Controller {
    fn record(&self, event: &Event) {
        let mut state = self.state();

        match event {
            Event::ModelStarted { step } => {
                state.step = *step;
            }
            Event::RunStarted { .. } => {
                if let Some(file) = &self.file {
                    state.learner = Learner::load_or_new(file, state.encoder.dimension());
                }
                state.situation = Situation::default();
                state.current = None;
                state.choices.clear();
                state.last_advised = None;
                state.step = 0;
            }

            Event::ModelResponded {
                step,
                message,
                stop_reason,
                ..
            } => {
                state.step = *step;

                let action = Action::of_response(message, stop_reason);

                // The first step has no tool results before it, so its
                // situation is encoded here.
                let code = match state.current.take() {
                    Some(code) => code,
                    None => state.encode(),
                };

                state.choices.push((code, action));
                state.situation.note_action(action);
            }

            Event::RunFinished { verified, .. } => {
                if let Some(passed) = verified {
                    self.learn(&mut state, *passed);
                }
            }

            other => state.situation.observe(other),
        }
    }
}

impl Adviser for Controller {
    fn prepare(&self, step: u32, max_steps: u32) {
        let mut state = self.state();
        state.step = step;
        state.max_steps = max_steps;
        if state.current.is_none() {
            state.current = Some(state.encode());
        }
    }
    fn begin_run(&self, max_steps: u32) {
        self.state().max_steps = max_steps;
    }
    fn inspect(&self, query: &serde_json::Value) -> Option<serde_json::Value> {
        let mut state = self.state();
        let mut detail = state.encoder.inspect(query)?;
        detail["checked_runs"] = state.learner.runs().into();
        detail["runs"] = state.learner.runs().into();
        if let Some(code) = &state.current {
            detail["action_values"] = json!(state.learner.values(code));
        }
        Some(detail)
    }
    fn telemetry(&self) -> Option<serde_json::Value> {
        let state = self.state();
        let trace = state.encoder.telemetry()?;
        let (values, decision) = state
            .current
            .as_ref()
            .or_else(|| state.choices.last().map(|(code, _)| code))
            .map(|code| state.decide(code))
            .unwrap_or(([0.0; 5], accumulator::compete([0.0; 5])));
        Some(json!({
            "source": "fly-brain", "trace": trace, "runs": state.learner.runs(),
            "values": values, "action": decision.action.label(),
            "confident": state.learner.runs() >= MIN_RUNS && decision.decided && decision.steps <= MAX_DECISION_STEPS,
            "race_steps": decision.steps,
        }))
    }
    fn advise(&self, step: u32, max_steps: u32) -> Option<Advice> {
        let mut state = self.state();

        state.step = step;
        state.max_steps = max_steps;

        let code = state.encode();

        let (values, decision) = state.decide(&code);

        state.current = Some(code);

        let confident = decision.decided && decision.steps <= MAX_DECISION_STEPS;

        let is_news = state.last_advised != Some(decision.action);

        if state.learner.runs() < MIN_RUNS || !confident || !is_news {
            return None;
        }

        state.last_advised = Some(decision.action);

        let scores: Vec<String> = Action::ALL
            .iter()
            .map(|action| format!("{} {:.2}", action.label(), values[action.index()]))
            .collect();

        let text = format!(
            "[{}, learned from {} checked runs in this repository] From a state like this one, \
             the runs that reached passing checks soonest went on to {}. Expected reward by \
             next action: {}. This is a statistical hint, not an instruction.",
            state.encoder.name(),
            state.learner.runs(),
            decision.action.phrase(),
            scores.join(", "),
        );

        Some(Advice {
            source: state.encoder.name().to_string(),
            text,
            detail: json!({
                "advised": decision.action.label(),
                "values": values,
                "race_steps": decision.steps,
                "runs": state.learner.runs(),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use cortex_core::{Outcome, Usage};
    use serde_json::json;

    use super::*;
    use crate::situation::tests::{response, tool_events};
    use crate::spiking::BrainConfig;

    #[test]
    fn concurrent_controllers_preserve_every_checked_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brain.json");
        let controllers: Vec<_> = (0..12)
            .map(|_| Controller::new(Box::new(DirectEncoder), Some(path.clone())))
            .collect();
        std::thread::scope(|scope| {
            for controller in controllers {
                scope.spawn(move || {
                    play(&controller, true);
                });
            }
        });
        assert_eq!(
            Controller::new(Box::new(DirectEncoder), Some(path)).runs(),
            12
        );
    }

    fn respond(controller: &Controller, step: u32, calls: &[(&str, serde_json::Value)]) {
        let (message, stop_reason) = response(calls);

        controller.record(&Event::ModelResponded {
            step,
            message,
            stop_reason,
            usage: Usage::default(),
            duration_ms: 1,
        });
    }

    fn tool(controller: &Controller, name: &str, input: serde_json::Value, output: &str) {
        for event in tool_events(name, input, output) {
            controller.record(&event);
        }
    }

    /// Plays one run: read, edit, then either check the work before
    /// finishing (and pass) or finish at once (and fail). Returns the
    /// advice given right after the edit.
    fn play(controller: &Controller, checks_first: bool) -> Option<Advice> {
        controller.record(&Event::RunStarted {
            instruction: "fix it".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });

        respond(controller, 1, &[("read_file", json!({}))]);
        tool(controller, "read_file", json!({}), "...");
        controller.advise(1, 20);

        respond(controller, 2, &[("edit_file", json!({}))]);
        tool(controller, "edit_file", json!({}), "edited");

        let after_edit = controller.advise(2, 20);

        let mut steps = 3;

        if checks_first {
            respond(
                controller,
                3,
                &[("shell", json!({ "command": "npm test" }))],
            );
            tool(
                controller,
                "shell",
                json!({ "command": "npm test" }),
                "exit code: 0",
            );
            controller.advise(3, 20);

            steps = 4;
        }

        respond(controller, steps, &[]);

        controller.record(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps,
            usage: Usage::default(),
            duration_ms: 1,
            verified: Some(checks_first),
        });

        after_edit
    }

    /// Runs that check their work pass and runs that finish straight
    /// after editing fail. The controller should learn to say "run
    /// something" after an edit.
    fn learns_to_check_after_editing(controller: Controller) {
        assert!(
            play(&controller, true).is_none(),
            "an untrained controller is silent"
        );

        for round in 0..12 {
            play(&controller, round % 2 == 0);
        }

        let advice = play(&controller, true).expect("a trained controller speaks");

        assert_eq!(advice.detail["advised"], "run");
        assert!(advice.text.contains("run a command to check the work"));

        let values = &advice.detail["values"];

        assert!(values[Action::Run.index()].as_f64() > values[Action::Finish.index()].as_f64());
    }

    #[test]
    fn the_baseline_learns_to_check_after_editing() {
        learns_to_check_after_editing(Controller::new(Box::new(DirectEncoder), None));
    }

    #[test]
    fn the_spiking_brain_learns_to_check_after_editing() {
        let brain = Brain::generate(BrainConfig::tiny());

        learns_to_check_after_editing(Controller::new(Box::new(FlyBrainEncoder::new(brain)), None));
    }

    /// The same lesson through the measured fly connectome. Needs the
    /// converted file, which is not in the repository:
    ///
    ///     CORTEX_CONNECTOME=path/to/fly.csv cargo test --release -p cortex-brain -- --ignored
    #[test]
    #[ignore = "needs the measured connectome file"]
    fn the_measured_fly_brain_learns_to_check_after_editing() {
        let path = std::env::var("CORTEX_CONNECTOME").expect("set CORTEX_CONNECTOME");

        let brain = Brain::from_file(BrainConfig::measured(), path.as_ref()).unwrap();

        assert!(brain.config().neurons > 100_000);

        learns_to_check_after_editing(Controller::new(Box::new(FlyBrainEncoder::new(brain)), None));
    }

    /// Every run here passes, so finishing always earns the most. The
    /// controller must still not tell a run that has only just edited to
    /// stop.
    #[test]
    fn finishing_is_never_advised() {
        let controller = Controller::new(Box::new(DirectEncoder), None);

        for _ in 0..20 {
            if let Some(advice) = play(&controller, true) {
                assert_ne!(advice.detail["advised"], "finish");
            }
        }

        assert!(controller.runs() >= MIN_RUNS);
    }

    #[test]
    fn a_run_without_a_checked_result_teaches_nothing() {
        let controller = Controller::new(Box::new(DirectEncoder), None);

        controller.record(&Event::RunStarted {
            instruction: "fix it".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });

        respond(&controller, 1, &[]);

        controller.record(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 1,
            usage: Usage::default(),
            duration_ms: 1,
            verified: None,
        });

        assert_eq!(controller.runs(), 0);
    }

    #[test]
    fn what_was_learned_is_kept_between_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("controller.json");

        let first = Controller::new(Box::new(DirectEncoder), Some(file.clone()));

        play(&first, true);

        let second = Controller::new(Box::new(DirectEncoder), Some(file));

        assert_eq!(second.runs(), 1);
    }
}

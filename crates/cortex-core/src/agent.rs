//! The agent loop: ask the model, run the tools it asks for, repeat.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::advice::Adviser;
use crate::briefing::{self, Briefing, ContextSource};
use crate::changes::{self, Originals};
use crate::compact::CompactConfig;
use crate::context::{self, MaskConfig, PruneConfig};
use crate::effect::{self, Experiment, ExperimentConfig};
use crate::error::{AgentError, ProviderError};
use crate::inbox::{self, Inbox};
use crate::message::{Block, Image, Message, Role, ToolCall, ToolResult};
use crate::provider::{ModelProvider, ModelRequest, ModelResponse, StopReason, StreamObserver};
use crate::reflect;
use crate::repeat;
use crate::review;
use crate::rollout::RolloutConfig;
use crate::tool::{ToolContext, ToolRegistry};
use crate::trajectory::{Event, TrajectorySink};
use crate::usage::Usage;
use crate::verify::Verifier;
use crate::window::{self, ContextUse};

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub system_prompt: String,

    /// Upper bound on model calls for one instruction.
    pub max_steps: u32,

    /// Trim old tool output when the conversation grows large. `None`
    /// leaves the conversation untouched.
    pub prune: Option<PruneConfig>,

    /// How many times a run may be sent back to the model because the
    /// project's checks failed. After that the run ends unverified, so a
    /// check that was already failing before the task cannot trap the agent.
    pub max_verification_rounds: u32,

    /// Answer a repeated read-only call whose output has not changed with
    /// one line instead of the same text again. See [`crate::repeat`].
    pub fold_repeats: bool,

    /// The parts that go beyond the plain loop. All off by default.
    pub extras: Extras,
}

/// Optional behaviour of the loop, each part measured against the loop
/// without it.
#[derive(Debug, Clone, Default)]
pub struct Extras {
    /// Replace tool output older than the last few steps with one line.
    pub mask: Option<MaskConfig>,

    /// When the model says it is done, hold its changes against the
    /// request and send back what looks unmet, once.
    pub review: bool,

    /// After a run, ask the model what it taught and report the lessons
    /// as an event for memory to keep.
    pub reflect: bool,

    /// Make several attempts in copies of the project and keep the best.
    pub rollouts: Option<RolloutConfig>,

    /// Replace the older part of a long conversation with a summary the
    /// model writes. See [`crate::compact`].
    pub compact: Option<CompactConfig>,

    /// The model's context window in tokens, for a model whose window
    /// [`crate::window::context_window`] does not know, or to compact
    /// sooner than the real window would ask for.
    pub context_window: Option<u64>,

    /// Keep each adviser silent in a share of runs, chosen at random, so
    /// that runs with and without its advice can be compared. See
    /// [`crate::effect`].
    pub experiment: Option<ExperimentConfig>,
}

/// The diff shown to the reviewer is shortened to this.
const MAX_REVIEWED_DIFF_CHARS: usize = 24_000;

/// How far a run has got. Returned with a provider failure so that
/// [`Agent::resume`] can carry on with the counts intact.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    /// Model calls that were answered.
    pub steps: u32,

    pub usage: Usage,

    /// Time spent working, not counting any wait before a resume.
    pub elapsed: Duration,

    /// Boxed so that an error carrying a `Progress` stays small.
    pub(crate) gate: Box<Gate>,
}

impl Progress {
    pub(crate) fn starting(instruction: &str) -> Self {
        let mut progress = Self::default();

        progress.gate.instruction = instruction.to_string();

        progress
    }
}

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The model ended its turn normally.
    Completed,

    /// The run used all of its steps.
    StepLimit,

    /// A response hit the provider's output limit.
    Truncated,

    /// The provider declined the request.
    Refused,
}

#[derive(Debug, Clone)]
pub struct RunReport {
    pub outcome: Outcome,
    pub final_text: String,
    pub steps: u32,
    pub usage: Usage,
    pub duration: Duration,

    /// Whether the project's checks passed on the finished work. `None`
    /// when no verifier was set or it was never reached.
    pub verified: Option<bool>,
}

pub struct Agent {
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) tools: ToolRegistry,
    pub(crate) context: ToolContext,
    pub(crate) config: AgentConfig,
    pub(crate) sinks: Vec<Arc<dyn TrajectorySink>>,
    pub(crate) context_sources: Vec<Arc<dyn ContextSource>>,
    pub(crate) verifier: Option<Arc<dyn Verifier>>,
    pub(crate) advisers: Vec<Arc<dyn Adviser>>,
    pub(crate) stream_observer: Option<Arc<dyn StreamObserver>>,
    pub(crate) inbox: Option<Arc<Inbox>>,

    /// The advisers that have spoken in this agent's runs, by the name
    /// they sign their advice with. An adviser has no name until it
    /// speaks, so this is how a run's hold-outs can be listed up front.
    pub(crate) heard: Arc<Mutex<BTreeSet<String>>>,
}

impl Agent {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        tools: ToolRegistry,
        context: ToolContext,
        config: AgentConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            context,
            config,
            sinks: Vec::new(),
            context_sources: Vec::new(),
            verifier: None,
            advisers: Vec::new(),
            stream_observer: None,
            inbox: None,
            heard: Arc::default(),
        }
    }

    /// Lets the person speak to a run in progress: what is put in `inbox`
    /// is shown to the model after the step it is working on.
    pub fn with_inbox(mut self, inbox: Arc<Inbox>) -> Self {
        self.inbox = Some(inbox);

        self
    }

    pub fn with_sink(mut self, sink: Arc<dyn TrajectorySink>) -> Self {
        self.sinks.push(sink);

        self
    }

    /// Adds a source of context that is consulted before each instruction.
    pub fn with_context_source(mut self, source: Arc<dyn ContextSource>) -> Self {
        self.context_sources.push(source);

        self
    }

    /// Lets `observer` watch each response while the model is writing it.
    pub fn with_stream_observer(mut self, observer: Arc<dyn StreamObserver>) -> Self {
        self.stream_observer = Some(observer);

        self
    }

    /// Adds an adviser that may comment after each step's tools have run.
    pub fn with_adviser(mut self, adviser: Arc<dyn Adviser>) -> Self {
        self.advisers.push(adviser);

        self
    }

    /// Makes the run answer to the project's checks before it may finish.
    pub fn with_verifier(mut self, verifier: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(verifier);

        self
    }

    /// Carries out one instruction.
    ///
    /// `conversation` is owned by the caller so that an interactive session
    /// can pass the same history to successive instructions.
    pub async fn run(
        &self,
        conversation: &mut Vec<Message>,
        instruction: &str,
    ) -> Result<RunReport, AgentError> {
        self.run_with_images(conversation, instruction, &[]).await
    }

    /// As [`Self::run`], with pictures for the model to look at.
    pub async fn run_with_images(
        &self,
        conversation: &mut Vec<Message>,
        instruction: &str,
        images: &[Image],
    ) -> Result<RunReport, AgentError> {
        self.emit(Event::RunStarted {
            instruction: instruction.to_string(),
            provider: self.provider.name().to_string(),
            model: self.provider.model().to_string(),
        });

        if !images.is_empty() {
            self.emit(Event::ImagesAttached {
                images: images.to_vec(),
            });
        }

        let experiment = self.draw_experiment();

        let briefings = self.gather_context(instruction, experiment.as_ref()).await;

        if let Some(experiment) = &experiment {
            self.emit(self.assignment(experiment, &briefings));
        }

        for adviser in &self.advisers {
            adviser.begin_run(self.config.max_steps);
        }

        let rollouts = self
            .config
            .extras
            .rollouts
            .as_ref()
            .filter(|rollouts| rollouts.count > 1);

        if let Some(rollouts) = rollouts {
            return self
                .run_rollouts(rollouts, conversation, instruction, images, briefings)
                .await;
        }

        conversation.push(Message::user_with_images(
            briefing::compose(&briefings, instruction),
            images,
        ));

        let mut progress = Progress::starting(instruction);

        progress.gate.experiment = experiment;

        self.work(conversation, progress).await
    }

    /// Carries on a run that stopped on a provider failure.
    ///
    /// `conversation` must be the one that run was using, untouched, and
    /// `progress` the one that came back with the error. The step that
    /// failed is asked again; every step before it is kept.
    pub async fn resume(
        &self,
        conversation: &mut Vec<Message>,
        progress: Progress,
    ) -> Result<RunReport, AgentError> {
        self.emit(Event::RunResumed {
            steps: progress.steps,
        });

        self.work(conversation, progress).await
    }

    /// The loop, then what follows a finished run.
    async fn work(
        &self,
        conversation: &mut Vec<Message>,
        progress: Progress,
    ) -> Result<RunReport, AgentError> {
        let (outcome, progress) = self.steps(conversation, progress).await?;

        Ok(self.finish(conversation, outcome, progress).await)
    }

    /// The loop itself, from wherever `progress` says the run is, until
    /// the run stops.
    pub(crate) async fn steps(
        &self,
        conversation: &mut Vec<Message>,
        mut progress: Progress,
    ) -> Result<(Outcome, Progress), AgentError> {
        let started = Instant::now();
        let specs = self.tools.specs();

        let outcome = loop {
            if progress.steps >= self.config.max_steps {
                break Outcome::StepLimit;
            }

            let step = progress.steps + 1;

            self.compact_when_full(step, conversation, &mut progress)
                .await;

            self.prune(step, conversation);
            self.mask(step, conversation);
            for adviser in &self.advisers {
                let adviser = adviser.clone();
                let max_steps = self.config.max_steps;
                adviser_job(move || adviser.prepare(step, max_steps)).await;
            }

            let request = ModelRequest {
                system: &self.config.system_prompt,
                messages: conversation,
                tools: &specs,
            };

            self.emit(Event::ModelStarted { step });

            let sent = window::measure(&self.config.system_prompt, &specs, conversation);

            let call_started = Instant::now();

            let response = match self.ask(request).await {
                Ok(response) => response,

                // The step did not happen, so it is not counted, and the
                // conversation is exactly as it was before the request.
                Err(error) => {
                    progress.elapsed += started.elapsed();

                    return Err(AgentError::Provider { error, progress });
                }
            };

            progress.steps = step;
            progress.usage += response.usage;

            self.emit(Event::ModelResponded {
                step,
                message: response.message.clone(),
                stop_reason: response.stop_reason.clone(),
                usage: response.usage,
                duration_ms: call_started.elapsed().as_millis() as u64,
            });

            let input = response.usage.total_input();

            self.emit(Event::ContextMeasured {
                step,
                context: Box::new(ContextUse::new(
                    input,
                    response.usage.output_tokens,
                    self.context_window(),
                    sent,
                )),
                quota: self.provider.quota(),
            });

            progress.gate.heard_back(input, self.compact_threshold());

            let calls = response.message.tool_calls();

            conversation.push(response.message);

            match response.stop_reason {
                StopReason::ToolUse => {
                    for call in &calls {
                        if self.tools.is_mutating(&call.name) {
                            progress.gate.changed = true;
                            progress.gate.originals.keep(&self.context.workspace, call);
                        }
                    }

                    let results = self.run_tools(step, &calls, conversation).await;

                    let mut message = Message::tool_results(results);

                    message
                        .blocks
                        .extend(self.advice(step, progress.gate.experiment.as_ref()).await);
                    message.blocks.extend(self.interjections(step));

                    conversation.push(message);
                }

                StopReason::Paused => {}

                StopReason::EndTurn | StopReason::Other(_) => {
                    // Something said as the model was finishing is
                    // answered before the run may end.
                    let said = self.interjections(step);

                    if !said.is_empty() {
                        conversation.push(Message {
                            role: Role::User,
                            blocks: said,
                        });

                        continue;
                    }

                    let feedback = match self.check_work(step, &mut progress.gate).await {
                        Some(feedback) => Some(feedback),
                        None => self.review_work(step, conversation, &mut progress).await,
                    };

                    match feedback {
                        Some(feedback) => conversation.push(Message::user_text(feedback)),
                        None => break Outcome::Completed,
                    }
                }

                StopReason::MaxTokens => break Outcome::Truncated,

                StopReason::Refusal => break Outcome::Refused,
            }
        };

        close_unanswered_calls(conversation);

        progress.elapsed += started.elapsed();

        Ok((outcome, progress))
    }

    /// Ends a run: takes its lessons, when asked to, and reports it.
    pub(crate) async fn finish(
        &self,
        conversation: &[Message],
        outcome: Outcome,
        mut progress: Progress,
    ) -> RunReport {
        let started = Instant::now();

        self.reflect(conversation, outcome, &mut progress).await;

        let duration = progress.elapsed + started.elapsed();

        self.emit(Event::RunFinished {
            outcome,
            steps: progress.steps,
            usage: progress.usage,
            duration_ms: duration.as_millis() as u64,
            verified: progress.gate.verified,
        });

        RunReport {
            outcome,
            final_text: last_assistant_text(conversation),
            steps: progress.steps,
            usage: progress.usage,
            duration,
            verified: progress.gate.verified,
        }
    }

    /// Asks the model what the run taught. Only a run that changed
    /// something is worth the call, and a failed call costs the run
    /// nothing but the lessons.
    async fn reflect(&self, conversation: &[Message], outcome: Outcome, progress: &mut Progress) {
        if !self.config.extras.reflect || !progress.gate.changed {
            return;
        }

        let started = Instant::now();

        let learned = reflect::lessons(
            self.provider.as_ref(),
            &progress.gate.instruction,
            conversation,
            outcome,
            progress.gate.verified,
        )
        .await;

        let Ok((lessons, usage)) = learned else {
            return;
        };

        progress.usage += usage;

        self.emit(Event::LessonsLearned {
            lessons,
            usage,
            duration_ms: started.elapsed().as_millis() as u64,
        });
    }

    /// The review of finished work against the request. Called when the
    /// model says it is done and the checks, if any, have passed.
    ///
    /// Returns what to send back to the model, or `None` when the run may
    /// end. It speaks once per run, so a reviewer that is wrong costs one
    /// extra round and cannot trap the run.
    async fn review_work(
        &self,
        step: u32,
        conversation: &[Message],
        progress: &mut Progress,
    ) -> Option<String> {
        let gate = &mut progress.gate;

        if !self.config.extras.review || gate.reviewed || gate.verified == Some(false) {
            return None;
        }

        let changed = gate.originals.changes(&self.context.workspace);

        if changed.is_empty() {
            return None;
        }

        gate.reviewed = true;

        let started = Instant::now();

        let diff = changes::render(&changed, MAX_REVIEWED_DIFF_CHARS);
        let report = last_assistant_text(conversation);

        let reviewed =
            review::review(self.provider.as_ref(), &gate.instruction, &diff, &report).await;

        let review = reviewed.ok().flatten()?;

        progress.usage += review.usage;

        let feedback = review.feedback();

        self.emit(Event::ReviewRan {
            step,
            requirements: review.requirements,
            feedback: feedback.clone(),
            usage: review.usage,
            duration_ms: started.elapsed().as_millis() as u64,
        });

        feedback
    }

    async fn ask(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        match &self.stream_observer {
            Some(observer) => {
                self.provider
                    .complete_streaming(request, observer.as_ref())
                    .await
            }

            None => self.provider.complete(request).await,
        }
    }

    /// Asks every context source for what it knows about the task.
    ///
    /// A briefing that `experiment` holds out is recorded and left out.
    async fn gather_context(
        &self,
        instruction: &str,
        experiment: Option<&Experiment>,
    ) -> Vec<Briefing> {
        let mut briefings = Vec::new();

        for source in &self.context_sources {
            let started = Instant::now();

            let Some(briefing) = source.brief(instruction, &self.context.workspace).await else {
                continue;
            };

            if experiment.is_some_and(|experiment| experiment.withholds_briefing(&briefing.source))
            {
                self.emit(Event::ContextWithheld {
                    step: 0,
                    source: briefing.source,
                    text: briefing.text,
                    detail: briefing.detail,
                    duration_ms: started.elapsed().as_millis() as u64,
                });

                continue;
            }

            self.emit(Event::ContextPrepared {
                source: briefing.source.clone(),
                text: briefing.text.clone(),
                detail: briefing.detail.clone(),
                duration_ms: started.elapsed().as_millis() as u64,
            });

            briefings.push(briefing);
        }

        briefings
    }

    /// Decides, for a run that is starting, which advisers stay silent.
    /// `None` when no experiment is set up, or the model is a stand-in
    /// whose runs would measure nothing.
    fn draw_experiment(&self) -> Option<Experiment> {
        let config = self.config.extras.experiment.as_ref()?;

        if !config.stand_ins && effect::is_stand_in(self.provider.name()) {
            return None;
        }

        Experiment::draw(config)
    }

    /// The hold-outs of a run as an event: every adviser heard from so
    /// far, and the briefings gathered for this run.
    fn assignment(&self, experiment: &Experiment, briefings: &[Briefing]) -> Event {
        let mut held_out = Vec::new();
        let mut speaking = Vec::new();

        let heard = self
            .heard
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        for source in heard.iter() {
            if experiment.withholds_advice(source) {
                held_out.push(source.clone());
            } else {
                speaking.push(source.clone());
            }
        }

        // Withheld briefings have already been recorded one by one.
        let briefed = briefings
            .iter()
            .map(|briefing| briefing.source.clone())
            .collect();

        Event::ExperimentAssigned {
            seed: experiment.seed,
            share: experiment.share,
            briefing_share: experiment.briefing_share,
            held_out,
            speaking,
            briefed,
        }
    }

    /// Notes from the advisers, placed after the step's tool results.
    /// What an adviser that `experiment` holds out would have said is
    /// recorded and not shown.
    async fn advice(&self, step: u32, experiment: Option<&Experiment>) -> Vec<Block> {
        let mut blocks = Vec::new();

        for adviser in &self.advisers {
            let started = Instant::now();

            let worker = adviser.clone();
            let max_steps = self.config.max_steps;
            let advice = adviser_job(move || worker.advise(step, max_steps)).await;
            self.emit_adviser(adviser.as_ref(), step);
            let Some(advice) = advice else {
                continue;
            };

            self.heard
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(advice.source.clone());

            if experiment.is_some_and(|experiment| experiment.withholds_advice(&advice.source)) {
                adviser.held_out(step);

                self.emit(Event::ContextWithheld {
                    step,
                    source: advice.source,
                    text: advice.text,
                    detail: advice.detail,
                    duration_ms: started.elapsed().as_millis() as u64,
                });

                continue;
            }

            self.emit(Event::ContextPrepared {
                source: advice.source,
                text: advice.text.clone(),
                detail: advice.detail,
                duration_ms: started.elapsed().as_millis() as u64,
            });

            blocks.push(Block::Text { text: advice.text });
        }

        blocks
    }

    /// What the person has said since the last step, as the model is
    /// shown it.
    fn interjections(&self, step: u32) -> Vec<Block> {
        let Some(inbox) = &self.inbox else {
            return Vec::new();
        };

        inbox
            .take()
            .into_iter()
            .map(|text| {
                let shown = inbox::as_shown(&text);

                self.emit(Event::UserInterjected { step, text });

                Block::Text { text: shown }
            })
            .collect()
    }

    /// The verification gate. Called when the model says it is finished.
    ///
    /// Returns the failure report to send back to the model, or `None` when
    /// the run may end.
    async fn check_work(&self, step: u32, gate: &mut Gate) -> Option<String> {
        let verifier = self.verifier.as_ref()?;

        // A run that only read and answered has no work to check.
        if !gate.changed {
            return None;
        }

        let started = Instant::now();

        let verification = verifier.verify(&self.context.workspace).await;

        if verification.checks.is_empty() {
            return None;
        }

        let passed = verification.passed();

        gate.verified = Some(passed);

        self.emit(Event::VerificationRan {
            step,
            passed,
            checks: verification.checks.clone(),
            duration_ms: started.elapsed().as_millis() as u64,
        });

        if passed || gate.rejections >= self.config.max_verification_rounds {
            return None;
        }

        gate.rejections += 1;

        Some(verification.feedback())
    }

    /// Runs the calls from one model response.
    ///
    /// Calls that only look are run together when several stand next to
    /// each other. Anything that can change the project runs alone, in the
    /// order the model asked, so its effects are seen by what follows.
    /// The results come back in the order of the calls either way.
    async fn run_tools(
        &self,
        step: u32,
        calls: &[ToolCall],
        conversation: &[Message],
    ) -> Vec<ToolResult> {
        let mut results = Vec::with_capacity(calls.len());

        for group in self.groups(calls) {
            for call in group {
                self.emit(Event::ToolStarted {
                    step,
                    call: call.clone(),
                });
            }

            let finished = match group {
                [call] => vec![timed_dispatch(&self.tools, call, &self.context).await],
                _ => self.run_together(group).await,
            };

            for (call, (mut result, duration_ms)) in group.iter().zip(finished) {
                if self.config.fold_repeats && !self.tools.is_mutating(&call.name) {
                    repeat::fold(conversation, call, &mut result);
                }

                self.emit(Event::ToolFinished {
                    step,
                    tool: call.name.clone(),
                    result: result.clone(),
                    duration_ms,
                });

                results.push(result);
            }
        }

        results
    }

    /// Splits `calls` into runs of read-only calls and single mutating ones.
    fn groups<'a>(&self, calls: &'a [ToolCall]) -> Vec<&'a [ToolCall]> {
        let mut groups = Vec::new();
        let mut start = 0;

        for (position, call) in calls.iter().enumerate() {
            if !self.tools.is_mutating(&call.name) {
                continue;
            }

            if start < position {
                groups.push(&calls[start..position]);
            }

            groups.push(&calls[position..=position]);

            start = position + 1;
        }

        if start < calls.len() {
            groups.push(&calls[start..]);
        }

        groups
    }

    /// Runs read-only calls at the same time, each on its own task.
    async fn run_together(&self, calls: &[ToolCall]) -> Vec<(ToolResult, u64)> {
        let mut tasks = JoinSet::new();

        for (position, call) in calls.iter().enumerate() {
            let tools = self.tools.clone();
            let context = self.context.clone();
            let call = call.clone();

            tasks.spawn(async move { (position, timed_dispatch(&tools, &call, &context).await) });
        }

        let mut finished: Vec<Option<(ToolResult, u64)>> = vec![None; calls.len()];

        while let Some(joined) = tasks.join_next().await {
            if let Ok((position, outcome)) = joined {
                finished[position] = Some(outcome);
            }
        }

        // A slot is only empty when the tool panicked. The model still
        // needs an answer for that call.
        calls
            .iter()
            .zip(finished)
            .map(|(call, outcome)| outcome.unwrap_or_else(|| (crashed(call), 0)))
            .collect()
    }

    /// The input size, in tokens, from which the conversation is
    /// compacted without being asked. `None` when that is switched off
    /// or the model's window is not known.
    fn compact_threshold(&self) -> Option<u64> {
        let share = self.config.extras.compact.as_ref()?.auto_at?;
        let window = self.context_window()?;

        Some((window as f64 * f64::from(share.clamp(0.0, 1.0))) as u64)
    }

    /// Compacts the conversation before `step` when the last call showed
    /// it has filled most of the window.
    ///
    /// It cannot go round in circles. It needs a measurement from a model
    /// call made after the last compaction. If that first call is still
    /// over the line, summarising again would not help, and it is left
    /// alone for the rest of the run; the same when the summary call
    /// fails. The run carries on either way.
    async fn compact_when_full(
        &self,
        step: u32,
        conversation: &mut Vec<Message>,
        progress: &mut Progress,
    ) {
        let Some(threshold) = self.compact_threshold() else {
            return;
        };

        if progress.gate.compacting_is_futile || progress.gate.last_input < threshold {
            return;
        }

        // Whatever happens, not again before the model has been asked.
        progress.gate.last_input = 0;

        match self.compact_at(step, conversation, true).await {
            Ok(Some(compaction)) => {
                progress.usage += compaction.usage;
                progress.gate.just_compacted = true;
            }

            Ok(None) | Err(_) => progress.gate.compacting_is_futile = true,
        }
    }

    fn prune(&self, step: u32, conversation: &mut [Message]) {
        let Some(config) = &self.config.prune else {
            return;
        };

        let stats = context::prune(conversation, config);

        if stats.results_trimmed > 0 {
            self.emit(Event::ContextPruned {
                step,
                results_trimmed: stats.results_trimmed,
                chars_removed: stats.chars_removed,
            });
        }
    }

    fn mask(&self, step: u32, conversation: &mut [Message]) {
        let Some(config) = &self.config.extras.mask else {
            return;
        };

        let stats = context::mask(conversation, config);

        if stats.results_trimmed > 0 {
            self.emit(Event::ContextMasked {
                step,
                results_masked: stats.results_trimmed,
                chars_removed: stats.chars_removed,
            });
        }
    }

    pub(crate) fn emit(&self, event: Event) {
        for sink in &self.sinks {
            sink.record(&event);
        }
        let step = match event {
            Event::RunFinished { steps, .. } => Some(steps),
            Event::ModelStarted { step: 1 } => Some(1),
            _ => None,
        };
        if let Some(step) = step {
            for adviser in &self.advisers {
                self.emit_adviser(adviser.as_ref(), step);
            }
        }
    }

    fn emit_adviser(&self, adviser: &dyn Adviser, step: u32) {
        if let Some(detail) = adviser.telemetry() {
            let source = detail["source"].as_str().unwrap_or("adviser").to_string();
            let event = Event::AdviserObserved {
                source,
                step,
                detail,
            };
            for sink in &self.sinks {
                sink.record(&event);
            }
        }
    }
}

async fn adviser_job<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> T {
    static JOBS: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    let permit = JOBS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1)))
        .clone()
        .acquire_owned()
        .await
        .expect("adviser scheduler open");
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        job()
    })
    .await
    .expect("adviser worker finished")
}

/// Runs one call and reports how long it took, in milliseconds.
async fn timed_dispatch(
    tools: &ToolRegistry,
    call: &ToolCall,
    context: &ToolContext,
) -> (ToolResult, u64) {
    let started = Instant::now();

    let result = tools.dispatch(call, context).await;

    (result, started.elapsed().as_millis() as u64)
}

fn crashed(call: &ToolCall) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        output: format!("the `{}` tool stopped unexpectedly", call.name),
        is_error: true,
        images: Vec::new(),
    }
}

/// What the verification gate knows about the current run.
#[derive(Debug, Clone, Default)]
pub(crate) struct Gate {
    /// A tool that can change files or run commands was called.
    pub(crate) changed: bool,

    /// Times the model was sent back to fix failing checks.
    rejections: u32,

    /// Result of the most recent verification.
    pub(crate) verified: Option<bool>,

    /// The review has had its one say.
    reviewed: bool,

    /// What the files the run edited held before it did.
    originals: Originals,

    /// What the run was asked to do, for the review and the lessons.
    instruction: String,

    /// Tokens the last model call was sent, or 0 when the conversation
    /// has been compacted since.
    last_input: u64,

    /// No model call has been made since the conversation was compacted.
    just_compacted: bool,

    /// Compacting did not bring the conversation under the line, or the
    /// summary could not be written. It is not tried again in this run.
    compacting_is_futile: bool,

    /// Which advisers stay silent in this run.
    pub(crate) experiment: Option<Experiment>,
}

impl Gate {
    /// Takes note of how large the input of the call just answered was.
    /// `threshold` is the size from which the conversation is compacted.
    fn heard_back(&mut self, input: u64, threshold: Option<u64>) {
        self.last_input = input;

        if std::mem::take(&mut self.just_compacted)
            && threshold.is_some_and(|threshold| input >= threshold)
        {
            self.compacting_is_futile = true;
        }
    }
}

/// Providers reject a history where a tool call has no result. If a run
/// stopped right after the model asked for tools, answer those calls so the
/// conversation can continue.
///
/// [`Agent::run`] does this itself when a run ends. A caller that abandons
/// a run part-way (by dropping its future) must call it before using the
/// conversation again.
pub fn close_unanswered_calls(conversation: &mut Vec<Message>) {
    let Some(last) = conversation.last() else {
        return;
    };

    if last.role != Role::Assistant {
        return;
    }

    let calls = last.tool_calls();

    if calls.is_empty() {
        return;
    }

    let results = calls
        .into_iter()
        .map(|call| ToolResult {
            call_id: call.id,
            output: "This call was not run because the run stopped first.".to_string(),
            is_error: true,
            images: Vec::new(),
        })
        .collect();

    conversation.push(Message::tool_results(results));
}

pub(crate) fn last_assistant_text(conversation: &[Message]) -> String {
    conversation
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .map(Message::text)
        .unwrap_or_default()
}

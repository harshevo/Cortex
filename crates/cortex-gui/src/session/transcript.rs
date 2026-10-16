//! The conversation as a list of things to draw.

use std::time::{Duration, Instant};

use cortex_core::{
    CheckOutcome, Event, Image, Outcome, Requirement, ToolCall, ToolResult, Usage, repeat,
};

use super::stats::{Stats, tokens_per_second};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Bad,

    /// The engine tidying the context. Counted in the statistics and
    /// not drawn in the conversation.
    Housekeeping,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolItem {
    pub call: ToolCall,

    /// `None` while the tool is still running.
    pub result: Option<ToolResult>,

    pub duration_ms: u64,

    /// Whether the card is unfolded to show input and output.
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User {
        text: String,

        /// Pictures sent with it.
        images: Vec<Image>,
    },

    /// Context the engine put in front of the task (routed code).
    Briefing {
        source: String,
        text: String,
        open: bool,
    },

    Assistant {
        step: u32,
        text: String,

        /// The model's reasoning for this step, when the provider showed
        /// it while it was being written. Not kept in the session's log.
        thinking: String,

        duration_ms: u64,
        speed: Option<f32>,
    },

    Tool(ToolItem),

    Checks {
        passed: bool,
        checks: Vec<CheckOutcome>,
        open: bool,
    },

    /// The finished work held against the request.
    Review {
        requirements: Vec<Requirement>,

        /// Something looked unmet and the model was asked again.
        sent_back: bool,

        open: bool,
    },

    Notice {
        text: String,
        tone: Tone,
    },

    RunSummary {
        outcome: Outcome,
        verified: Option<bool>,
        steps: u32,
        usage: Usage,
        duration_ms: u64,
    },
}

/// A response the model is in the middle of writing.
#[derive(Debug, Clone, PartialEq)]
pub struct Writing {
    pub text: String,

    /// The reasoning that came before the text, from a provider that
    /// shows it.
    pub thinking: String,

    /// When the first piece arrived.
    since: Instant,
}

impl Writing {
    /// Roughly how fast the text is arriving, in tokens per second. The
    /// provider only counts tokens once the response is complete, so this
    /// goes by the usual four characters to a token.
    pub fn estimated_speed(&self) -> Option<f32> {
        const CHARS_PER_TOKEN: f32 = 4.0;

        /// Too soon to say anything meaningful.
        const MIN_SECONDS: f32 = 0.3;

        let seconds = self.since.elapsed().as_secs_f32();

        (seconds >= MIN_SECONDS)
            .then(|| self.text.chars().count() as f32 / CHARS_PER_TOKEN / seconds)
    }
}

/// What a run is busy with while there is nothing of it to read yet.
#[derive(Debug, Clone, PartialEq)]
pub enum Activity {
    /// The engine is collecting what it knows about the task.
    Preparing,

    /// The request is with the model and nothing has come back.
    Waiting,

    /// The request failed and goes out again at `until`.
    Retrying { reason: String, until: Instant },

    /// The model is writing the arguments of a call to this tool.
    Calling(String),

    /// Attempts are being made in copies of the project. Their steps are
    /// only known once one is kept.
    Attempting,

    /// The model has said it is done; its work is being checked.
    Finishing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Doing {
    pub activity: Activity,

    /// When the wait began. A request that is retried keeps counting.
    pub since: Instant,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    pub items: Vec<Item>,
    pub stats: Stats,

    /// The response being written right now, if the provider streams.
    pub writing: Option<Writing>,

    /// What the run is busy with, while it has nothing to show for it.
    pub doing: Option<Doing>,

    /// A run is in progress.
    pub running: bool,

    /// The model named by the most recent run.
    pub model: Option<String>,

    /// The last run stopped on a provider failure that may not last, and
    /// can be carried on from the step that failed.
    pub can_continue: bool,
}

impl Session {
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::AdviserObserved { .. } => {}
            Event::ImagesAttached { images } => {
                if let Some(Item::User { images: shown, .. }) = self.items.last_mut() {
                    shown.clone_from(images);
                }
            }

            Event::RunStarted {
                instruction, model, ..
            } => {
                self.running = true;
                self.can_continue = false;
                self.model = Some(model.clone());
                self.stats.run_started();
                self.busy_with(Activity::Preparing);

                self.items.push(Item::User {
                    text: instruction.clone(),
                    images: Vec::new(),
                });
            }

            Event::RunResumed { steps } => {
                self.running = true;
                self.can_continue = false;
                self.stats.resumed();
                self.doing = None;

                self.items.push(Item::Notice {
                    text: format!("Carrying on after step {steps}"),
                    tone: Tone::Muted,
                });
            }

            Event::ContextPrepared { source, text, .. } => {
                self.items.push(Item::Briefing {
                    source: source.clone(),
                    text: text.clone(),
                    open: false,
                });
            }

            Event::ModelStarted { step } => {
                self.stats.model_started(*step);
                self.busy_with(Activity::Waiting);
            }

            Event::ModelResponded {
                step,
                message,
                usage,
                duration_ms,
                ..
            } => {
                let thinking = self
                    .writing
                    .take()
                    .map(|writing| writing.thinking.trim().to_string())
                    .unwrap_or_default();

                self.stats.model_responded(*step, *usage, *duration_ms);

                // Its tools show themselves running. With none, the run
                // ends unless the checks or a review send it back.
                if message.tool_calls().is_empty() {
                    self.busy_with(Activity::Finishing);
                } else {
                    self.doing = None;
                }

                self.items.push(Item::Assistant {
                    step: *step,
                    text: message.text().trim().to_string(),
                    thinking,
                    duration_ms: *duration_ms,
                    speed: tokens_per_second(usage.output_tokens, *duration_ms),
                });
            }

            Event::ToolStarted { call, .. } => {
                self.items.push(Item::Tool(ToolItem {
                    call: call.clone(),
                    result: None,
                    duration_ms: 0,
                    open: false,
                }));
            }

            Event::ToolFinished {
                result,
                duration_ms,
                ..
            } => {
                self.stats.tool_finished(result.is_error);

                if repeat::is_repeat_note(&result.output) {
                    self.stats.repeat_folded();
                }

                self.finish_tool(result, *duration_ms);
            }

            Event::UserInterjected { text, .. } => {
                self.items.push(Item::User {
                    text: text.clone(),
                    images: Vec::new(),
                });
            }

            Event::ContextPruned {
                results_trimmed,
                chars_removed,
                ..
            } => {
                self.stats.pruned(*chars_removed);

                self.items.push(Item::Notice {
                    text: format!(
                        "Removed {results_trimmed} superseded tool results ({chars_removed} characters)"
                    ),
                    tone: Tone::Housekeeping,
                });
            }

            Event::ContextMasked {
                results_masked,
                chars_removed,
                ..
            } => {
                self.stats.masked(*chars_removed);

                self.items.push(Item::Notice {
                    text: format!(
                        "Masked {results_masked} old tool results ({chars_removed} characters)"
                    ),
                    tone: Tone::Housekeeping,
                });
            }

            Event::ContextCompacted {
                step,
                messages_replaced,
                chars_before,
                chars_after,
                usage,
                automatic,
                ..
            } => {
                let removed = chars_before.saturating_sub(*chars_after);

                self.stats.compacted(removed, *usage, *step > 0);

                let why = if *automatic {
                    "the context window was filling up"
                } else {
                    "you asked"
                };

                self.items.push(Item::Notice {
                    text: format!(
                        "Replaced the {messages_replaced} oldest messages with a summary because \
                         {why} ({removed} characters shorter)"
                    ),
                    tone: Tone::Muted,
                });
            }

            Event::ContextMeasured { context, quota, .. } => {
                self.stats.context_measured(**context, *quota);
            }

            Event::ExperimentAssigned {
                seed,
                held_out,
                speaking,
                ..
            } => self.stats.experiment_assigned(*seed, held_out, speaking),

            // Not drawn: the model was not shown it.
            Event::ContextWithheld { source, .. } => self.stats.hold_out(source),

            Event::ReviewRan {
                requirements,
                feedback,
                ..
            } => {
                self.items.push(Item::Review {
                    requirements: requirements.clone(),
                    sent_back: feedback.is_some(),
                    open: feedback.is_some(),
                });
            }

            Event::LessonsLearned { lessons, .. } => {
                for lesson in lessons {
                    self.items.push(Item::Notice {
                        text: format!(
                            "Lesson kept: {}. {}",
                            lesson.title.trim_end_matches('.'),
                            lesson.advice
                        ),
                        tone: Tone::Muted,
                    });
                }
            }

            Event::AttemptStarted {
                attempt,
                of,
                strategy,
            } => {
                self.busy_with(Activity::Attempting);

                self.items.push(Item::Notice {
                    text: format!(
                        "Attempt {attempt} of up to {of} is working in its own copy of the project ({})",
                        strategy.replace('_', " ")
                    ),
                    tone: Tone::Muted,
                });
            }

            Event::AttemptFinished {
                attempt,
                steps,
                verified,
                review_score,
                changed_files,
                error,
                ..
            } => {
                let (text, tone) = match error {
                    Some(error) => (format!("Attempt {attempt} failed: {error}"), Tone::Bad),

                    None => {
                        let checks = match verified {
                            Some(true) => "checks passed",
                            Some(false) => "checks failed",
                            None => "no checks run",
                        };

                        let review = match review_score {
                            Some(score) => format!("review {:.0}%", score * 100.0),
                            None => "not reviewed".to_string(),
                        };

                        (
                            format!(
                                "Attempt {attempt}: {steps} steps, {changed_files} files changed, {checks}, {review}"
                            ),
                            Tone::Muted,
                        )
                    }
                };

                self.items.push(Item::Notice { text, tone });
            }

            Event::AttemptChosen {
                attempt, reason, ..
            } => {
                self.items.push(Item::Notice {
                    text: format!("Kept attempt {attempt} ({reason}). Its steps follow."),
                    tone: Tone::Muted,
                });
            }

            Event::VerificationRan { passed, checks, .. } => {
                self.stats.verification_ran(*passed);

                self.items.push(Item::Checks {
                    passed: *passed,
                    checks: checks.clone(),
                    open: !*passed,
                });
            }

            Event::RunFinished {
                outcome,
                steps,
                usage,
                duration_ms,
                verified,
            } => {
                self.running = false;
                self.doing = None;
                self.stats.run_ended();
                self.stats.run_used(*usage);

                self.items.push(Item::RunSummary {
                    outcome: *outcome,
                    verified: *verified,
                    steps: *steps,
                    usage: *usage,
                    duration_ms: *duration_ms,
                });
            }
        }
    }

    /// A request has gone out, or gone out again: whatever was being
    /// written belongs to an attempt that is over.
    pub fn writing_started(&mut self) {
        self.writing = None;

        self.keep_waiting(Activity::Waiting);
    }

    /// The next piece of the response being written.
    pub fn write(&mut self, piece: &str) {
        self.doing = None;

        self.being_written().text.push_str(piece);
    }

    /// The next piece of the model's reasoning.
    pub fn think(&mut self, piece: &str) {
        self.doing = None;

        self.being_written().thinking.push_str(piece);
    }

    /// The model has begun asking for a tool.
    pub fn calling(&mut self, tool: &str) {
        self.keep_waiting(Activity::Calling(tool.to_string()));
    }

    /// The request failed and goes out again after `wait`.
    pub fn retrying(&mut self, reason: &str, wait: Duration) {
        self.keep_waiting(Activity::Retrying {
            reason: reason.to_string(),
            until: Instant::now() + wait,
        });
    }

    fn busy_with(&mut self, activity: Activity) {
        self.doing = Some(Doing {
            activity,
            since: Instant::now(),
        });
    }

    /// Another stage of the same wait: the clock is not started again.
    fn keep_waiting(&mut self, activity: Activity) {
        match &mut self.doing {
            Some(doing) => doing.activity = activity,
            None => self.busy_with(activity),
        }
    }

    fn being_written(&mut self) -> &mut Writing {
        self.writing.get_or_insert_with(|| Writing {
            text: String::new(),
            thinking: String::new(),
            since: Instant::now(),
        })
    }

    /// Something the app did outside any run, said in the conversation.
    pub fn notice(&mut self, text: &str, tone: Tone) {
        self.items.push(Item::Notice {
            text: text.to_string(),
            tone,
        });
    }

    /// The run ended with an error instead of a `RunFinished` event.
    /// `can_continue` says whether it can be carried on from there.
    pub fn fail(&mut self, error: &str, can_continue: bool) {
        self.end_early(error, Tone::Bad);

        self.can_continue = can_continue;
    }

    /// The run was abandoned because the person asked.
    pub fn stop(&mut self) {
        self.end_early("Stopped.", Tone::Muted);
    }

    fn end_early(&mut self, notice: &str, tone: Tone) {
        self.running = false;
        self.can_continue = false;
        self.writing = None;
        self.doing = None;
        self.stats.run_ended();

        // A tool that was still running will never report back.
        self.items
            .retain(|item| !matches!(item, Item::Tool(tool) if tool.result.is_none()));

        self.items.push(Item::Notice {
            text: notice.to_string(),
            tone,
        });
    }

    fn finish_tool(&mut self, result: &ToolResult, duration_ms: u64) {
        let waiting = self.items.iter_mut().rev().find_map(|item| match item {
            Item::Tool(tool) if tool.call.id == result.call_id && tool.result.is_none() => {
                Some(tool)
            }

            _ => None,
        });

        if let Some(tool) = waiting {
            tool.result = Some(result.clone());
            tool.duration_ms = duration_ms;
        }
    }
}

#[cfg(test)]
mod tests {
    use cortex_core::{Message, StopReason};
    use serde_json::json;

    use super::*;
    use crate::session::stats::Cell;

    fn run_started() -> Event {
        Event::RunStarted {
            instruction: "fix it".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        }
    }

    fn responded(step: u32, text: &str) -> Event {
        Event::ModelResponded {
            step,
            message: Message::assistant_text(text),
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 300,
                cache_write_tokens: 0,
            },
            duration_ms: 1_000,
        }
    }

    fn call() -> ToolCall {
        ToolCall {
            id: "call_0".to_string(),
            name: "search".to_string(),
            input: json!({ "pattern": "rsi" }),
        }
    }

    #[test]
    fn the_context_the_hold_outs_and_a_compaction_reach_the_statistics() {
        use cortex_core::{ContextParts, ContextUse, Usage};

        let mut session = Session::default();

        session.apply(&run_started());

        session.apply(&Event::ExperimentAssigned {
            seed: 7,
            share: 0.25,
            briefing_share: 0.0,
            held_out: vec!["intuition".to_string()],
            speaking: vec!["mind".to_string()],
            briefed: Vec::new(),
        });

        session.apply(&responded(1, "Looking."));

        let chars = ContextParts {
            tool_results: 3_000,
            user: 1_000,
            ..ContextParts::default()
        };

        session.apply(&Event::ContextMeasured {
            step: 1,
            context: Box::new(ContextUse::new(2_000, 40, Some(8_000), chars)),
            quota: None,
        });

        let items = session.items.len();

        // What a held-out adviser would have said is not drawn.
        session.apply(&Event::ContextWithheld {
            step: 1,
            source: "intuition".to_string(),
            text: "Look at money.ts.".to_string(),
            detail: json!({}),
            duration_ms: 1,
        });

        assert_eq!(session.items.len(), items);
        assert_eq!(session.stats.held_out, ["intuition"]);
        assert_eq!(session.stats.speaking, ["mind"]);
        assert_eq!(session.stats.context.unwrap().share, Some(0.25));

        session.apply(&Event::ContextCompacted {
            step: 2,
            kept: 2,
            text: "a summary".to_string(),
            messages_replaced: 5,
            chars_before: 4_000,
            chars_after: 2_000,
            usage: Usage::default(),
            duration_ms: 1,
            automatic: true,
        });

        assert_eq!(session.stats.compactions, 1);
        assert_eq!(session.stats.chars_compacted, 2_000);
        assert_eq!(session.stats.tokens_compacted_estimate, 1_000);
        assert_eq!(session.stats.context.unwrap().tokens, 1_000);

        assert!(matches!(
            session.items.last(),
            Some(Item::Notice { text, tone: Tone::Muted })
                if text.contains("5 oldest messages") && text.contains("filling up")
        ));
    }

    #[test]
    fn a_run_becomes_a_transcript() {
        let mut session = Session::default();

        session.apply(&run_started());
        session.apply(&Event::ModelStarted { step: 1 });

        assert!(session.running);
        assert_eq!(session.stats.cells, [Cell::Thinking]);

        session.apply(&responded(1, "Looking."));
        session.apply(&Event::ToolStarted {
            step: 1,
            call: call(),
        });

        let Some(Item::Tool(tool)) = session.items.last() else {
            panic!("the last item should be the tool call");
        };

        assert!(tool.result.is_none());

        session.apply(&Event::ToolFinished {
            step: 1,
            tool: "search".to_string(),
            result: ToolResult {
                call_id: "call_0".to_string(),
                output: "rsi.py:4".to_string(),
                is_error: false,
                images: Vec::new(),
            },
            duration_ms: 12,
        });

        let Some(Item::Tool(tool)) = session.items.last() else {
            panic!("the last item should still be the tool call");
        };

        assert_eq!(tool.result.as_ref().unwrap().output, "rsi.py:4");
        assert_eq!(tool.duration_ms, 12);

        session.apply(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 1,
            usage: Usage::default(),
            duration_ms: 1_500,
            verified: None,
        });

        assert!(!session.running);
        assert_eq!(session.stats.speed(), Some(50.0));
        assert_eq!(session.stats.cache_hit_ratio(), Some(0.75));
        assert_eq!(session.stats.cells, [Cell::Model, Cell::Observation]);

        assert!(matches!(session.items[0], Item::User { .. }));
        assert!(matches!(
            session.items[1],
            Item::Assistant { speed: Some(_), .. }
        ));
        assert!(matches!(session.items[3], Item::RunSummary { .. }));
    }

    #[test]
    fn text_being_written_is_replaced_by_the_finished_response() {
        let mut session = Session::default();

        session.apply(&run_started());
        session.write("Look");
        session.write("ing.");

        assert_eq!(session.writing.as_ref().unwrap().text, "Looking.");

        session.apply(&responded(1, "Looking."));

        assert!(session.writing.is_none());
    }

    #[test]
    fn a_wait_says_what_it_is_for_until_there_is_something_to_read() {
        let mut session = Session::default();

        session.apply(&run_started());
        assert_eq!(
            session.doing.as_ref().unwrap().activity,
            Activity::Preparing
        );

        session.apply(&Event::ModelStarted { step: 1 });
        session.writing_started();

        let since = session.doing.as_ref().unwrap().since;

        // A retry is a stage of the same wait.
        session.retrying("Rate limited by the provider", Duration::from_secs(30));
        session.writing_started();
        session.calling("read_file");

        let doing = session.doing.as_ref().unwrap();

        assert_eq!(doing.activity, Activity::Calling("read_file".to_string()));
        assert_eq!(doing.since, since);

        session.think("Looking at the loop.");
        assert_eq!(session.doing, None);

        session.apply(&Event::ModelStarted { step: 2 });
        session.apply(&responded(2, "Done."));
        assert_eq!(
            session.doing.as_ref().unwrap().activity,
            Activity::Finishing
        );

        session.stop();
        assert_eq!(session.doing, None);
    }

    #[test]
    fn a_retried_request_starts_the_text_again() {
        let mut session = Session::default();

        session.write("Half a sent");
        session.writing_started();
        session.write("A whole one.");

        assert_eq!(session.writing.as_ref().unwrap().text, "A whole one.");
    }

    #[test]
    fn stopping_drops_the_tool_that_was_still_running() {
        let mut session = Session::default();

        session.apply(&run_started());
        session.apply(&Event::ToolStarted {
            step: 1,
            call: call(),
        });
        session.write("Half");
        session.stop();

        assert!(!session.running);
        assert!(session.writing.is_none());
        assert!(
            !session
                .items
                .iter()
                .any(|item| matches!(item, Item::Tool(_)))
        );
        assert!(matches!(
            session.items.last(),
            Some(Item::Notice {
                tone: Tone::Muted,
                ..
            })
        ));
    }

    #[test]
    fn an_error_ends_the_run() {
        let mut session = Session::default();

        session.apply(&run_started());
        session.apply(&Event::ModelStarted { step: 1 });
        session.fail("the provider is unreachable", false);

        assert!(!session.running);
        assert!(!session.can_continue);
        assert!(session.stats.cells.is_empty());
        assert!(matches!(
            session.items.last(),
            Some(Item::Notice {
                tone: Tone::Bad,
                ..
            })
        ));
    }

    #[test]
    fn a_run_that_can_be_carried_on_says_so_until_it_is() {
        let mut session = Session::default();

        session.apply(&run_started());
        session.apply(&Event::ModelStarted { step: 3 });
        session.fail("request failed with HTTP 429", true);

        assert!(!session.running);
        assert!(session.can_continue);

        session.apply(&Event::RunResumed { steps: 2 });

        assert!(session.running);
        assert!(!session.can_continue);
        assert_eq!(session.stats.resumes, 1);

        // Asking for something else instead gives the chance up.
        session.fail("request failed with HTTP 429", true);
        session.apply(&run_started());

        assert!(!session.can_continue);
    }

    #[test]
    fn folded_repeats_and_pruned_output_are_counted() {
        let mut session = Session::default();

        session.apply(&run_started());
        session.apply(&Event::ToolStarted {
            step: 1,
            call: call(),
        });

        // What the engine leaves in place of a repeated, unchanged result.
        let mut conversation = Vec::new();
        let text = "x".repeat(2_000);

        conversation.push(Message {
            role: cortex_core::Role::Assistant,
            blocks: vec![cortex_core::Block::ToolCall(call())],
        });

        conversation.push(Message::tool_results(vec![ToolResult {
            call_id: call().id,
            output: text.clone(),
            is_error: false,
            images: Vec::new(),
        }]));

        let mut again = ToolResult {
            call_id: call().id,
            output: text,
            is_error: false,
            images: Vec::new(),
        };

        repeat::fold(&conversation, &call(), &mut again);

        session.apply(&Event::ToolFinished {
            step: 1,
            tool: call().name,
            result: again,
            duration_ms: 1,
        });

        session.apply(&Event::ContextPruned {
            step: 2,
            results_trimmed: 1,
            chars_removed: 4_200,
        });

        assert_eq!(session.stats.repeats_folded, 1);
        assert_eq!(session.stats.chars_pruned, 4_200);
    }

    #[test]
    fn a_review_and_attempts_are_shown_and_their_tokens_counted() {
        let mut session = Session::default();

        session.apply(&run_started());

        session.apply(&Event::AttemptStarted {
            attempt: 1,
            of: 3,
            strategy: "reproduce_first".to_string(),
        });

        session.apply(&Event::AttemptFinished {
            attempt: 1,
            strategy: "reproduce_first".to_string(),
            outcome: Some(Outcome::Completed),
            steps: 4,
            usage: Usage::default(),
            verified: Some(true),
            review_score: Some(0.5),
            changed_files: 2,
            error: None,
            duration_ms: 10,
        });

        // The kept attempt's one step, as it is replayed.
        session.apply(&responded(1, "done"));

        session.apply(&Event::ReviewRan {
            step: 1,
            requirements: vec![Requirement {
                text: "the export is fixed too".to_string(),
                met: false,
                why: "untouched".to_string(),
            }],
            feedback: Some("not done".to_string()),
            usage: Usage::default(),
            duration_ms: 5,
        });

        // The run reports more than its one step used: the attempts that
        // were not kept and the review are model calls too.
        session.apply(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 9,
            usage: Usage {
                input_tokens: 900,
                output_tokens: 80,
                cache_read_tokens: 300,
                cache_write_tokens: 0,
            },
            duration_ms: 20,
            verified: Some(true),
        });

        let notices: Vec<&str> = session
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Notice { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(
            notices,
            [
                "Attempt 1 of up to 3 is working in its own copy of the project (reproduce first)",
                "Attempt 1: 4 steps, 2 files changed, checks passed, review 50%",
            ]
        );

        assert!(session.items.iter().any(|item| matches!(
            item,
            Item::Review {
                sent_back: true,
                open: true,
                ..
            }
        )));

        assert_eq!(session.stats.usage.input_tokens, 900);
        assert_eq!(session.stats.usage.output_tokens, 80);
        assert_eq!(session.stats.usage.cache_read_tokens, 300);
    }
}

//! Conversation routes to explicit skills, learned knowledge, or an honest unknown.

use super::{
    Brain, Thought, Utterance,
    knowledge::{Knowledge, question_key},
    now_ms,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const TRACES: usize = 64;
const SESSIONS: usize = 32;
const TURNS: usize = 16;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trace {
    pub inventory: Option<super::quantity::Inventory>,
    pub time_ms: u64,
    pub session: String,
    pub question: String,
    pub answer: String,
    pub method: String,
    pub checked: bool,
    pub steps: Vec<String>,
    pub sources: Vec<u64>,
    pub arithmetic: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Conversation {
    #[serde(default)]
    pub inventory: super::quantity::Inventory,
    pub session: String,
    pub turns: VecDeque<Trace>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cognition {
    pub last_study: Option<super::learning::Study>,
    pub knowledge: Knowledge,
    pub conversations: VecDeque<Conversation>,
    pub traces: VecDeque<Trace>,
    pub questions: VecDeque<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CognitiveView {
    pub last_study: Option<super::learning::Study>,
    pub statements: usize,
    pub dialogue: usize,
    pub formulas: Vec<String>,
    pub traces: Vec<Trace>,
    pub questions: Vec<String>,
    pub processes: Vec<(String, String)>,
}

impl Cognition {
    pub fn view(&self) -> CognitiveView {
        CognitiveView {
            last_study:self.last_study.clone(),
            statements: self.knowledge.statements.len(),
            dialogue: self.knowledge.dialogue.len(),
            formulas: self.knowledge.formulas.iter().map(|f| format!("{} = {} (memory {})",f.target,f.expression,f.memory)).collect(),
            traces: self.traces.iter().cloned().collect(),
            questions: self.questions.iter().cloned().collect(),
            processes: [
                ("experience","Recorded inputs, lessons, outcomes and conversation turns"),
                ("awareness","Measured attention, surprise, confidence and prediction accuracy"),
                ("intuition","Immediate sequence predictions; not checked conclusions"),
                ("thinking","Associative continuation and memory recombination"),
                ("reasoning","Exact arithmetic, explicit learned rules, bounded inference and sequence search"),
                ("decision","Recorded selection of a skill or candidate continuation"),
                ("curiosity","Unanswered questions and surprising words with learning progress"),
            ].into_iter().map(|(a,b)| (a.into(),b.into())).collect(),
        }
    }

    fn last(&self, session: &str) -> Option<&Trace> {
        self.conversations
            .iter()
            .find(|c| c.session == session)?
            .turns
            .back()
    }

    fn record(&mut self, trace: Trace) {
        let at = self
            .conversations
            .iter()
            .position(|c| c.session == trace.session);
        let mut conversation = at
            .and_then(|at| self.conversations.remove(at))
            .unwrap_or_else(|| Conversation {
                session: trace.session.clone(),
                ..Conversation::default()
            });
        if let Some(inventory) = &trace.inventory {
            conversation.inventory = inventory.clone();
        }
        conversation.turns.push_back(trace.clone());
        while conversation.turns.len() > TURNS {
            conversation.turns.pop_front();
        }
        self.conversations.push_back(conversation);
        while self.conversations.len() > SESSIONS {
            self.conversations.pop_front();
        }
        self.traces.push_back(trace);
        while self.traces.len() > TRACES {
            self.traces.pop_front();
        }
    }

    pub fn footprint(&self) -> u64 {
        // Includes string contents and conservative container overhead.
        let knowledge = self
            .knowledge
            .statements
            .iter()
            .map(|s| s.subject.len() + s.relation.len() + s.object.len() + s.source.len() + 160)
            .sum::<usize>()
            + self
                .knowledge
                .dialogue
                .iter()
                .map(|d| d.question.len() + d.answer.len() + d.source.len() + 160)
                .sum::<usize>()
            + self
                .knowledge
                .formulas
                .iter()
                .map(|f| {
                    f.target.len()
                        + f.expression.len()
                        + f.source.len()
                        + f.variables.iter().map(|v| v.len() + 24).sum::<usize>()
                        + 160
                })
                .sum::<usize>();
        let inventory_size = |inventory: &super::quantity::Inventory| {
            inventory
                .counts
                .keys()
                .map(|name| name.len() + 96)
                .sum::<usize>()
                + inventory
                    .history
                    .iter()
                    .map(|(name, steps)| {
                        name.len() + 96 + steps.iter().map(|step| step.len() + 24).sum::<usize>()
                    })
                    .sum::<usize>()
        };
        let trace_size = |t: &Trace| {
            t.question.len()
                + t.answer.len()
                + t.steps.iter().map(|s| s.len() + 24).sum::<usize>()
                + t.sources.len() * 8
                + t.inventory.as_ref().map_or(0, inventory_size)
                + 256
        };
        (knowledge
            + self.traces.iter().map(trace_size).sum::<usize>()
            + self
                .conversations
                .iter()
                .flat_map(|c| &c.turns)
                .map(trace_size)
                .sum::<usize>()
            + self
                .conversations
                .iter()
                .map(|c| inventory_size(&c.inventory))
                .sum::<usize>()) as u64
    }
}

impl Brain {
    pub fn chat(&mut self, session: &str, text: &str) -> Utterance {
        let session: String = session.chars().take(128).collect();
        let text: String = text.chars().take(4096).collect();
        let mut trace = Trace {
            time_ms: now_ms(),
            session: session.clone(),
            question: text.clone(),
            ..Trace::default()
        };
        let lower = text.trim().to_lowercase();
        let key = question_key(&text);
        if matches!(key.as_str(), "learn foundations" | "study foundations") {
            let count = self.foundations();
            trace.answer = format!(
                "I studied {count} introductory lessons in conversation, maths and mechanics. Ask me about gravity, or try ‘what is force if mass is 2 kg and acceleration is 3 m/s^2?’. These are supplied lessons, not discoveries."
            );
            trace.method = "teaching".into();
            trace.steps.push(
                "Rehearsed the bundled curriculum under a fixed four-pass limit per lesson.".into(),
            );
        } else if let Some(lesson) = ["remember that ", "learn that ", "remember: ", "teach: "]
            .iter()
            .find_map(|p| lower.strip_prefix(p))
        {
            let study = self.study(lesson, "human teaching", 4);
            if let Some(id) = study.memory {
                trace.answer = "I recorded that lesson. I will keep its source and check calculations rather than treating every assertion as true.".into();
                trace.method = "teaching".into();
                trace.sources.push(id);
                trace.steps.push(format!("Stored memory {id}; {} rehearsal passes, lesson prediction {:.0}% → {:.0}%. This measures the taught lesson, not held-out accuracy.",study.passes,study.before.accuracy*100.0,study.after.accuracy*100.0));
            } else {
                trace.answer = "Give me a complete statement or formula to learn.".into();
                trace.method = "unknown".into();
            }
        } else if matches!(
            key.as_str(),
            "why" | "why is that" | "explain that" | "how did you get that"
        ) || (key.starts_with("why ") && key[4..].parse::<i64>().is_ok())
        {
            if let Some(last) = self.mind.cognition.last(&session).cloned() {
                trace.answer = if last.method == "follow_up" {
                    last.answer.clone()
                } else if last.steps.is_empty() {
                    "I do not have a checked derivation for that answer.".into()
                } else {
                    format!(
                        "For my previous answer, {} {}",
                        last.answer,
                        last.steps.join(" ")
                    )
                };
                trace.method = "follow_up".into();
                trace.checked = last.checked;
                trace.steps = last.steps;
                trace.sources = last.sources;
                trace.arithmetic = last.arithmetic;
            } else {
                trace.answer = "Which result would you like me to explain?".into();
                trace.method = "unknown".into();
            }
        } else if self.answer_quantity(&session, &text, &mut trace) {
            // Recorded collection operations produce their own explanation.
        } else if let Some(calculation) = super::arithmetic::question(&text) {
            trace.method = "built_in_arithmetic".into();
            match calculation {
                Ok(calculation) => {
                    trace.answer = calculation.answer();
                    trace.arithmetic = Some(calculation.expression);
                    trace.steps = calculation.steps;
                    trace.checked = true;
                    if lower.contains("why") || lower.starts_with("explain") {
                        trace.answer.push(' ');
                        trace.answer.push_str(&trace.steps.join(" "));
                    }
                }
                Err(error) => {
                    trace.answer = error;
                    trace.steps.push(
                        "The arithmetic executor rejected the request; no answer was invented."
                            .into(),
                    );
                }
            }
        } else if self.answer_formula(&text, &mut trace) {
            // Formula execution fills the evidence trace, including failures.
        } else if let Some(answer) = self.mind.cognition.knowledge.answer(&text) {
            trace.answer = answer.text;
            trace.steps = answer.steps;
            trace.sources = answer.memories;
            trace.method = if answer.conflict {
                "conflict"
            } else if answer.deduction {
                "deduction"
            } else {
                "learned_knowledge"
            }
            .into();
            // Premises are learned assertions, not independently verified facts.
            trace.checked = answer.deduction && !answer.conflict;
            if answer.conflict
                && matches!(
                    key.as_str(),
                    "hello" | "how are you" | "thank you" | "goodbye"
                )
            {
                trace.answer = match key.as_str() {
                    "hello" => "Hello. What would you like to explore?",
                    "how are you" => {
                        "I am ready to work through a question or learn a lesson with you."
                    }
                    "thank you" => "You are welcome.",
                    _ => "Goodbye. We can continue from recorded lessons next time.",
                }
                .into();
                trace.method = "built_in_conversation".into();
                trace.steps.push("Selected a neutral greeting from the conversation interface; differing learned greetings were retained as alternatives.".into());
            }
        } else if matches!(key.as_str(), "what do you know" | "what have you learned") {
            let knowledge = &self.mind.cognition.knowledge;
            trace.answer = format!(
                "I have {} learned statements, {} dialogue examples and {} formula rules. I also have a built-in exact arithmetic executor. Ask a question, or say ‘remember that’ followed by a lesson.",
                knowledge.statements.len(),
                knowledge.dialogue.len(),
                knowledge.formulas.len()
            );
            trace.method = "self_monitor".into();
            trace.checked = true;
        } else if matches!(
            key.as_str(),
            "what did you dream" | "tell me your dream" | "what was your dream"
        ) {
            if let Some(dream) = self.mind.thoughts.iter().rev().find(|t| t.kind == "dream") {
                trace.answer = format!(
                    "My last generated dream was: {} It is an imagined recombination, not something observed.",
                    dream.text
                );
                trace.sources = dream.sources.clone();
                trace.steps.push(dream.why.clone());
            } else {
                trace.answer="I have not recorded a dream yet. The Dream control generates one from recorded memories.".into();
            }
            trace.method = "self_monitor".into();
            trace.checked = true;
        } else if matches!(
            key.as_str(),
            "what are you curious about" | "what do you want to learn"
        ) {
            let questions = self
                .mind
                .cognition
                .questions
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>();
            let words = self
                .curiosity(3)
                .into_iter()
                .map(|c| c.word)
                .collect::<Vec<_>>();
            trace.answer = if !questions.is_empty() {
                format!("I still need evidence for: {}", questions.join("; "))
            } else if !words.is_empty() {
                format!(
                    "These words still surprise my sequence predictions: {}.",
                    words.join(", ")
                )
            } else {
                "I need some observations or lessons before I can identify a learning gap.".into()
            };
            trace.method = "self_monitor".into();
            trace.checked = true;
        } else if matches!(
            key.as_str(),
            "what have you experienced" | "what do you remember"
        ) {
            let recent = self.engrams.iter().rev().take(3).collect::<Vec<_>>();
            trace.answer = if recent.is_empty() {
                "I have no recorded observations yet.".into()
            } else {
                format!(
                    "My recent recorded inputs include: {}",
                    recent
                        .iter()
                        .map(|e| format!(
                            "‘{}’",
                            e.tokens
                                .iter()
                                .take(20)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" ")
                        ))
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            };
            trace.sources = recent.iter().map(|e| e.id).collect();
            trace.method = "self_monitor".into();
            trace.checked = true;
        } else if let Some(seed) = lower.strip_prefix("continue ") {
            let said = self.say(&format!("<story> {seed}"));
            trace.answer = if said.text.is_empty() {
                "I have not learned a continuation for that yet.".into()
            } else {
                said.text
            };
            trace.steps.push(said.why);
            trace.method = "neural_continuation".into();
        } else if matches!(
            key.as_str(),
            "are you aware" | "are you conscious" | "what are you thinking" | "how do you feel"
        ) {
            trace.answer = format!(
                "I track my internal activity; that does not demonstrate subjective experience. {}",
                self.report()
            );
            trace.method = "self_monitor".into();
            trace.checked = true;
        } else if matches!(key.as_str(), "hello" | "hi" | "hey") {
            trace.answer = "Hello. We can work through arithmetic, discuss what you teach me, or inspect my activity. What would you like to explore?".into();
            trace.method = "built_in_conversation".into();
        } else if key == "what is your name" {
            trace.answer = "My name is Cortex.".into();
            trace.method = "built_in_conversation".into();
        } else {
            trace.answer = "I don't know enough to answer that reliably yet. Teach me a statement with ‘remember that …’, a formula such as ‘force = mass * acceleration’, or a question/answer example with feedback.".into();
            trace.method = "unknown".into();
            if !self.mind.cognition.questions.contains(&text) {
                self.mind.cognition.questions.push_back(text.clone());
            }
            while self.mind.cognition.questions.len() > 32 {
                self.mind.cognition.questions.pop_front();
            }
        }
        // Hearing a question does not promote it to a factual premise.
        self.learn(&format!("<user> {text}"), "user");
        self.resolve_questions();
        self.finish_answer(trace)
    }

    fn answer_quantity(&self, session: &str, text: &str, trace: &mut Trace) -> bool {
        let state = self
            .mind
            .cognition
            .conversations
            .iter()
            .find(|c| c.session == session)
            .map(|c| c.inventory.clone())
            .unwrap_or_default();
        let Some(result) = state.respond(text) else {
            return false;
        };
        trace.method = "grounded_quantity".into();
        match result {
            Ok(result) => {
                trace.answer = result.answer;
                trace.steps = result.steps;
                trace.checked = true;
                if result.hypothetical {
                    trace.steps.insert(
                        0,
                        "Hypothetical scenario; the actual recorded collection was not changed."
                            .into(),
                    );
                } else {
                    trace.inventory = Some(result.state);
                }
            }
            Err(error) => trace.answer = error,
        }
        true
    }

    pub(crate) fn resolve_questions(&mut self) {
        let knowledge = &self.mind.cognition.knowledge;
        self.mind.cognition.questions.retain(|question| {
            !knowledge.answer(question).is_some_and(|a| !a.conflict)
                && !knowledge
                    .formulas
                    .iter()
                    .any(|f| f.answer(question).is_some_and(|a| a.is_ok()))
        });
    }

    fn answer_formula(&self, text: &str, trace: &mut Trace) -> bool {
        let answers: Vec<_> = self
            .mind
            .cognition
            .knowledge
            .formulas
            .iter()
            .filter_map(|f| f.answer(text).map(|a| (f, a)))
            .collect();
        if answers.is_empty() {
            return false;
        }
        let successes: Vec<_> = answers
            .iter()
            .filter_map(|(_, answer)| answer.as_ref().ok())
            .collect();
        if let Some(first) = successes.first() {
            if successes.iter().any(|answer| answer.text != first.text) {
                trace.method = "conflict".into();
                trace.answer = "My learned formulas give conflicting results. Please correct the rule before I choose an answer.".into();
                trace.sources = successes.iter().map(|a| a.memory).collect();
                trace.steps = successes.iter().flat_map(|a| a.steps.clone()).collect();
            } else {
                trace.method = "learned_formula".into();
                trace.answer = first.text.clone();
                trace.steps = first.steps.clone();
                trace.sources = vec![first.memory];
                trace.checked = true;
            }
        } else {
            trace.method = "learned_formula".into();
            trace.answer = answers[0].1.as_ref().err().unwrap().clone();
            trace.sources = answers.iter().map(|(f, _)| f.memory).collect();
            trace.steps.push(
                "Execution stopped because the rule, givens or units were insufficient.".into(),
            );
        }
        true
    }

    fn finish_answer(&mut self, trace: Trace) -> Utterance {
        let process = if trace.method == "self_monitor" {
            "aware"
        } else if trace.checked {
            "reason"
        } else {
            "decide"
        };
        for token in super::words::tokenize(&trace.question).iter().take(80) {
            self.fire_word(process, token);
        }
        let confidence = if trace.method == "built_in_arithmetic" && trace.checked {
            1.0
        } else if matches!(
            trace.method.as_str(),
            "learned_knowledge" | "deduction" | "learned_formula"
        ) {
            0.5
        } else {
            0.0
        };
        let why = format!(
            "Method: {}. {}{}",
            trace.method,
            if trace.checked {
                "Execution or inference checked; learned premises remain assumptions. "
            } else {
                "No independent correctness guarantee. "
            },
            trace.steps.join(" ")
        );
        self.mind.think(Thought {
            time_ms: now_ms(),
            kind: process.into(),
            seed: trace.question.clone(),
            text: trace.answer.clone(),
            confidence,
            sources: trace.sources.clone(),
            why: why.clone(),
            ..Thought::default()
        });
        let said = Utterance {
            time_ms: now_ms(),
            prompt: trace.question.clone(),
            text: trace.answer.clone(),
            confidence,
            why,
            ..Utterance::default()
        };
        self.stats.spoken += 1;
        self.said.push_back(said.clone());
        while self.said.len() > super::SAID {
            self.said.pop_front();
        }
        self.phase = format!("answered using {}", trace.method);
        self.mind.cognition.record(trace);
        said
    }
}

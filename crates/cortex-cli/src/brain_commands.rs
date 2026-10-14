use anyhow::Result;
use cortex_living::service::{self, Request};
use cortex_living::teacher::Config;

use crate::args::{ModelArgs, ProviderKind};

#[derive(Debug, PartialEq)]
pub enum Command {
    Start(String),
    Stop,
    Status,
    Ask(String),
    Recall(String),
    BrainMode,
    AgentMode,
    Feedback(bool, String),
}

pub fn parse(text: &str) -> Option<Command> {
    let text = text.trim();
    match text {
        "/teach stop" | "/stop-teaching" => return Some(Command::Stop),
        "/teach status" => return Some(Command::Status),
        "/brain" => return Some(Command::BrainMode),
        "/agent" => return Some(Command::AgentMode),
        "/brain-feedback correct" => return Some(Command::Feedback(true, String::new())),
        _ => {}
    }
    for prefix in ["/teach snn+dann", "teach snn+dann", "/teach-snn-dann"] {
        if let Some(tail) = text.strip_prefix(prefix)
            && (tail.is_empty() || tail.starts_with(char::is_whitespace))
        {
            return Some(Command::Start(tail.trim().into()));
        }
    }
    if let Some(question) = text.strip_prefix("/brain ") {
        return Some(Command::Ask(question.trim().into()));
    }
    if let Some(cue) = text.strip_prefix("/recall ") {
        return Some(Command::Recall(cue.trim().into()));
    }
    text.strip_prefix("/brain-feedback wrong ")
        .map(|answer| Command::Feedback(false, answer.trim().into()))
}

pub fn teacher_config(args: &ModelArgs, topic: &str) -> Config {
    let mut config = Config {
        provider: match args.provider {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Codex => "codex",
            ProviderKind::Local => "local",
            ProviderKind::Hosted => "hosted",
        }.into(),
        model: args.model.clone().or_else(|| args.provider.default_model().map(str::to_string)).unwrap_or_default(),
        base_url: args.base_url.clone(),
        effort: args.effort.clone(),
        ..Config::default()
    };
    if !topic.is_empty() { config.topic = topic.into(); }
    config
}

#[derive(Default)]
pub struct Chat {
    pub brain_mode: bool,

    /// What was last said to the brain, and what it answered, for feedback.
    pub last_attempt: Option<(String, String)>,
}

pub struct Reply {
    pub text: String,
    pub monitor_teacher: bool,
}

impl Chat {
    pub async fn handle(&mut self, text: &str, model: &ModelArgs, session: &str) -> Result<Option<Reply>> {
        let Some(command) = parse(text).or_else(|| self.brain_mode.then(|| Command::Ask(text.into()))) else { return Ok(None); };
        if command == Command::BrainMode {
            self.brain_mode = true;
            return Ok(Some(Reply { text:"Talking to the brain. Everything you type goes to it, and it learns from what you say. /brain-feedback correct or /brain-feedback wrong <better answer> teaches it; /agent returns to the coding agent.".into(), monitor_teacher:false }));
        }
        if command == Command::AgentMode {
            self.brain_mode = false;
            return Ok(Some(Reply { text:"Coding agent chat is on.".into(), monitor_teacher:false }));
        }
        let config = teacher_config(model, "");
        let session = session.to_string();
        let last_attempt = self.last_attempt.clone();
        let (reply, attempt) = tokio::task::spawn_blocking(move || -> Result<_> {
            let file = service::default_file()?;
            service::ensure(&file)?;
            let mut attempt = None;
            let (text, monitor_teacher) = match command {
                Command::Start(topic) => {
                    let mut config = config;
                    if !topic.is_empty() { config.topic = topic; }
                    let status = service::request(&file, &Request::TeachStart { config })?;
                    (format!("Teaching started. {}\nIt continues after this chat closes and uses model tokens each round. /teach stop stops it; /brain <words> talks to the brain; /recall <cue> asks it to remember.", describe_status(&status)), true)
                }
                Command::Stop => {
                    let status = service::request(&file, &Request::TeachStop)?;
                    (format!("Teaching stopped. {}", describe_status(&status)), false)
                }
                Command::Status => {
                    let status = service::request(&file, &Request::TeachStatus { after:0 })?;
                    (describe_status(&status), status["active"].as_bool().unwrap_or(false))
                }
                Command::Ask(question) => {
                    let _ = session;
                    let said: cortex_living::neural::Utterance = serde_json::from_value(service::request(&file, &Request::NeuralConverse { text:question.clone() })?)?;
                    let words = if said.text.is_empty() { "…".to_string() } else { said.text.clone() };
                    attempt = Some((question, said.text.clone()));
                    let new = if said.novel { format!(" New sentence it was never taught: {}.", said.new_sentences.join("; ")) } else { String::new() };
                    (format!("Brain: {words}\n[{} Confidence {:.0}%.{new}]", said.why, said.confidence * 100.0), false)
                }
                Command::Recall(cue) => {
                    let recall: cortex_living::neural::Recall = serde_json::from_value(service::request(&file, &Request::NeuralRecall { cue })?)?;
                    let text = match recall.engram {
                        Some(id) => format!("Brain recalls: {}\n[Memory {id}: \"{}\". {}]", recall.recalled, recall.remembered, recall.why),
                        None => format!("Brain recalls nothing. [{}]", recall.why),
                    };
                    (text, false)
                }
                Command::Feedback(correct, better) => {
                    let (said_to, reply) = last_attempt.ok_or_else(|| anyhow::anyhow!("Talk to the brain before giving feedback"))?;
                    let answer = if correct { reply } else { better };
                    if answer.trim().is_empty() {
                        anyhow::bail!("Give the answer it should have said: /brain-feedback wrong <better answer>");
                    }
                    service::request(&file, &Request::NeuralLearn { text:format!("<user> {said_to} <brain> {answer}"), source:"human feedback".into(), correction:!correct })?;
                    (if correct { "The brain learned that its answer was right." } else { "The brain learned the better answer." }.into(), false)
                }
                _ => unreachable!(),
            };
            Ok((Reply { text, monitor_teacher }, attempt))
        }).await??;
        if attempt.is_some() { self.last_attempt = attempt; }
        Ok(Some(reply))
    }
}

pub fn describe_status(status: &serde_json::Value) -> String {
    let mut text = format!("Teacher: {}. Completed rounds: {}; correct: {}; wrong: {}; LLM tokens: {}.", status["phase"].as_str().filter(|s| !s.is_empty()).unwrap_or("not started"), status["rounds"], status["correct"], status["wrong"], status["usage"]["input_tokens"].as_u64().unwrap_or(0) + status["usage"]["output_tokens"].as_u64().unwrap_or(0));
    if let Some(problem) = status["problem"].as_str() { text.push_str(&format!(" {problem}")); }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn teaching_commands_have_precise_boundaries() {
        assert_eq!(parse("teach snn+dann words"), Some(Command::Start("words".into())));
        assert_eq!(parse("/teach snn+dann"), Some(Command::Start(String::new())));
        assert_eq!(parse("/teach stop"), Some(Command::Stop));
        assert_eq!(parse("/brain hello"), Some(Command::Ask("hello".into())));
        assert_eq!(parse("/recall the cat"), Some(Command::Recall("the cat".into())));
        assert_eq!(parse("teach snn+dannish"), None);
        assert_eq!(parse("Explain how to teach snn+dann"), None);
    }
}

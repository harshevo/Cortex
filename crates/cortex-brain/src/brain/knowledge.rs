//! Explicit learned statements, dialogue and rules, with memory provenance.

use super::words::{BRAIN, USER, tokenize};
use serde::{Deserialize, Serialize};

const MAX_LESSONS: usize = 10_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub subject: String,
    pub relation: String,
    pub object: String,
    pub memory: u64,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Dialogue {
    pub question: String,
    pub answer: String,
    pub memory: u64,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Knowledge {
    pub statements: Vec<Statement>,
    pub dialogue: Vec<Dialogue>,
    pub formulas: Vec<super::formula::Formula>,
}

pub fn words(text: &str) -> String {
    tokenize(text)
        .into_iter()
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn question_key(text: &str) -> String {
    let mut text = tokenize(text)
        .into_iter()
        .filter(|t| !matches!(t.as_str(), "." | "?" | "!" | ","))
        .collect::<Vec<_>>()
        .join(" ")
        .replace("what's ", "what is ")
        .replace("who's ", "who is ");
    for prefix in [
        "please ",
        "do you know ",
        "can you tell me ",
        "could you tell me ",
        "can you explain ",
        "could you explain ",
        "tell me ",
    ] {
        if let Some(tail) = text.strip_prefix(prefix) {
            text = tail.to_string();
        }
    }
    for prefix in ["explain ", "describe ", "define ", "tell me about "] {
        if matches!(text.as_str(), "explain that" | "describe that") {
            break;
        }
        if let Some(tail) = text.strip_prefix(prefix) {
            text = format!(
                "what is {}",
                tail.trim_start_matches("what ").trim_end_matches(" is")
            );
            break;
        }
    }
    match text.as_str() {
        "what are you called" | "what's your name" | "your name" => "what is your name".into(),
        "hi" | "hey" => "hello".into(),
        _ => text,
    }
}

pub fn concept(text: &str) -> String {
    let text = words(text);
    let text = ["all ", "a ", "an ", "the "]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .unwrap_or(&text);
    if !text.contains(' ') {
        match text {
            "children" => return "child".into(),
            "people" => return "person".into(),
            "mice" => return "mouse".into(),
            "geese" => return "goose".into(),
            "physics" | "mathematics" | "news" | "series" | "species" | "gas" | "cosmos" => {
                return text.into();
            }
            _ => {}
        }
        if let Some(stem) = text.strip_suffix("ies") {
            return format!("{stem}y");
        }
        if ["ches", "shes", "xes", "sses"]
            .iter()
            .any(|ending| text.ends_with(ending))
        {
            return text[..text.len() - 2].into();
        }
    }
    if text.ends_with('s')
        && !["ss", "is", "us"]
            .iter()
            .any(|ending| text.ends_with(ending))
        && !text.contains(' ')
    {
        text[..text.len() - 1].to_string()
    } else {
        text.into()
    }
}

impl Knowledge {
    pub fn learn(&mut self, text: &str, source: &str, memory: u64, correction: bool) -> usize {
        if source == "user" {
            return 0;
        }
        let text = text.trim();
        let before = self.statements.len() + self.dialogue.len() + self.formulas.len();
        if text.contains(USER) && text.contains(BRAIN) {
            for turn in text.split(USER).skip(1) {
                let Some((question, answer)) = turn.split_once(BRAIN) else {
                    continue;
                };
                let question = question_key(question);
                let answer = super::engram::join(&tokenize(answer.trim()))
                    .chars()
                    .take(2048)
                    .collect::<String>();
                if question.is_empty() || answer.is_empty() {
                    continue;
                }
                if correction {
                    self.dialogue.retain(|d| d.question != question);
                }
                if !self
                    .dialogue
                    .iter()
                    .any(|d| d.question == question && d.answer == answer)
                {
                    self.dialogue.push(Dialogue {
                        question,
                        answer: answer.clone(),
                        memory,
                        source: source.into(),
                    });
                }
                // Dialogue can contain hypothetical, quoted or contextual
                // claims. Only an explicit declarative lesson adds premises.
            }
        } else if !text.contains(USER) {
            self.statements_from(
                text.trim_start_matches("<story>"),
                source,
                memory,
                correction,
            );
        }
        trim(&mut self.statements);
        trim(&mut self.dialogue);
        trim(&mut self.formulas);
        (self.statements.len() + self.dialogue.len() + self.formulas.len()).saturating_sub(before)
    }

    fn statements_from(&mut self, text: &str, source: &str, memory: u64, correction: bool) {
        // Tokenization preserves decimal points within numbers.
        let tokens = tokenize(text);
        for sentence in tokens.split(|t| matches!(t.as_str(), "." | "!" | "?" | "<story>")) {
            if sentence.len() > 100 || sentence.is_empty() {
                continue;
            }
            let raw = sentence.join(" ");
            if let Some(formula) = super::formula::Formula::learn(&raw, source, memory) {
                if correction {
                    self.formulas.retain(|f| f.target != formula.target);
                }
                if !self
                    .formulas
                    .iter()
                    .any(|f| f.target == formula.target && f.expression == formula.expression)
                {
                    self.formulas.push(formula);
                }
                continue;
            }
            if matches!(
                sentence[0].as_str(),
                "what"
                    | "why"
                    | "where"
                    | "who"
                    | "how"
                    | "is"
                    | "are"
                    | "if"
                    | "can"
                    | "does"
                    | "do"
            ) {
                continue;
            }
            let Some(at) = sentence
                .iter()
                .position(|t| matches!(t.as_str(), "is" | "are"))
            else {
                continue;
            };
            let subject = concept(&sentence[..at].join(" "));
            let mut tail = &sentence[at + 1..];
            let relation = if tail.first().is_some_and(|t| t == "not") {
                tail = &tail[1..];
                "is_not"
            } else if sentence[0] == "all"
                || tail
                    .first()
                    .is_some_and(|t| matches!(t.as_str(), "a" | "an"))
            {
                "is_a"
            } else {
                "is"
            };
            let object = concept(&tail.join(" "));
            if subject.is_empty() || object.is_empty() || subject.len() > 100 || object.len() > 500
            {
                continue;
            }
            // First-person context belongs to dialogue, not a universal fact.
            if matches!(subject.as_str(), "i" | "you" | "it" | "this" | "that") {
                continue;
            }
            if correction {
                self.statements.retain(|f| f.subject != subject);
            }
            if !self
                .statements
                .iter()
                .any(|f| f.subject == subject && f.relation == relation && f.object == object)
            {
                self.statements.push(Statement {
                    subject,
                    relation: relation.into(),
                    object,
                    memory,
                    source: source.into(),
                });
            }
        }
    }

    pub fn answer(&self, question: &str) -> Option<LearnedAnswer> {
        let key = question_key(question);
        let answers: Vec<&Dialogue> = self.dialogue.iter().filter(|d| d.question == key).collect();
        if let Some(first) = answers.first() {
            let different = answers
                .iter()
                .any(|d| words(&d.answer) != words(&first.answer));
            return Some(LearnedAnswer {
                text: if different {
                    "I have conflicting learned answers. Please correct the answer I should use."
                        .into()
                } else {
                    first.answer.clone()
                },
                steps: answers
                    .iter()
                    .map(|d| format!("Memory {} ({}): {}", d.memory, d.source, d.answer))
                    .collect(),
                memories: answers.iter().map(|d| d.memory).collect(),
                conflict: different,
                deduction: false,
            });
        }
        if let Some(subject) = [
            "what is ",
            "what are ",
            "who is ",
            "tell me about ",
            "define ",
        ]
        .iter()
        .find_map(|p| key.strip_prefix(p))
        {
            let subject = concept(subject);
            let formulas: Vec<_> = self
                .formulas
                .iter()
                .filter(|f| f.target.replace('_', " ") == subject)
                .collect();
            if !formulas.is_empty() {
                return Some(LearnedAnswer {
                    text: formulas
                        .iter()
                        .map(|f| {
                            format!(
                                "According to my learned rule, {} = {}.",
                                f.target.replace('_', " "),
                                f.expression
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                    steps: formulas
                        .iter()
                        .map(|f| format!("Rule supplied by {} in memory {}.", f.source, f.memory))
                        .collect(),
                    memories: formulas.iter().map(|f| f.memory).collect(),
                    conflict: formulas.len() > 1,
                    deduction: false,
                });
            }
            let facts: Vec<&Statement> = self
                .statements
                .iter()
                .filter(|f| f.subject == subject)
                .collect();
            if facts.is_empty() {
                return None;
            }
            let conflict = facts.iter().any(|a| {
                facts.iter().any(|b| {
                    a.object == b.object && (a.relation == "is_not") != (b.relation == "is_not")
                })
            });
            return Some(LearnedAnswer {
                text: if conflict {
                    format!(
                        "My lessons disagree about {subject}. I need a correction before answering."
                    )
                } else {
                    facts
                        .iter()
                        .map(|f| {
                            format!(
                                "{} is {}{}.",
                                subject,
                                if f.relation == "is_not" { "not " } else { "" },
                                f.object
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                },
                steps: facts
                    .iter()
                    .map(|f| {
                        format!(
                            "Memory {} ({}): {} {} {}",
                            f.memory, f.source, f.subject, f.relation, f.object
                        )
                    })
                    .collect(),
                memories: facts.iter().map(|f| f.memory).collect(),
                conflict,
                deduction: false,
            });
        }
        if let Some(tail) = key.strip_prefix("is ").or_else(|| key.strip_prefix("are ")) {
            // Try every subject/object boundary; accept only stored concepts.
            let parts: Vec<&str> = tail.split_whitespace().collect();
            for at in 1..parts.len() {
                let subject = concept(&parts[..at].join(" "));
                let object = concept(&parts[at..].join(" "));
                if let Some(answer) = self.deduce(&subject, &object) {
                    return Some(answer);
                }
            }
        }
        None
    }

    fn deduce(&self, subject: &str, object: &str) -> Option<LearnedAnswer> {
        let mut open =
            std::collections::VecDeque::from([(subject.to_string(), Vec::<&Statement>::new())]);
        let mut visited = std::collections::HashSet::new();
        let mut positive = None;
        let mut negative = None;
        while let Some((current, path)) = open.pop_front() {
            if path.len() >= 8 || !visited.insert(current.clone()) || visited.len() > 256 {
                continue;
            }
            for fact in self.statements.iter().filter(|f| f.subject == current) {
                let mut path = path.clone();
                path.push(fact);
                if fact.object == object {
                    if fact.relation != "is_not" {
                        positive = Some(path.clone());
                    } else if current == subject {
                        negative = Some(path.clone());
                    }
                }
                // Only category-shaped terms are chained. Descriptive phrases
                // and negative statements are never treated as equivalences.
                if fact.relation == "is_a" && !fact.object.contains(' ') {
                    open.push_back((fact.object.clone(), path));
                }
            }
        }
        let conflict = positive.is_some() && negative.is_some();
        let yes = positive.is_some();
        let mut path = positive.or_else(|| negative.clone())?;
        if conflict {
            path.extend(negative.unwrap());
        }
        Some(LearnedAnswer {
            text: if conflict {
                format!("My lessons disagree about whether {subject} is {object}.")
            } else if yes {
                format!("Yes, according to my learned premises, {subject} is {object}.")
            } else {
                format!("No, according to my learned premise, {subject} is not {object}.")
            },
            steps: path
                .iter()
                .map(|f| {
                    format!(
                        "{} {} {} (memory {}, {}).",
                        f.subject, f.relation, f.object, f.memory, f.source
                    )
                })
                .collect(),
            memories: path.iter().map(|f| f.memory).collect(),
            conflict,
            deduction: true,
        })
    }
}

fn trim<T>(items: &mut Vec<T>) {
    if items.len() > MAX_LESSONS {
        items.drain(..items.len() - MAX_LESSONS);
    }
}

pub struct LearnedAnswer {
    pub text: String,
    pub steps: Vec<String>,
    pub memories: Vec<u64>,
    pub conflict: bool,
    pub deduction: bool,
}

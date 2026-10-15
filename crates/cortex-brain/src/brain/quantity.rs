//! A small grounded inventory: observations change quantities, queries read them.
//! Only the declared event grammar is accepted; unsupported stories abstain.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub counts: BTreeMap<String, i64>,
    #[serde(default)]
    pub history: BTreeMap<String, Vec<String>>,
}

pub struct Result {
    pub state: Inventory,
    pub answer: String,
    pub steps: Vec<String>,
    pub hypothetical: bool,
}

fn count(text: &str) -> Option<i64> {
    [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ]
    .iter()
    .position(|word| *word == text)
    .map(|at| at as i64)
    .or_else(|| text.parse().ok())
    .filter(|value| (0..=1_000_000).contains(value))
}

fn object(text: &str) -> String {
    super::knowledge::concept(text)
}

impl Inventory {
    pub fn respond(&self, text: &str) -> Option<std::result::Result<Result, String>> {
        let text = text.to_lowercase().replace('’', "'");
        let mut text = text.trim().trim_end_matches(['?', '.', '!']).trim();
        let hypothetical = text.starts_with("if ");
        if hypothetical {
            text = &text[3..];
        }
        let question = Regex::new(r"^how many ([a-z]+) do i have(?: now| left)?$").unwrap();
        if let Some(query) = question.captures(text) {
            let name = object(&query[1]);
            let quantity = self.counts.get(&name)?;
            let mut steps = self.history.get(&name).cloned().unwrap_or_default();
            steps.push(format!("Read the recorded count of {name}: {quantity}."));
            return Some(Ok(Result {
                state: self.clone(),
                answer: format!("You have {quantity} {}.", &query[1]),
                steps,
                hypothetical,
            }));
        }
        let mut query = None;
        let statement = if let Some((before, after)) = text
            .split_once(", how many ")
            .or_else(|| text.split_once(". how many "))
        {
            let question_text = format!("how many {after}");
            query = question.captures(&question_text).map(|c| object(&c[1]));
            // A partially understood question must not change actual state.
            query.as_ref()?;
            before
        } else {
            text
        };
        let event=Regex::new(r"^(?:i )?(have|had|get|receive|gain|lose|eat|give away|remove|add) ([a-z]+|[0-9]+) ([a-z]+)$").unwrap();
        let mut parsed = Vec::new();
        for clause in statement.split(" and ") {
            let captures = event.captures(clause.trim())?;
            parsed.push((
                captures[1].to_string(),
                count(&captures[2])?,
                object(&captures[3]),
            ));
        }
        if parsed.is_empty() {
            return None;
        }
        let mut state = self.clone();
        let mut steps = Vec::new();
        let mut last = None;
        for (verb, quantity, name) in parsed {
            let name = if name == "more" {
                match last.clone().or_else(|| {
                    (state.counts.len() == 1).then(|| state.counts.keys().next().unwrap().clone())
                }) {
                    Some(name) => name,
                    None => {
                        return Some(Err(
                            "More of which object? Name the object so I can keep separate counts."
                                .into(),
                        ));
                    }
                }
            } else {
                name
            };
            if !state.counts.contains_key(&name) && state.counts.len() >= 32 {
                return Some(Err(
                    "This conversation already tracks 32 object types.".into()
                ));
            }
            if matches!(verb.as_str(), "have" | "had") {
                state.counts.insert(name.clone(), quantity);
                state.history.remove(&name);
                steps.push(format!(
                    "Observed an initial collection of {quantity} {name} objects."
                ));
            } else {
                let Some(before) = state.counts.get(&name).copied() else {
                    return Some(Err(format!(
                        "How many {name} objects did you have before that event?"
                    )));
                };
                let adding = matches!(verb.as_str(), "get" | "receive" | "gain" | "add");
                let after = if adding {
                    before.checked_add(quantity)
                } else {
                    before.checked_sub(quantity)
                };
                let Some(after) = after.filter(|after| (0..=1_000_000).contains(after)) else {
                    return Some(Err("That event would produce a negative count or exceed the quantity budget. The recorded count was left unchanged.".into()));
                };
                if adding && quantity <= 20 {
                    steps.push(format!("Add {quantity} to {before}: {}. The collection contains {after} {name} objects.",(0..=quantity).map(|n| (before+n).to_string()).collect::<Vec<_>>().join(" → ")));
                } else {
                    steps.push(format!(
                        "{verb}: {before} {} {quantity} = {after} {name} objects.",
                        if adding { "+" } else { "-" }
                    ));
                }
                state.counts.insert(name.clone(), after);
            }
            let history = state.history.entry(name.clone()).or_default();
            history.push(steps.last().unwrap().clone());
            if history.len() > 8 {
                history.drain(..history.len() - 8);
            }
            last = Some(name);
        }
        let name = query.or(last)?;
        let Some(quantity) = state.counts.get(&name) else {
            return Some(Err(format!("No count for {name} was supplied.")));
        };
        let answer = format!(
            "{} {quantity} {name} object{}.",
            if hypothetical {
                "In that scenario, you would have"
            } else {
                "You have"
            },
            if *quantity == 1 { "" } else { "s" }
        );
        Some(Ok(Result {
            state,
            answer,
            steps,
            hypothetical,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collections_transfer_to_unseen_numbers_and_track_events() {
        for a in 0..10 {
            for b in 0..10 {
                let result = Inventory::default()
                    .respond(&format!(
                        "if i have {a} apples and get {b} more, how many apples do i have?"
                    ))
                    .unwrap()
                    .unwrap();
                assert_eq!(result.state.counts["apple"], a + b);
                assert!(result.hypothetical);
            }
        }
        let result = Inventory::default()
            .respond("i have one apple and get three more")
            .unwrap()
            .unwrap();
        assert_eq!(result.state.counts["apple"], 4);
        assert!(result.steps[1].contains("1 → 2 → 3 → 4"));
        assert!(result.state.respond("i eat 5 apples").unwrap().is_err());
        assert_eq!(result.state.counts["apple"], 4);
        assert!(
            result
                .state
                .respond("i get three more and aliens steal half")
                .is_none()
        );
    }
}

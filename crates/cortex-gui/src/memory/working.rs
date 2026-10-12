//! Short-term memory: what the engine's working memory holds for the
//! current run, one thought for each thing in it.

use cortex_memory::WorkingMemory;

use super::graph::Thought;
use crate::format;

/// Working memory can grow long; the picture stays readable with only the
/// most recent of each kind.
const MAX_PER_KIND: usize = 8;

/// The task first, then what was read, searched for, changed, run, got
/// wrong and written down.
pub fn thoughts(memory: &WorkingMemory) -> Vec<Thought> {
    if memory.instruction.is_empty() {
        return Vec::new();
    }

    let mut thoughts = vec![Thought {
        key: "task".to_string(),
        title: format::one_line(&memory.instruction, 70),
        detail: "The task it is working on".to_string(),
    }];

    let commands: Vec<String> = memory
        .commands
        .iter()
        .map(|record| record.command.clone())
        .collect();

    let kinds: [(&str, &str, &[String]); 7] = [
        ("read", "A file it has read", &memory.files_read),
        ("search", "Something it searched for", &memory.searches),
        ("change", "A file it has changed", &memory.files_changed),
        ("run", "A command it has run", &commands),
        ("failure", "Something that went wrong", &memory.failures),
        ("note", "Something it wrote down", &memory.notes),
        (
            "calculation",
            "A calculation evaluated locally (double precision)",
            &memory.calculations,
        ),
    ];

    for (kind, detail, items) in kinds {
        let start = items.len().saturating_sub(MAX_PER_KIND);

        for item in &items[start..] {
            thoughts.push(Thought {
                key: format!("{kind}:{item}"),
                title: format::one_line(item, 70),
                detail: detail.to_string(),
            });
        }
    }

    thoughts
}

#[cfg(test)]
mod tests {
    use cortex_core::{Event, ToolCall, ToolResult};
    use serde_json::json;

    use super::*;

    fn tool(memory: &mut WorkingMemory, id: &str, name: &str, input: serde_json::Value) {
        memory.observe(&Event::ToolStarted {
            step: 1,
            call: ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                input,
            },
        });

        memory.observe(&Event::ToolFinished {
            step: 1,
            tool: name.to_string(),
            result: ToolResult {
                call_id: id.to_string(),
                output: "ok".to_string(),
                is_error: false,
            },
            duration_ms: 1,
        });
    }

    #[test]
    fn an_idle_agent_has_nothing_in_mind() {
        assert!(thoughts(&WorkingMemory::default()).is_empty());
    }

    #[test]
    fn what_the_run_touches_becomes_thoughts() {
        let mut memory = WorkingMemory::default();

        memory.observe(&Event::RunStarted {
            instruction: "fix the window".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });

        tool(&mut memory, "a", "read_file", json!({ "path": "rsi.py" }));
        tool(&mut memory, "b", "read_file", json!({ "path": "rsi.py" }));

        let thoughts = thoughts(&memory);

        assert_eq!(thoughts[0].key, "task");
        assert_eq!(thoughts.len(), 2);
        assert_eq!(thoughts[1].key, "read:rsi.py");
    }
}

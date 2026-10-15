---
name: teach-snn-dann
description: Start, inspect, or stop Cortex's persistent SNN plus dendritic teaching loop, or talk directly to its learner.
---

Use `/Users/harshevo/.cortex/bin/cortex-brain-service`. The service owns the persistent brain independently of this chat.

When the human invokes this skill to teach, run `teach-start` with their optional topic. It reads the selected teacher provider/model from Cortex's desktop settings and runs until stopped. Inspect `teach-status` afterward and report active state or provider errors. If a loop is already active, inspect it rather than starting a duplicate. Do not keep this Codex turn alive with a polling loop: the background service continues teaching after the turn ends, including after Cortex closes. Explain that it uses model tokens and show the stop command.

- Stop: `teach-stop` cancels the teacher, including a pending model request; learned state and background replay remain.
- Status: `teach-status` reports counts, usage, errors, and recent lesson/quiz/answer/correction events.
- Talk: `say '<words>'` returns the brain's actual reply as JSON (`text`, `confidence`, `why`, `novel`). Present it as the brain's response, without silently substituting an LLM answer. For a back-and-forth the human can run `chat` in a terminal themselves.
- Manual teaching: `learn '<short simple sentences>'`, or `learn '<user> prompt <brain> response'` for a conversation turn.
- Feedback: `learn '<user> what was said <brain> its reply'` when it was right, or `correct '<user> what was said <brain> better answer'` when it was wrong.
- Remembering and its own mind: `recall '<cue>'`, `think`, `dream`, `reflect`; `brain` shows its progress.

Quote shell arguments correctly; never put API keys into commands or lesson data. Cortex's own coding sessions already feed the learner when its Brain capability is enabled. For this Codex session, send relevant teaching examples through `lesson` when the human asks to teach from this discussion; do not claim all Codex conversations are automatically observed.

The learner uses word overlap and spiking features to retrieve taught examples. The loop gives lessons, tests with separate questions, grades actual answers, and corrects mistakes. Teacher judgments are unverified. Distinguish recall, paraphrase, transfer, and retention. Do not claim fluent language, human-like consciousness, or guaranteed intelligence growth.

Inside Cortex chat, equivalent commands are `/teach snn+dann [topic]`, `/teach stop`, `/teach status`, `/brain <question>`, `/brain` to enter learner chat, and `/agent` to return to the coding agent.

---
name: teach
description: Start, inspect, or stop the LLM teacher of the user's standalone spiking/dendritic brain, which learns to talk the way a small child does.
---

Teach the brain according to `$ARGUMENTS`.

- `snn+dann [topic]`: call `teach_brain` with action `start` and the optional topic. It uses the selected provider and model. The human invocation authorizes a loop until they stop it; do not set a round limit. If a loop is already running, report its status instead of starting another.
- `stop`: call `teach_brain` with action `stop`. The brain keeps living in the background service: it sleeps, replays its memories and prunes.
- `status`: call `teach_brain` with action `status` and summarize rounds, right and wrong answers, errors, and the latest lesson, quiz, answer and correction.

Each round the teacher reads the brain's progress (vocabulary, next-word accuracy, what it said lately, the words it is still surprised by), writes a short simple story and one to three conversation turns for it to learn, quizzes it (continue a sentence, reply to something said, or recall an earlier lesson), grades what the brain actually said, and teaches a better answer when it was wrong. Only the quiz prompt reaches the brain before it answers. The brain also says something of its own each round, and sentences it was never taught are marked as new.

Tell the human the loop continues after the session closes and spends model tokens each round. Show `/teach stop`, `/teach status`, `/brain <words>` to talk to it, `/brain` to keep talking, `/recall <cue>` to ask it to remember, `/brain-feedback correct` or `/brain-feedback wrong <better answer>` to teach it from a conversation, and `/agent` to return to the coding agent.

For a lesson of your own use `brain_chat` with action `teach`; to test it use `say` or `recall`; `progress` shows how far it has come. The brain is not a language model; never present its words as reasoning or report it as aware.

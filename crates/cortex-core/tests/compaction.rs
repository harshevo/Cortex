//! Compacting a conversation, driven through the loop with a scripted
//! model.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::compact::is_summary;
use cortex_core::{
    CompactConfig, Event, Extras, Message, ModelProvider, ModelRequest, ModelResponse,
    ProviderError, Role, replay,
};
use cortex_providers::ScriptedProvider;

use common::{agent, calls_tool, context, is_well_formed, says};

/// Compacts at 800 of a window of 1,000 tokens and keeps two responses.
fn extras() -> Extras {
    Extras {
        compact: Some(CompactConfig {
            keep_recent: 2,
            auto_at: Some(0.8),
        }),
        context_window: Some(1_000),
        ..Extras::default()
    }
}

fn compactions(events: &[Event]) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| matches!(event, Event::ContextCompacted { .. }))
        .collect()
}

#[tokio::test]
async fn a_conversation_that_fills_the_window_is_compacted_once() {
    let (_dir, context) = context();

    let script = ScriptedProvider::new(vec![
        calls_tool(300),
        calls_tool(500),
        // Over the line: the conversation is compacted before step 4.
        calls_tool(850),
        says("## Requests\nFill things. In progress.", 600),
        calls_tool(400),
        calls_tool(500),
        says("Done.", 600),
    ]);

    let (agent, heard) = agent(Arc::new(script), context, extras());

    let mut conversation = Vec::new();

    let report = agent.run(&mut conversation, "fill things").await.unwrap();

    // The summary call is not a step, and its tokens are in the total.
    assert_eq!(report.steps, 6);
    assert_eq!(
        report.usage.input_tokens,
        300 + 500 + 850 + 600 + 400 + 500 + 600
    );

    let events = heard.events();
    let compacted = compactions(&events);

    assert_eq!(compacted.len(), 1);

    let Event::ContextCompacted {
        step,
        kept,
        messages_replaced,
        chars_before,
        chars_after,
        usage,
        automatic,
        ..
    } = compacted[0]
    else {
        unreachable!()
    };

    // The task and the first response with its result went; the two
    // latest responses and their results stayed.
    assert_eq!((*step, *kept, *messages_replaced), (4, 4, 3));
    assert!(chars_after < chars_before);
    assert_eq!(usage.input_tokens, 600);
    assert!(*automatic);

    assert!(is_summary(&conversation[0].text()));
    assert!(conversation[0].text().contains("Fill things. In progress."));
    assert_eq!(conversation[1].role, Role::Assistant);
    assert!(is_well_formed(&conversation));

    // The log replays to the conversation as it now is.
    assert_eq!(replay::conversation(&events), conversation);
}

#[tokio::test]
async fn it_does_not_compact_again_when_compacting_did_not_help() {
    let (_dir, context) = context();

    let script = ScriptedProvider::new(vec![
        calls_tool(300),
        calls_tool(500),
        calls_tool(900),
        says("A summary.", 600),
        // Still over the line after the summary: no use trying again.
        calls_tool(950),
        calls_tool(960),
        calls_tool(970),
        says("Done.", 980),
    ]);

    let (agent, heard) = agent(Arc::new(script), context, extras());

    let mut conversation = Vec::new();

    let report = agent.run(&mut conversation, "fill things").await.unwrap();

    assert_eq!(report.steps, 7);
    assert_eq!(compactions(&heard.events()).len(), 1);
    assert!(is_well_formed(&conversation));
}

#[tokio::test]
async fn it_compacts_again_once_the_conversation_has_grown_back() {
    let (_dir, context) = context();

    let script = ScriptedProvider::new(vec![
        calls_tool(300),
        calls_tool(500),
        calls_tool(900),
        says("The first summary.", 600),
        calls_tool(300),
        calls_tool(500),
        calls_tool(900),
        says("The second summary, which holds the first.", 600),
        says("Done.", 300),
    ]);

    let (agent, heard) = agent(Arc::new(script), context, extras());

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fill things").await.unwrap();

    let events = heard.events();

    assert_eq!(compactions(&events).len(), 2);
    assert!(conversation[0].text().contains("The second summary"));
    assert!(!conversation[0].text().contains("The first summary."));
    assert!(is_well_formed(&conversation));
    assert_eq!(replay::conversation(&events), conversation);
}

#[tokio::test]
async fn a_window_that_is_not_known_is_never_compacted_unasked() {
    let (_dir, context) = context();

    let script = ScriptedProvider::new(vec![
        calls_tool(900_000),
        calls_tool(950_000),
        calls_tool(990_000),
        says("Done.", 999_000),
    ]);

    let unknown = Extras {
        context_window: None,
        ..extras()
    };

    let (agent, heard) = agent(Arc::new(script), context, unknown);

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fill things").await.unwrap();

    assert!(compactions(&heard.events()).is_empty());

    // The size is still reported, without a share.
    let measured = heard.events().into_iter().find_map(|event| match event {
        Event::ContextMeasured { context, .. } => Some(context),
        _ => None,
    });

    let measured = measured.unwrap();

    assert_eq!(measured.tokens, 900_000);
    assert_eq!(measured.window, None);
    assert_eq!(measured.share, None);
}

/// Asks by hand, keeping one response.
fn by_hand() -> Extras {
    Extras {
        compact: Some(
            CompactConfig {
                keep_recent: 1,
                ..CompactConfig::default()
            }
            .manual(),
        ),
        ..Extras::default()
    }
}

#[tokio::test]
async fn an_idle_session_is_compacted_when_asked_and_replays_the_same() {
    let (_dir, context) = context();

    let script = ScriptedProvider::new(vec![
        calls_tool(100),
        calls_tool(200),
        says("Filled twice.", 300),
        // The summary, asked for by hand.
        says("## Requests\n\"fill things\": done.", 350),
        // The next instruction.
        calls_tool(120),
        says("Filled once more.", 150),
    ]);

    let (agent, heard) = agent(Arc::new(script), context, by_hand());

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fill things").await.unwrap();

    assert_eq!(conversation.len(), 6);

    let compaction = agent.compact(&mut conversation).await.unwrap().unwrap();

    assert_eq!(compaction.messages_replaced, 5);
    assert_eq!(compaction.kept, 1);
    assert!(compaction.chars_removed() > 400);
    assert_eq!(compaction.usage.input_tokens, 350);

    // The summary, then the model's last answer.
    assert_eq!(conversation.len(), 2);
    assert!(is_summary(&conversation[0].text()));
    assert!(conversation[0].text().contains("\"fill things\": done."));
    assert_eq!(conversation[1], Message::assistant_text("Filled twice."));

    // Between runs it is step 0, and not said to be automatic.
    let events = heard.events();

    assert!(matches!(
        compactions(&events)[0],
        Event::ContextCompacted {
            step: 0,
            automatic: false,
            ..
        }
    ));

    assert_eq!(replay::conversation(&events), conversation);

    // The session carries on from the summary.
    let report = agent.run(&mut conversation, "once more").await.unwrap();

    assert_eq!(report.final_text, "Filled once more.");
    assert!(is_well_formed(&conversation));
    assert_eq!(replay::conversation(&heard.events()), conversation);
}

#[tokio::test]
async fn too_short_a_conversation_is_left_alone_and_costs_nothing() {
    let (_dir, context) = context();

    let script = ScriptedProvider::new(vec![says("Hello.", 100)]);

    // The capability is off. Asking by hand still works, with the
    // settings that suit the provider: six responses kept.
    let (agent, heard) = agent(Arc::new(script), context, Extras::default());

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "hello").await.unwrap();

    let before = conversation.clone();

    // The script has nothing left: a model call would fail.
    assert_eq!(agent.compact(&mut conversation).await.unwrap(), None);
    assert_eq!(conversation, before);
    assert!(compactions(&heard.events()).is_empty());
}

#[tokio::test]
async fn with_nothing_kept_the_summary_is_the_whole_conversation() {
    let (_dir, context) = context();

    let script = ScriptedProvider::new(vec![
        calls_tool(300),
        calls_tool(900),
        says("## Next\nFill once more.", 500),
        says("Done.", 200),
    ]);

    // What the Anthropic provider is given: nothing kept beside the
    // summary, so no reasoning from before it is sent back.
    let nothing_kept = Extras {
        compact: Some(CompactConfig {
            keep_recent: 0,
            auto_at: Some(0.8),
        }),
        context_window: Some(1_000),
        ..Extras::default()
    };

    let (agent, heard) = agent(Arc::new(script), context, nothing_kept);

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fill things").await.unwrap();

    // The summary, told to carry on, and the answer to it.
    assert_eq!(conversation.len(), 2);
    assert!(is_summary(&conversation[0].text()));
    assert!(
        conversation[0]
            .text()
            .ends_with("where the summary leaves off.")
    );
    assert_eq!(conversation[1], Message::assistant_text("Done."));

    assert_eq!(replay::conversation(&heard.events()), conversation);
}

/// A model that answers from a script but cannot write a summary: a
/// request without tools fails.
struct NoSummaries(ScriptedProvider);

#[async_trait]
impl ModelProvider for NoSummaries {
    fn name(&self) -> &str {
        "scripted"
    }

    fn model(&self) -> &str {
        "scripted"
    }

    async fn complete(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        if request.tools.is_empty() {
            return Err(ProviderError::Network("the line went dead".to_string()));
        }

        self.0.complete(request).await
    }
}

#[tokio::test]
async fn a_summary_that_cannot_be_written_leaves_the_conversation_as_it_was() {
    let (_dir, context) = context();

    let script = ScriptedProvider::new(vec![
        calls_tool(300),
        calls_tool(500),
        calls_tool(900),
        // Over the line from here on; the summary fails, once, and the
        // run goes on with the whole conversation.
        calls_tool(950),
        says("Done.", 990),
    ]);

    let (agent, heard) = agent(Arc::new(NoSummaries(script)), context, extras());

    let mut conversation = Vec::new();

    let report = agent.run(&mut conversation, "fill things").await.unwrap();

    assert_eq!(report.steps, 5);
    assert_eq!(report.usage.input_tokens, 300 + 500 + 900 + 950 + 990);

    let events = heard.events();

    assert!(compactions(&events).is_empty());

    // Nothing was replaced: the task, then five responses and four
    // answers to their calls.
    assert_eq!(conversation.len(), 10);
    assert_eq!(conversation[0], Message::user_text("fill things"));
    assert!(is_well_formed(&conversation));
    assert_eq!(replay::conversation(&events), conversation);

    // Asked for by hand it fails outright, and changes nothing either.
    let before = conversation.clone();

    assert!(agent.compact(&mut conversation).await.is_err());
    assert_eq!(conversation, before);
}

#[tokio::test]
async fn every_model_call_reports_how_full_the_window_is() {
    let (_dir, context) = context();

    let script = ScriptedProvider::new(vec![calls_tool(300), says("Done.", 450)]);

    let (agent, heard) = agent(Arc::new(script), context, extras());

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fill things").await.unwrap();

    let measured: Vec<_> = heard
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ContextMeasured {
                step,
                context,
                quota,
            } => Some((step, context, quota)),
            _ => None,
        })
        .collect();

    assert_eq!(measured.len(), 2);

    let (step, first, quota) = &measured[0];

    assert_eq!(*step, 1);
    assert_eq!(first.tokens, 300);
    assert_eq!(first.reply_tokens, 20);
    assert_eq!(first.window, Some(1_000));
    assert_eq!(first.share, Some(0.3));
    assert_eq!(*quota, None);

    // What was sent: the system prompt, the tool, and the task.
    assert_eq!(first.chars.system, 4);
    assert_eq!(first.chars.user, 11);
    assert!(first.chars.tool_specs > 0);
    assert_eq!(first.chars.tool_results, 0);
    assert!(first.estimated_tokens.total() <= 300);

    // By the second call the tool's output is in the conversation.
    let (_, second, _) = &measured[1];

    assert_eq!(second.tokens, 450);
    assert_eq!(second.chars.tool_results, 400);
    assert!(second.chars.tool_calls > 0);
    assert!(second.estimated_tokens.tool_results > second.estimated_tokens.user);
}

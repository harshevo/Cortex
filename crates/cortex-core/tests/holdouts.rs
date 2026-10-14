//! Holding advisers out of runs, and the records of tokens and of how
//! runs went, driven through the loop with a scripted model.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use cortex_core::effect::{Arm, EffectLedger, EffectSink, Ending, held_out};
use cortex_core::ledger::{CallKind, Ledger, LedgerSink, PriceTable};
use cortex_core::{
    Advice, Adviser, Briefing, ContextSource, Event, ExperimentConfig, Extras, Workspace, replay,
};
use cortex_providers::ScriptedProvider;
use serde_json::json;

use common::{agent, calls_tool, context, says};

const ADVICE: &str = "Run the tests next.";
const MEMORY: &str = "## Memory\n\n- the tests run with `make check`\n";

/// An adviser that speaks after every step and counts what happens to
/// it.
#[derive(Default)]
struct Talker {
    asked: AtomicU32,
    kept_from_the_model: AtomicU32,
}

impl Adviser for Talker {
    fn advise(&self, _step: u32, _max_steps: u32) -> Option<Advice> {
        self.asked.fetch_add(1, Ordering::Relaxed);

        Some(Advice {
            source: "talker".to_string(),
            text: ADVICE.to_string(),
            detail: json!({ "advised": "inspect" }),
        })
    }

    fn held_out(&self, _step: u32) {
        self.kept_from_the_model.fetch_add(1, Ordering::Relaxed);
    }
}

struct Memory;

#[async_trait]
impl ContextSource for Memory {
    async fn brief(&self, _instruction: &str, _workspace: &Workspace) -> Option<Briefing> {
        Some(Briefing {
            source: "memory".to_string(),
            text: MEMORY.to_string(),
            detail: json!({}),
        })
    }
}

/// The first seed that does, or does not, hold `source` out of a quarter
/// of runs.
fn seed_where(source: &str, held: bool) -> u64 {
    (0..)
        .find(|seed| held_out(*seed, source, 0.25) == held)
        .unwrap()
}

fn experiment(seed: u64) -> Extras {
    Extras {
        experiment: Some(ExperimentConfig {
            seed: Some(seed),
            stand_ins: true,
            ..ExperimentConfig::default()
        }),
        ..Extras::default()
    }
}

fn script() -> Arc<ScriptedProvider> {
    Arc::new(ScriptedProvider::new(vec![
        calls_tool(100),
        calls_tool(200),
        says("Done.", 300),
    ]))
}

fn shown(conversation: &[cortex_core::Message], text: &str) -> usize {
    conversation
        .iter()
        .filter(|message| message.text().contains(text))
        .count()
}

#[tokio::test]
async fn a_held_out_adviser_is_asked_and_recorded_and_not_shown() {
    let (_dir, context) = context();

    let seed = seed_where("talker", true);

    let (agent, heard) = agent(script(), context, experiment(seed));

    let talker = Arc::new(Talker::default());
    let agent = agent.with_adviser(talker.clone());

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fill things").await.unwrap();

    // It was asked after both steps that ran tools, and told both times
    // that the model did not see what it said.
    assert_eq!(talker.asked.load(Ordering::Relaxed), 2);
    assert_eq!(talker.kept_from_the_model.load(Ordering::Relaxed), 2);

    assert_eq!(shown(&conversation, ADVICE), 0);

    let events = heard.events();

    let withheld: Vec<&Event> = events
        .iter()
        .filter(|event| matches!(event, Event::ContextWithheld { .. }))
        .collect();

    assert_eq!(withheld.len(), 2);

    assert!(matches!(
        withheld[0],
        Event::ContextWithheld { step: 1, source, text, .. }
            if source == "talker" && text == ADVICE
    ));

    assert!(!events.iter().any(|event| matches!(
        event,
        Event::ContextPrepared { source, .. } if source == "talker"
    )));

    // The log replays to what the model was shown: no advice in it.
    assert_eq!(replay::conversation(&events), conversation);
}

#[tokio::test]
async fn an_adviser_that_is_not_held_out_speaks_as_ever() {
    let (_dir, context) = context();

    let seed = seed_where("talker", false);

    let (agent, heard) = agent(script(), context, experiment(seed));

    let talker = Arc::new(Talker::default());
    let agent = agent.with_adviser(talker.clone());

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fill things").await.unwrap();

    assert_eq!(talker.kept_from_the_model.load(Ordering::Relaxed), 0);
    assert_eq!(shown(&conversation, ADVICE), 2);

    let events = heard.events();

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ContextWithheld { .. }))
    );

    assert_eq!(replay::conversation(&events), conversation);
}

#[tokio::test]
async fn the_hold_outs_follow_from_the_seed_and_are_said_when_the_run_starts() {
    let seed = seed_where("talker", true);

    let assigned = |events: &[Event]| {
        events.iter().find_map(|event| match event {
            Event::ExperimentAssigned {
                seed,
                share,
                held_out,
                speaking,
                ..
            } => Some((*seed, *share, held_out.clone(), speaking.clone())),

            _ => None,
        })
    };

    let mut seen = Vec::new();

    for _ in 0..2 {
        let (_dir, context) = context();

        let script = Arc::new(ScriptedProvider::new(vec![
            calls_tool(100),
            says("Done.", 200),
            says("Done again.", 300),
        ]));

        let (agent, heard) = agent(script, context, experiment(seed));
        let agent = agent.with_adviser(Arc::new(Talker::default()));

        let mut conversation = Vec::new();

        agent.run(&mut conversation, "fill things").await.unwrap();

        // An adviser has no name until it has spoken, so the first run
        // of an agent cannot list it. The seed still decides it.
        let first = assigned(&heard.events()).unwrap();

        assert_eq!((first.0, first.1), (seed, 0.25));
        assert!(first.2.is_empty() && first.3.is_empty());

        agent.run(&mut conversation, "again").await.unwrap();

        let events = heard.events();
        let second = assigned(&events[events.len() / 2..]).unwrap();

        assert_eq!(second.2, ["talker"]);

        seen.push(conversation);
    }

    // The same seed, the same run.
    assert_eq!(seen[0], seen[1]);
}

#[tokio::test]
async fn a_stand_in_model_is_not_experimented_on_unless_asked() {
    let (_dir, context) = context();

    let quiet = Extras {
        experiment: Some(ExperimentConfig {
            seed: Some(seed_where("talker", true)),
            ..ExperimentConfig::default()
        }),
        ..Extras::default()
    };

    let (agent, heard) = agent(script(), context, quiet);
    let agent = agent.with_adviser(Arc::new(Talker::default()));

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fill things").await.unwrap();

    assert_eq!(shown(&conversation, ADVICE), 2);

    assert!(
        !heard
            .events()
            .iter()
            .any(|event| matches!(event, Event::ExperimentAssigned { .. }))
    );
}

#[tokio::test]
async fn a_briefing_is_held_out_only_when_that_is_asked_for() {
    let briefed = |briefing_share: f64| async move {
        let (_dir, context) = context();

        let extras = Extras {
            experiment: Some(ExperimentConfig {
                share: 0.0,
                briefing_share,
                seed: Some(1),
                stand_ins: true,
            }),
            ..Extras::default()
        };

        let script = Arc::new(ScriptedProvider::new(vec![says("Done.", 100)]));

        let (agent, heard) = agent(script, context, extras);
        let agent = agent.with_context_source(Arc::new(Memory));

        let mut conversation = Vec::new();

        agent.run(&mut conversation, "fill things").await.unwrap();

        (conversation, heard.events())
    };

    // Every briefing held out.
    let (conversation, events) = briefed(1.0).await;

    assert_eq!(conversation[0].text(), "fill things");

    assert!(events.iter().any(|event| matches!(
        event,
        Event::ContextWithheld { step: 0, source, text, .. } if source == "memory" && text == MEMORY
    )));

    assert_eq!(replay::conversation(&events), conversation);

    // None: with nothing to hold out there is no experiment at all.
    let (conversation, events) = briefed(0.0).await;

    assert!(conversation[0].text().starts_with("## Memory"));

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ExperimentAssigned { .. }))
    );
}

#[tokio::test]
async fn a_run_leaves_a_line_in_each_record() {
    let (dir, context) = context();

    let usage = Arc::new(
        LedgerSink::new(Ledger::open(dir.path().join("usage")), "/work/project")
            .recording_stand_ins(),
    );

    let effect = Arc::new(
        EffectSink::new(
            EffectLedger::open(dir.path().join("effect")),
            "/work/project",
        )
        .recording_stand_ins(),
    );

    usage.set_session("session-one");
    effect.set_session("session-one");

    let script = Arc::new(ScriptedProvider::new(vec![
        calls_tool(100),
        calls_tool(200),
        says("Done.", 300),
        says("You are welcome.", 50),
    ]));

    let (agent, _heard) = agent(script, context, experiment(seed_where("talker", true)));

    let agent = agent
        .with_adviser(Arc::new(Talker::default()))
        .with_sink(usage.clone())
        .with_sink(effect.clone());

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fill things").await.unwrap();

    assert_eq!(effect.held_out_now(), Vec::<String>::new());

    agent
        .run(&mut conversation, "thanks, perfect")
        .await
        .unwrap();

    // Tokens: one line per model call, under the session's name.
    let calls = usage.ledger().records(0, u64::MAX);

    assert_eq!(calls.len(), 4);
    assert!(calls.iter().all(|call| call.session == "session-one"));
    assert!(calls.iter().all(|call| call.kind == CallKind::Step));
    assert_eq!(calls[0].title.as_deref(), Some("fill things"));

    let summary = usage
        .ledger()
        .range(0, u64::MAX, 0, &PriceTable::built_in());

    assert_eq!(summary.totals.input_tokens, 650);
    assert_eq!(summary.totals.output_tokens, 80);
    assert_eq!(summary.session_count, 1);
    assert_eq!(summary.favourite_model.as_deref(), Some("scripted"));

    // The stand-in has no price, and none is made up for it.
    assert_eq!(summary.cost.usd, None);
    assert_eq!(summary.cost.unpriced_tokens, 730);

    // How the runs went: the first with the adviser held out, and taken
    // well by the person.
    let runs = effect.ledger().runs(0, u64::MAX);

    assert_eq!(runs.len(), 2);

    let first = &runs[0];

    assert_eq!(first.session, "session-one");
    assert_eq!(first.ending, Ending::Finished);
    assert_eq!(
        (first.steps, first.tool_calls, first.failed_tool_calls),
        (3, 2, 0)
    );
    assert_eq!(first.usage.input_tokens, 600);
    assert_eq!(
        first.reaction,
        Some(cortex_core::effect::Reaction::Accepted)
    );

    let talker = &first.advisers[0];

    assert_eq!(talker.source, "talker");
    assert_eq!(talker.arm(), Arm::HeldOut);

    // It would have advised twice, to look at something; the model's
    // next step did that once and then finished.
    assert_eq!(
        (talker.advised, talker.comparable, talker.followed),
        (2, 2, 1)
    );
}

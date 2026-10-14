use super::*;

const STORIES: [&str; 3] = [
    "<story> the cat sat on the mat . the cat was happy .",
    "<story> the dog ran to the park . the dog saw a big tree .",
    "<story> a bird sang in the morning . the sun was warm .",
];

fn taught(times: usize) -> Brain {
    let mut brain = Brain::new();
    for _ in 0..times {
        for story in STORIES {
            brain.learn(story, "teacher");
        }
    }
    brain
}

#[test]
fn being_taught_makes_it_predict_better_than_guessing_the_commonest() {
    let brain = taught(8);
    let stats = brain.stats();
    assert!(stats.recent_right > stats.recent_baseline + 0.2, "{stats:?}");
    assert!(brain.cortex.totals.synapses_grown > 0);
    assert!(brain.cortex.segments_alive() > 0);
}

#[test]
fn it_says_what_follows_from_what_its_branches_learned() {
    let mut brain = taught(8);
    let said = brain.say("<story> the dog ran");
    assert!(said.text.starts_with("to the park."), "{said:?}");
    assert!(!said.novel, "{said:?}");
    assert!(said.confidence > 0.5);
    let step = brain.snapshot().trace.last().cloned().unwrap();
    assert_eq!(step.mode, "speak");
    assert!(step.why.contains("columns primed"), "{}", step.why);
}

#[test]
fn a_conversation_turn_is_answered_from_what_was_learned() {
    let mut brain = Brain::new();
    for _ in 0..8 {
        brain.learn("<user> hello <brain> hi , i am happy .", "teacher");
        brain.learn("<user> what is your name <brain> my name is cortex .", "teacher");
    }
    let reply = brain.converse("what is your name");
    assert_eq!(reply.text, "my name is cortex.", "{reply:?}");
}

#[test]
fn a_memory_is_recalled_by_refiring_the_cells_it_was_stored_in() {
    let mut brain = taught(10);
    let recall = brain.recall("the dog ran");
    let engram = brain.engram(recall.engram.unwrap()).unwrap();
    assert!(engram.text().contains("dog ran to the park"), "{recall:?}");
    // The first sentence settles first; a second sentence that shares
    // ". the" with other stories needs more repetition, which sleep gives.
    assert!(recall.fidelity > 0.5, "{recall:?}");
    assert!(recall.recalled.contains("ran to the park"), "{recall:?}");
    assert_eq!(recall.reignited, engram.cells[3]);
    assert!(!recall.returned.is_empty());
    assert!(recall.why.contains("Re-fired"));
    assert!(engram.recalls == 1 && engram.strength > 1.0);

    let nothing = brain.recall("quantum chromodynamics");
    assert!(nothing.engram.is_none() || nothing.matched < recall.matched);
}

#[test]
fn sleep_replays_prunes_and_the_brain_survives_a_restart() {
    let mut brain = taught(3);
    let grown = brain.cortex.totals.synapses_grown;
    for _ in 0..12 {
        assert!(brain.sleep().starts_with("asleep"));
    }
    assert!(brain.stats().replays == 12);
    assert!(brain.cortex.totals.synapses_grown >= grown);
    assert!(brain.is_sound());

    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("brain.bin");
    brain.save(&file).unwrap();
    let mut back = Brain::load(&file).unwrap();
    assert_eq!(back.stats(), brain.stats());
    assert_eq!(back.cortex.segments_alive(), brain.cortex.segments_alive());
    assert_eq!(back.say("<story> the dog ran").text, brain.say("<story> the dog ran").text);
}

#[test]
fn a_column_grows_cells_when_its_cells_are_full_and_unused_ones_die() {
    let mut brain = Brain::new();
    // Many different words before the same word fill its column's cells.
    for at in 0..200 {
        brain.learn(&format!("word{at} hub word{at} hub"), "teacher");
    }
    let hub = columns_of(&brain, "hub")[0];
    assert!(brain.cortex.column_cells(hub).len() > cortex::START_CELLS);
    assert!(brain.cortex.totals.cells_grown > 0);
    brain.cortex.clock += 100_000;
    let (_, _, _) = brain.cortex.prune(1, 0.0);
    assert!(brain.cortex.totals.synapses_pruned > 0);
    assert!(brain.cortex.totals.cells_died > 0);
    assert!(brain.is_sound());
}

#[test]
fn it_is_curious_about_words_that_still_surprise_it() {
    let mut brain = taught(2);
    brain.learn("the zebra jumped over the zebra fence", "teacher");
    let curious = brain.curiosity(10);
    assert!(!curious.is_empty());
    assert!(curious.iter().any(|c| c.word == "zebra"), "{curious:?}");
}

#[test]
fn snapshot_carries_engrams_with_their_columns_and_the_trace() {
    let brain = taught(2);
    let snapshot = brain.snapshot();
    assert_eq!(snapshot.columns.len(), words::COLUMNS);
    // A lesson repeated is one memory, strengthened.
    assert_eq!(snapshot.engram_count, 3);
    assert!(snapshot.engrams.iter().all(|e| e.strength > 1.0));
    assert!(snapshot.engrams.iter().all(|e| !e.columns.is_empty() && e.cells > 0));
    assert!(!snapshot.trace.is_empty());
    assert!(snapshot.trace.iter().any(|s| s.mode == "learn" && s.bursting > 0));
}

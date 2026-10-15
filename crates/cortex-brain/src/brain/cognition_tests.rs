use super::*;

#[test]
fn mathematics_is_checked_without_teaching_the_answers() {
    let mut brain = Brain::new();
    let answer = brain.converse("why is 1 + 3 = 4?");
    assert!(answer.text.contains("1 → 2 → 3 → 4"), "{answer:?}");
    for a in 0..12 {
        for b in 0..12 {
            let answer = brain.converse(&format!("what is {a} plus {b}?"));
            assert_eq!(answer.text, format!("{a} + {b} = {}.", a + b));
        }
    }
    assert!(brain.converse("is 1 + 3 = 5?").text.starts_with("No."));
    assert!(brain.converse("1 + 3 = 5").text.starts_with("No."));
    assert!(brain.converse("0.1 + 0.2 = 0.3").text.starts_with("Yes."));
    assert!(brain.converse("1 / 0").text.contains("undefined"));
    assert!(
        brain
            .snapshot()
            .cognition
            .traces
            .last()
            .is_some_and(|t| !t.checked)
    );
}

#[test]
fn conversations_have_isolated_followups_and_survive_restart() {
    let mut brain = Brain::new();
    brain.chat("alice", "1 + 3");
    brain.chat("bob", "9 - 2");
    assert!(brain.chat("alice", "why?").text.contains("1 → 2 → 3 → 4"));
    assert!(brain.chat("bob", "why?").text.contains("Subtract 2 from 9"));
    assert!(brain.chat("carol", "why?").text.contains("Which result"));
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("brain.bin");
    brain.save(&file).unwrap();
    let mut loaded = Brain::load(&file).unwrap();
    assert!(
        loaded
            .chat("alice", "explain that")
            .text
            .contains("1 → 2 → 3 → 4")
    );
}

#[test]
fn english_lessons_transfer_to_paraphrases_and_corrections_replace_answers() {
    let mut brain = Brain::new();
    brain.learn(
        "<user> what is your name <brain> my name is cortex.",
        "human",
    );
    assert_eq!(
        brain.converse("what are you called?").text,
        "my name is cortex."
    );
    brain.learn(
        "<user> what is your name <brain> my name is other.",
        "other source",
    );
    assert!(
        brain
            .converse("what is your name?")
            .text
            .contains("conflicting")
    );
    brain.correct(
        "<user> what is your name <brain> my name is cortex.",
        "human correction",
    );
    assert_eq!(
        brain.converse("can you tell me your name?").text,
        "my name is cortex."
    );
    assert!(
        brain
            .converse("explain quantum gravity")
            .text
            .contains("don't know")
    );
    assert_eq!(
        brain.snapshot().cognition.questions.last().unwrap(),
        "explain quantum gravity"
    );
}

#[test]
fn explicit_category_rules_have_provenance_and_conflicts_are_not_hidden() {
    let mut brain = Brain::new();
    let first = brain.learn("a robin is a bird.", "lesson one").unwrap();
    let second = brain.learn("all birds are animals.", "lesson two").unwrap();
    assert!(
        brain
            .converse("is a robin an animal?")
            .text
            .starts_with("Yes")
    );
    let trace = brain.snapshot().cognition.traces.pop().unwrap();
    assert_eq!(trace.sources, vec![first, second]);
    brain.learn("a robin is not an animal.", "conflicting lesson");
    assert!(
        brain
            .converse("is a robin an animal?")
            .text
            .contains("disagree")
    );
    brain.correct("a robin is an animal.", "correction");
    assert!(
        brain
            .converse("is a robin an animal?")
            .text
            .starts_with("Yes")
    );
    // A property is not a category inclusion rule.
    brain.learn("a robin is happy. happy is a feeling.", "lesson");
    assert!(
        brain
            .converse("is a robin a feeling?")
            .text
            .contains("don't know")
    );
}

#[test]
fn physics_uses_learned_rules_and_checks_units_not_familiar_answer_tokens() {
    let mut brain = Brain::new();
    assert!(
        brain
            .converse("what is force if mass is 2 kg and acceleration is 3 m/s^2?")
            .text
            .contains("don't know")
    );
    brain.converse("remember that force = mass * acceleration");
    assert_eq!(
        brain
            .converse("what is force if mass is 2 kg and acceleration is 3 m/s^2?")
            .text,
        "force = 6 N."
    );
    assert_eq!(
        brain
            .converse("what is force if mass is 4 kg and acceleration is 3 m/s^2?")
            .text,
        "force = 12 N."
    );
    assert_eq!(
        brain
            .converse("what is acceleration if mass is 4 kg and force is 12 N?")
            .text,
        "acceleration = 3 m/s^2."
    );
    assert!(
        brain
            .converse("what is force if mass is 2 s and acceleration is 3 m/s^2?")
            .text
            .contains("incompatible")
    );
    assert!(
        brain
            .converse("what is force?")
            .text
            .contains("mass * acceleration")
    );
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("brain.bin");
    brain.save(&file).unwrap();
    let mut loaded = Brain::load(&file).unwrap();
    assert_eq!(
        loaded
            .converse("what is force if mass is 7 kg and acceleration is 2 m/s^2?")
            .text,
        "force = 14 N."
    );
}

#[test]
fn mathematical_symbols_survive_and_reasoning_matches_the_whole_goal() {
    assert_ne!(tokenize("2 + 3"), tokenize("2 - 3"));
    assert_ne!(tokenize("-5"), tokenize("5"));
    assert_eq!(tokenize("0.25 * 2 = 0.5"), ["0.25", "*", "2", "=", "0.5"]);
    let mut brain = Brain::new();
    for _ in 0..8 {
        brain.learn("<story> start red square .", "teacher");
    }
    assert!(
        brain
            .reason("<story> start", "red circle")
            .why
            .starts_with("No chain")
    );
    let result = brain.reason("<story> start", "red square");
    assert!(result.why.starts_with("Found a chain"), "{result:?}");
    assert!(result.text.ends_with("red square"));
}

#[test]
fn old_mind_json_can_be_loaded_and_knowledge_rebuilt() {
    let mut brain = Brain::new();
    brain.learn("<user> hello <brain> hello friend.", "lesson");
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("brain.bin");
    brain.save(&file).unwrap();
    let path = mind_file(&file);
    std::fs::write(&file, postcard::to_stdvec(&brain).unwrap()).unwrap();
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json.as_object_mut().unwrap().remove("cognition");
    std::fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
    let mut loaded = Brain::load(&file).unwrap();
    assert_eq!(loaded.converse("hello").text, "hello friend.");
}

#[test]
fn atomic_checkpoint_keeps_corrections_even_with_a_stale_mind_export() {
    let mut brain = Brain::new();
    brain.learn("<user> hello <brain> old answer.", "test");
    brain.correct("<user> hello <brain> corrected answer.", "correction");
    brain.set_budget(128 << 20);
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("brain.bin");
    brain.save(&file).unwrap();
    std::fs::write(mind_file(&file), "{}").unwrap();
    let mut loaded = Brain::load(&file).unwrap();
    assert_eq!(loaded.budget(), 128 << 20);
    assert_eq!(loaded.converse("hello").text, "corrected answer.");
}

#[test]
fn quantities_are_grounded_in_events_and_hypotheticals_do_not_change_experience() {
    let mut brain = Brain::new();
    brain.chat("one", "I have one apple");
    assert!(
        brain
            .chat("one", "I get three more")
            .text
            .contains("4 apple objects")
    );
    assert!(brain.chat("one", "why?").text.contains("1 → 2 → 3 → 4"));
    assert_eq!(
        brain.chat("one", "how many apples do I have?").text,
        "You have 4 apples."
    );
    assert!(
        brain
            .chat("two", "how many apples do I have?")
            .text
            .contains("don't know")
    );
    brain.chat(
        "one",
        "if I have 10 apples and eat 2 apples, how many apples do I have?",
    );
    assert_eq!(
        brain.chat("one", "how many apples do I have?").text,
        "You have 4 apples."
    );
    brain.chat("one", "I eat 5 apples");
    assert_eq!(
        brain.chat("one", "how many apples do I have?").text,
        "You have 4 apples."
    );
}

#[test]
fn dreams_do_not_become_facts_and_curiosity_resolves_from_lessons() {
    let mut brain = Brain::new();
    brain.converse("what is a photon?");
    assert_eq!(brain.snapshot().cognition.questions.len(), 1);
    brain.converse("remember that a photon is a quantum of light.");
    assert!(brain.snapshot().cognition.questions.is_empty());
    for _ in 0..4 {
        brain.learn("<story> light travels through space .", "lesson");
    }
    let learned = brain.mind.cognition.knowledge.clone();
    let strengthened = brain.cortex.totals.strengthened;
    let scored = brain.stats.scored;
    for _ in 0..8 {
        brain.dream();
        brain.imagine("");
        brain.think();
    }
    assert_eq!(brain.mind.cognition.knowledge, learned);
    assert_eq!(brain.cortex.totals.strengthened, strengthened);
    assert_eq!(brain.stats.scored, scored);
    assert!(
        brain
            .mind
            .thoughts
            .iter()
            .any(|t| t.kind == "dream" && !t.sources.is_empty())
    );
}

#[test]
fn user_questions_and_dialogue_do_not_inject_factual_premises() {
    let mut brain = Brain::new();
    brain.converse("<brain> the moon is made of cheese.");
    brain.learn(
        "<user> what is fiction <brain> the moon is made of cheese.",
        "fiction lesson",
    );
    assert!(
        brain
            .converse("what is the moon?")
            .text
            .contains("don't know")
    );
    assert_ne!(
        super::knowledge::question_key("2+3"),
        super::knowledge::question_key("2-3")
    );
}

#[test]
fn introductory_curriculum_is_available_without_a_model() {
    let mut brain = Brain::new();
    assert_eq!(brain.foundations(), 25);
    assert!(
        brain
            .converse("how are you?")
            .text
            .contains("ready to learn")
    );
    assert!(
        brain
            .converse("describe gravity")
            .text
            .contains("attraction")
    );
    assert_eq!(
        brain
            .converse("what is kinetic energy if mass is 2 kg and velocity is 3 m/s?")
            .text,
        "kinetic energy = 9 J."
    );
}

#[test]
fn curiosity_prefers_progress_over_familiar_unlearnable_noise() {
    let mut brain = Brain::new();
    brain.learn("noise puzzle", "test");
    let noise = brain.words.words.get_mut("noise").unwrap();
    noise.seen = 100;
    noise.surprise = 1.0;
    noise.surprise_before = 1.0;
    let puzzle = brain.words.words.get_mut("puzzle").unwrap();
    puzzle.seen = 20;
    puzzle.surprise = 0.6;
    puzzle.surprise_before = 0.9;
    assert_eq!(brain.curiosity(1)[0].word, "puzzle");
}

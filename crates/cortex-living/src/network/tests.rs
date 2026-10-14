use super::*;

fn moment(network: &mut Network, percepts: &[&str]) {
    let percepts: Vec<String> = percepts.iter().map(|text| text.to_string()).collect();

    network.live(&percepts);
}

/// An edit, then the tests, then their passing: the same every time.
fn routine(network: &mut Network, times: usize) {
    for _ in 0..times {
        moment(network, &["does: edit_file", "file: fees.rs"]);
        moment(network, &["does: shell", "runs: cargo test"]);
        moment(network, &["result: worked"]);
    }
}

#[test]
fn a_neuron_is_grown_for_what_was_never_met_and_only_once() {
    let mut network = Network::new();

    assert!(!network.knows("file: fees.rs"));

    moment(&mut network, &["does: edit_file", "file: fees.rs"]);

    let grown = network.meters().grown;

    assert!(network.knows("file: fees.rs"));
    assert_eq!(grown, 2);

    moment(&mut network, &["does: edit_file", "file: fees.rs"]);

    // The second meeting grows the assembly for the pair, and no
    // second neuron for either part.
    assert_eq!(network.meters().grown, grown + 1);
    assert!(network.knows("does: edit_file + file: fees.rs"));

    moment(&mut network, &["does: edit_file", "file: fees.rs"]);

    assert_eq!(network.meters().grown, grown + 1);
}

#[test]
fn what_follows_grows_a_branch_and_is_then_expected() {
    let mut network = Network::new();

    routine(&mut network, 1);

    assert!(network.weight("runs: cargo test", "result: worked") > 0.0);
    assert!(network.meters().branched > 0);

    routine(&mut network, 5);
    moment(&mut network, &["does: edit_file", "file: fees.rs"]);

    assert!(
        network
            .expecting()
            .contains(&"runs: cargo test".to_string()),
        "{:?}",
        network.expecting()
    );
}

#[test]
fn a_branch_answers_to_a_combination_and_not_to_its_parts() {
    let mut network = Network::new();

    // The tests pass after an edit to one file and fail after an edit
    // to the other. The test run itself is the same in both.
    for _ in 0..12 {
        moment(&mut network, &["does: edit_file", "file: fees.rs"]);
        moment(&mut network, &["does: shell", "runs: cargo test"]);
        moment(&mut network, &["result: worked"]);

        moment(&mut network, &["does: edit_file", "file: tax.rs"]);
        moment(&mut network, &["does: shell", "runs: cargo test"]);
        moment(&mut network, &["result: failed"]);
    }

    moment(&mut network, &["does: edit_file", "file: fees.rs"]);
    moment(&mut network, &["does: shell", "runs: cargo test"]);

    let after_fees = network.expecting();

    moment(&mut network, &["result: worked"]);
    moment(&mut network, &["does: edit_file", "file: tax.rs"]);
    moment(&mut network, &["does: shell", "runs: cargo test"]);

    let after_tax = network.expecting();

    // The surest expectation is the right one each time.
    assert_eq!(after_fees[0], "result: worked", "after fees.rs");
    assert_eq!(after_tax[0], "result: failed", "after tax.rs");
}

#[test]
fn a_routine_stops_being_surprising_and_beats_guessing_the_common() {
    let mut network = Network::new();

    routine(&mut network, 2);

    let early = network.meters().surprise;

    routine(&mut network, 40);

    let meters = network.meters();

    assert!(
        meters.surprise < early,
        "{} then {}",
        early,
        meters.surprise
    );
    assert!(meters.surprise < 0.3, "{}", meters.surprise);
    assert!(
        meters.expected_right > meters.baseline_right,
        "{} against {}",
        meters.expected_right,
        meters.baseline_right
    );

    // It has also learned to expect not to be surprised.
    assert!(meters.self_accuracy > 0.7, "{}", meters.self_accuracy);
}

#[test]
fn a_branch_that_stops_being_right_is_weakened_and_removed() {
    let mut network = Network::new();

    routine(&mut network, 6);

    let learned = network.weight("runs: cargo test", "result: worked");

    for _ in 0..80 {
        moment(&mut network, &["does: edit_file", "file: fees.rs"]);
        moment(&mut network, &["does: shell", "runs: cargo test"]);
        moment(&mut network, &["result: failed"]);
    }

    assert!(
        network.weight("runs: cargo test", "result: worked") < learned * 0.2,
        "{learned} {:#?}",
        network.explain("result: worked")
    );
    assert!(network.weight("runs: cargo test", "result: failed") > 0.5);
    assert!(network.meters().disconnected > 0);
}

#[test]
fn a_proven_branch_is_stiffer_than_a_new_one() {
    let new = Segment::default();

    let proven = Segment {
        hits: 30,
        misses: 1,
        ..Segment::default()
    };

    let failing = Segment {
        hits: 30,
        misses: 40,
        ..Segment::default()
    };

    assert!(proven.plasticity() < new.plasticity());
    assert!(failing.plasticity() > proven.plasticity());
}

#[test]
fn the_new_and_unexpected_hold_the_workspace() {
    let mut network = Network::new();

    routine(&mut network, 10);
    moment(&mut network, &["person: rejects"]);

    let snapshot = network.snapshot();

    assert_eq!(snapshot.workspace[0], "person: rejects");
    assert!(snapshot.workspace.len() <= WORKSPACE);

    // And it stays for the next moment, to be wired to what follows.
    moment(&mut network, &["does: edit_file", "file: fees.rs"]);

    assert!(
        network
            .snapshot()
            .workspace
            .contains(&"person: rejects".to_string())
    );
    assert!(network.weight("person: rejects", "file: fees.rs") > 0.0);
}

#[test]
fn its_own_states_are_wired_to_the_world() {
    let mut network = Network::new();

    routine(&mut network, 4);

    // Something new follows the test run, every time.
    for round in 0..6 {
        moment(&mut network, &["does: shell", "runs: cargo test"]);
        moment(&mut network, &[&format!("result: error {round}")]);
    }

    assert!(network.weight("runs: cargo test", SURPRISED) > 0.3);
}

#[test]
fn news_reaches_what_came_before_it() {
    let mut network = Network::new();

    for _ in 0..10 {
        moment(&mut network, &["does: edit_file", "file: fees.rs"]);
        moment(&mut network, &["checks: passed"]);

        moment(&mut network, &["does: edit_file", "file: tax.rs"]);
        moment(&mut network, &["checks: failed"]);
    }

    assert!(
        network.value("file: fees.rs") > 0.2,
        "{}",
        network.value("file: fees.rs")
    );
    assert!(
        network.value("file: tax.rs") < -0.2,
        "{}",
        network.value("file: tax.rs")
    );
}

#[test]
fn it_imagines_the_way_on_a_step_at_a_time() {
    let mut network = Network::new();

    routine(&mut network, 12);
    moment(&mut network, &["does: edit_file", "file: fees.rs"]);

    let imagined = network.imagine(3);

    assert!(imagined.len() >= 2, "{imagined:?}");
    assert!(imagined[0].labels.contains(&"runs: cargo test".to_string()));
    assert!(imagined[1].labels.contains(&"result: worked".to_string()));

    // Each further step is less sure, and the good news is counted.
    assert!(imagined[1].confidence < imagined[0].confidence);
    assert!(imagined[1].worth > 0.0);

    // Imagining changes nothing.
    assert_eq!(network.imagine(3), imagined);
}

#[test]
fn it_finds_the_way_back_from_a_goal() {
    let mut network = Network::new();

    for _ in 0..8 {
        moment(&mut network, &["does: edit_file", "file: fees.rs"]);
        moment(&mut network, &["does: shell", "runs: cargo test"]);
        moment(&mut network, &["checks: passed"]);
    }

    let plan = network.plan("checks: passed", 4);

    assert!(plan.len() >= 2, "{plan:?}");

    let last = plan.last().unwrap();

    assert!(last.contains(&"runs: cargo test".to_string()), "{plan:?}");
    assert!(
        plan[plan.len() - 2]
            .iter()
            .any(|label| label.contains("fees.rs") || label == "does: edit_file"),
        "{plan:?}"
    );

    assert_eq!(network.snapshot().goal.as_deref(), Some("checks: passed"));
}

#[test]
fn it_says_why_it_expects_something() {
    let mut network = Network::new();

    routine(&mut network, 8);
    moment(&mut network, &["does: edit_file", "file: fees.rs"]);
    moment(&mut network, &["does: shell", "runs: cargo test"]);

    let snapshot = network.snapshot();

    let reason = snapshot
        .reasons
        .iter()
        .find(|reason| reason.expected == "result: worked")
        .expect("it expects the tests to pass");

    assert!(reason.because.contains(&"runs: cargo test".to_string()));
    assert!(reason.hits >= 5);
    assert_eq!(reason.misses, 0);
}

#[test]
fn it_advises_only_once_its_record_is_long_enough() {
    let mut network = Network::new();

    routine(&mut network, 2);
    moment(&mut network, &["does: edit_file", "file: fees.rs"]);

    assert_eq!(network.advise(), None);

    moment(&mut network, &["does: shell", "runs: cargo test"]);
    moment(&mut network, &["result: worked"]);

    routine(&mut network, 20);
    moment(&mut network, &["does: edit_file", "file: fees.rs"]);

    let hunch = network.advise().expect("it is sure by now");

    assert_eq!(hunch.kind, HunchKind::Next);
    assert_eq!(hunch.label, "does: shell");
    assert_eq!(hunch.with, ["runs: cargo test"]);
    assert!(hunch.hits >= MIN_RECORD);
    assert!(hunch.worth > 0.0);

    // Not said twice in a row.
    assert_eq!(network.advise(), None);
}

#[test]
fn wrong_advice_makes_it_more_cautious() {
    let mut network = Network::new();

    routine(&mut network, 20);
    moment(&mut network, &["does: edit_file", "file: fees.rs"]);

    let caution = network.meters().caution;

    assert!(network.advise().is_some());

    // Something else came.
    moment(&mut network, &["does: read_file", "file: other.rs"]);

    assert!(network.meters().caution > caution);
    assert_eq!(network.meters().advised, 1);
    assert!(network.knows(ADVICE_WRONG));
}

#[test]
fn it_warns_when_the_usual_way_on_goes_badly() {
    let mut network = Network::new();

    for _ in 0..20 {
        moment(&mut network, &["did: edit_file", "result: worked"]);
        moment(&mut network, &["does: shell", "runs: make deploy"]);
        moment(&mut network, &["did: shell", "result: failed"]);
        moment(&mut network, &["checks: failed"]);
    }

    moment(&mut network, &["did: edit_file", "result: worked"]);

    let hunch = network.advise().expect("it has seen this go badly");

    assert_eq!(hunch.kind, HunchKind::Warning, "{hunch:?}");
    assert!(hunch.worth < 0.0);
}

#[test]
fn it_learns_faster_when_things_change_and_slower_when_they_hold() {
    let mut network = Network::new();

    routine(&mut network, 80);

    let settled = network.meters().plasticity;

    assert!(settled < 1.0, "{settled}");

    // Nothing is as it was.
    for round in 0..60 {
        moment(&mut network, &[&format!("does: tool {}", round % 7)]);
        moment(&mut network, &[&format!("result: {}", round % 5)]);
    }

    assert!(network.meters().plasticity > settled);
}

#[test]
fn rest_replays_a_limited_number_of_times_and_grows_nothing() {
    let mut network = Network::new();

    routine(&mut network, 3);

    let before = network.weight("runs: cargo test", "result: worked");
    let neurons = network.snapshot().neurons.len();
    let branches = network.meters().branched;
    let moments = network.moment();
    let expecting = network.expecting();

    let mut rests = 0;

    while network.rest() {
        rests += 1;

        assert!(rests < 200, "rest never ends");
    }

    assert!(rests > 0);
    assert!(network.weight("runs: cargo test", "result: worked") > before);
    assert_eq!(network.snapshot().neurons.len(), neurons);
    assert_eq!(network.meters().branched, branches);
    assert_eq!(network.moment(), moments);
    assert!(network.meters().replays > 0);

    // What it expected of the next lived moment is as it was.
    assert_eq!(network.expecting(), expecting);
}

#[test]
fn an_assembly_that_never_fires_again_is_removed() {
    let mut network = Network::new();

    moment(&mut network, &["does: read_file", "file: once.rs"]);
    moment(&mut network, &["does: read_file", "file: once.rs"]);

    assert!(network.knows("does: read_file + file: once.rs"));

    // It fired when it was grown and never again.
    for round in 0..(UNPROVEN as usize + 2 * TIDY_EVERY as usize) {
        moment(&mut network, &[&format!("tick: {}", round % 3)]);
    }

    assert!(!network.knows("does: read_file + file: once.rs"));
    assert!(network.meters().removed > 0);
}

#[test]
fn a_saved_network_carries_on_where_it_was() {
    let mut network = Network::new();

    routine(&mut network, 6);

    let text = serde_json::to_string(&network).unwrap();

    let mut loaded: Network = serde_json::from_str(&text).unwrap();

    loaded.wake();

    assert_eq!(loaded.moment(), network.moment());
    assert_eq!(
        loaded.weight("runs: cargo test", "result: worked"),
        network.weight("runs: cargo test", "result: worked")
    );

    // No second neuron for what it already knows.
    let grown = loaded.meters().grown;

    routine(&mut loaded, 1);

    assert_eq!(loaded.meters().grown, grown);
}

#[test]
fn a_network_saved_before_dendrites_keeps_what_it_learned() {
    let old = r#"{
        "moment": 12,
        "next_id": 2,
        "neurons": [
            {"id": 1, "kind": "sense", "label": "runs: cargo test", "born": 1,
             "reason": "", "fired": 4, "last_fired": 11, "expects_surprise": 0.5,
             "out": [{"to": 2, "weight": 0.8}]},
            {"id": 2, "kind": "sense", "label": "result: worked", "born": 2,
             "reason": "", "fired": 4, "last_fired": 12, "expects_surprise": 0.5,
             "out": []}
        ]
    }"#;

    let mut network: Network = serde_json::from_str(old).unwrap();

    network.wake();

    assert_eq!(network.weight("runs: cargo test", "result: worked"), 0.8);

    // And it is saved the new way.
    let saved = serde_json::to_string(&network).unwrap();

    assert!(!saved.contains("\"out\""));
}

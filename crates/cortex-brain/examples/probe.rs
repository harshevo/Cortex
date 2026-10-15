//! Teaches a small set of simple stories, then lets it speak from
//! prompts, and reports what was new. `cargo run --release -p cortex-brain --example probe`
use cortex_brain::Brain;

fn main() {
    let stories = [
        "<story> the cat sat on the mat . the cat was happy .",
        "<story> the dog ran to the park . the dog was happy .",
        "<story> the cat ran to the tree . the cat was tired .",
        "<story> the dog sat on the rug . the dog was sleepy .",
        "<story> a bird sang in the tree . the bird was happy .",
        "<story> the boy ran to the park . the boy saw the dog .",
        "<story> the girl sat on the rug . the girl saw the cat .",
        "<story> the bird sat on the mat . the bird was sleepy .",
    ];
    let mut brain = Brain::new();
    let started = std::time::Instant::now();
    for round in 0..10 {
        for story in stories {
            brain.learn(story, "teacher");
        }
        let s = brain.stats();
        println!(
            "round {round}: next word right {:.2} vs commonest {:.2}",
            s.recent_right, s.recent_baseline
        );
    }
    println!("learned in {:?}", started.elapsed());
    let id = brain
        .learn(
            "<story> the girl sat on the rug . the girl saw the cat .",
            "teacher",
        )
        .unwrap();
    for step in brain.snapshot().trace.iter().rev().take(14).rev() {
        println!(
            "{:>6} predicted {:>2} burst {:>2} branches {}",
            step.token,
            step.predicted,
            step.bursting,
            step.branches.len()
        );
    }
    let e = brain.engram(id).unwrap().clone();
    for (t, c) in e.tokens.iter().zip(&e.cells) {
        println!("{t}: {} cells {:?}", c.len(), &c[..3.min(c.len())]);
    }
    brain.cortex.reset();
    brain.cortex.reactivate(&e.cells[10]);
    println!(
        "after re-firing saw cells: {} primed",
        brain.cortex.primed_cells().len()
    );
    let r = brain.recall("the girl saw");
    println!(
        "recall before sleep: {} | fidelity {:.2} | {}",
        r.recalled, r.fidelity, r.why
    );
    for at in 0..30 {
        let did = brain.sleep();
        if at % 6 == 5 {
            println!("{did}");
        }
        let r = brain.recall("the girl saw");
        if r.fidelity < 0.5 {
            println!("sleep {at}: recall broke: {}", r.recalled);
            break;
        }
    }
    for prompt in [
        "<story> the boy",
        "<story> the girl",
        "<story> the bird ran",
        "<story> the boy sat",
        "<story> a cat",
        "<story> the girl ran",
    ] {
        for _ in 0..3 {
            let u = brain.say(prompt);
            println!(
                "{prompt} → {}  novel={} {:?}",
                u.text, u.novel, u.new_sentences
            );
        }
    }
    let r = brain.recall("the girl saw");
    println!(
        "recall: {} | fidelity {:.2} | {}",
        r.recalled, r.fidelity, r.why
    );
    let snap = brain.snapshot();
    println!(
        "cells {} branches {} synapses {} connected {} vocab {}",
        snap.cells, snap.branches, snap.synapses, snap.connected, snap.vocabulary
    );
}

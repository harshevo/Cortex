//! How the brain's step time holds up as it grows.
//! `cargo run --release -p cortex-neural --example scale`
use cortex_neural::Brain;
fn main() {
    let subjects = ["the cat", "the dog", "a bird", "the boy", "the girl", "my mother", "a farmer", "the teacher", "a fish", "the king"];
    let verbs = ["sat on", "ran to", "looked at", "walked past", "jumped over", "sang near", "slept under", "found"];
    let things = ["the mat", "the park", "a tree", "the river", "the house", "a red ball", "the hill", "a small box", "the moon", "the garden"];
    let mut brain = Brain::new();
    let mut n = 0u64;
    let started = std::time::Instant::now();
    for round in 0..6 {
        let t = std::time::Instant::now();
        let mut words = 0;
        for (i, s) in subjects.iter().enumerate() {
            for (j, v) in verbs.iter().enumerate() {
                let o = things[(i * 3 + j + round) % things.len()];
                let extra = format!("word{}x{}", n % 997, round);
                let text = format!("<story> {s} {v} {o} . it was {extra} .");
                words += text.split_whitespace().count();
                brain.learn(&text, "teacher");
                n += 1;
            }
        }
        let snap = brain.snapshot();
        println!("round {round}: {:.1} µs/word, cells {}, branches {}, synapses {}, columns {}, words {}, footprint {} MB, next word right {:.2} vs {:.2}",
            t.elapsed().as_micros() as f64 / words as f64, snap.cells, snap.branches, snap.synapses, snap.area, snap.vocabulary, snap.footprint >> 20, snap.stats.recent_right, snap.stats.recent_baseline);
    }
    println!("total {:?}", started.elapsed());
    for p in ["<story> the king", "<story> a fish", "<story> my mother"] { println!("{p} → {}", brain.say(p).text); }
}

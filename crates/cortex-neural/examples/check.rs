//! Loads a saved brain and says what it holds.
//! `cargo run --release -p cortex-neural --example check -- <brain.bin>`
fn main() {
    let path = std::env::args().nth(1).expect("give a brain file");
    let mut brain = cortex_neural::Brain::load(std::path::Path::new(&path)).expect("load");
    let s = brain.snapshot();
    println!("loaded: {} memories, {} words, {} cells, {} branches, {} synapses, {} lessons", s.engram_count, s.vocabulary, s.cells, s.branches, s.synapses, s.stats.lessons);
    println!("says: {}", brain.say("<story>").text);
}

//! Loads a saved brain and lets its mind run: thoughts, a hunch, an
//! imagining, a dream, a chain of reasoning and a look at itself.
//! `cargo run --release -p cortex-neural --example mind -- <brain.bin> [from] [to]`
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("give a brain file");
    let from = args.next().unwrap_or_else(|| "a model".into());
    let to = args.next().unwrap_or_else(|| "learns".into());
    let mut brain = cortex_neural::Brain::load(std::path::Path::new(&path)).expect("load");
    let started = std::time::Instant::now();
    for thought in [
        brain.think(),
        brain.think(),
        brain.intuit(""),
        brain.imagine(""),
        brain.imagine("the brain"),
        brain.dream(),
        brain.reason(&from, &to),
        brain.reflect(),
    ] {
        println!("[{}] {}\n    {}\n", thought.kind, thought.text, thought.why);
    }
    println!("took {:?}", started.elapsed());
    let started = std::time::Instant::now();
    let wiring = brain.wiring();
    let json = serde_json::to_string(&wiring).unwrap();
    println!("wiring: {} synapses, {} cells, {} KB as JSON, built in {:?}", wiring.synapse_count, wiring.cells.len(), json.len() / 1024, started.elapsed());
    let snapshot = serde_json::to_string(&brain.snapshot()).unwrap();
    println!("snapshot: {} KB", snapshot.len() / 1024);
}

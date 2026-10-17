//! `cortex learned`: what the project's runs have taught.

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use cortex_core::learn;
use cortex_core::notes::Notes;

use crate::args::LearnedArgs;

pub fn execute(args: LearnedArgs) -> Result<ExitCode> {
    let root = args.dir.canonicalize()?;
    let learned = learn::for_project(&root);

    if let Some(name) = &args.accept {
        let candidate = learned
            .candidates
            .iter()
            .find(|candidate| &candidate.name == name)
            .ok_or_else(|| anyhow!("no candidate skill is called {name}"))?;

        let path = learn::accept(&root, candidate)?;

        println!("Saved {}. `/{name}` starts it.", path.display());

        return Ok(ExitCode::SUCCESS);
    }

    if let Some(name) = &args.dismiss {
        learn::dismiss(&root, name)?;

        println!("{name} will not be offered again.");

        return Ok(ExitCode::SUCCESS);
    }

    if learned.runs == 0 {
        println!("Nothing learned yet. Every run that changes the project is kept to learn from.");

        return Ok(ExitCode::SUCCESS);
    }

    println!("{} runs\n", learned.runs);

    for kind in &learned.kinds {
        let tools = match kind.helpful_tools.is_empty() {
            true => String::new(),
            false => format!(" · passing runs used {}", kind.helpful_tools.join(", ")),
        };

        println!(
            "{:<16} {:>3} runs  {:>3.0}% passed  ~{} steps{tools}",
            kind.kind.label(),
            kind.runs,
            kind.pass_rate() * 100.0,
            kind.median_steps
        );
    }

    let notes = Notes::for_project(&root).all();
    let doubtful: Vec<String> = learned
        .notes
        .iter()
        .filter(|value| value.doubtful())
        .filter_map(|value| notes.iter().find(|note| note.id == value.id))
        .map(|note| format!("  #{} {}", note.id, note.text))
        .collect();

    if !doubtful.is_empty() {
        println!(
            "\nNotes never shown in a run that passed:\n{}",
            doubtful.join("\n")
        );
    }

    if !learned.candidates.is_empty() {
        println!("\nCandidate skills:");

        for candidate in &learned.candidates {
            println!(
                "  {} — {} from {} passing runs (cortex learned --accept {})",
                candidate.name,
                candidate.steps.join(" → "),
                candidate.runs,
                candidate.name
            );
        }
    }

    Ok(ExitCode::SUCCESS)
}

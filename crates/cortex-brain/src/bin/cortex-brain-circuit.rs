use cortex_brain::circuit::{
    Config,
    language::{Assistant, read_lessons},
};
use std::{
    io::{self, BufRead, Write},
    path::Path,
};
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        return Err("Use init CHECKPOINT [small|30m], train DATA_DIR CHECKPOINT [EPOCHS], calibrate DEV_FILE CHECKPOINT, evaluate DATA_DIR CHECKPOINT, ask CHECKPOINT QUESTION, generate CHECKPOINT QUESTION, generate-neural CHECKPOINT QUESTION, chat CHECKPOINT, serve CHECKPOINT [SOCKET], consolidate CHECKPOINT, assess CHECKPOINT, monitor CHECKPOINT, monitor-data CHECKPOINT CONTEXTS.json NEW_OUTPUT.json, monitor-learning-data CHECKPOINT CONTEXTS.json NEW_OUTPUT.json, sequence-probe CHECKPOINT FEATURES.json QUESTION, activity-probe CHECKPOINT QUESTION, cache-instructions CHECKPOINT PAIRS.json NEW_DIRECTORY, or inspect CHECKPOINT OUTPUT.json".into());
    };
    let required = |i: usize| {
        args.get(i)
            .map(String::as_str)
            .ok_or("Missing argument.".to_owned())
    };
    match command {
        "activity-probe" => {
            let brain = Assistant::load(Path::new(required(1)?))?;
            println!(
                "{}",
                serde_json::to_string(&brain.activity_probe(&args[2..].join(" "))?)
                    .map_err(|e| e.to_string())?
            );
        }
        "sequence-probe" => {
            let brain = Assistant::load(Path::new(required(1)?))?;
            let activity: Vec<f32> = serde_json::from_reader(
                std::fs::File::open(required(2)?).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            println!(
                "{}",
                brain.sequence_probe(&args[3..].join(" "), &activity, 64)?
            );
        }
        "monitor-learning-data" => {
            let brain = Assistant::load(Path::new(required(1)?))?;
            let spec: serde_json::Value = serde_json::from_reader(
                std::fs::File::open(required(2)?).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let result = brain.monitor_learning_data(&spec)?;
            let file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(required(3)?)
                .map_err(|e| e.to_string())?;
            serde_json::to_writer(file, &result).map_err(|e| e.to_string())?;
            println!("Saved lifecycle monitoring data.");
        }
        "consolidate" => {
            let path = Path::new(required(1)?);
            let mut brain = Assistant::load(path)?;
            brain.consolidate(path)?;
            println!("{}", brain.state()["adaptation"]);
        }
        "cache-instructions" => {
            let brain = Assistant::load(Path::new(required(1)?))?;
            let spec: serde_json::Value = serde_json::from_reader(
                std::fs::File::open(required(2)?).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            brain.cache_instruction_features(&spec, Path::new(required(3)?))?;
            println!("Saved instruction features.");
        }
        "monitor-data" => {
            let brain = Assistant::load(Path::new(required(1)?))?;
            let spec: serde_json::Value = serde_json::from_reader(
                std::fs::File::open(required(2)?).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let result = brain.monitoring_dataset(&spec)?;
            let file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(required(3)?)
                .map_err(|e| e.to_string())?;
            serde_json::to_writer(file, &result).map_err(|e| e.to_string())?;
            println!("Saved monitoring states.");
        }
        "monitor" => {
            let brain = Assistant::load(Path::new(required(1)?))?;
            println!("{}", brain.hidden_state_monitoring()?);
        }
        "assess" => {
            let brain = Assistant::load(Path::new(required(1)?))?;
            println!("{}", brain.assess_capabilities()?);
        }
        "stream" => {
            let mut brain = Assistant::load(Path::new(required(1)?))?;
            eprintln!(
                "Continuous circuit: text or JSON {{\"question\":\"...\",\"frames\":[[input_channels]]}}; /reset, /quit. No online training."
            );
            brain.reset_activity();
            for line in io::stdin().lock().lines() {
                let line = line.map_err(|e| e.to_string())?;
                if line.trim() == "/quit" {
                    break;
                }
                if line.trim() == "/reset" {
                    brain.reset_activity();
                    println!("{{\"reset\":true}}");
                    continue;
                }
                if line.trim().is_empty() {
                    continue;
                }
                let result = (|| -> Result<serde_json::Value, String> {
                    if line.len() > 4 << 20 {
                        return Err("Input exceeds 4 MiB.".into());
                    }
                    if line.starts_with('{') {
                        #[derive(serde::Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Input {
                            question: String,
                            #[serde(default)]
                            frames: Vec<Vec<usize>>,
                        }
                        let input: Input =
                            serde_json::from_str(&line).map_err(|e| e.to_string())?;
                        if !input.frames.is_empty() {
                            brain.sense(&input.frames)?;
                        }
                        brain.generate_continuing(&input.question, 64)
                    } else {
                        brain.generate_continuing(&line, 64)
                    }
                })();
                match result {
                    Ok(value) => println!("{value}"),
                    Err(error) => println!("{}", serde_json::json!({"error":error})),
                }
                io::stdout().flush().map_err(|e| e.to_string())?;
            }
        }
        "generate" => {
            let mut brain = Assistant::load(Path::new(required(1)?))?;
            println!("{}", brain.generate(&args[2..].join(" "), 64)?);
        }
        "generate-neural" => {
            let mut brain = Assistant::load(Path::new(required(1)?))?;
            println!("{}", brain.generate_neural(&args[2..].join(" "), 64)?);
        }
        "init" => {
            let config = if args.get(2).is_some_and(|s| s == "30m") {
                Config {
                    inputs: 512,
                    association: 1536,
                    episodic: 768,
                    context: 768,
                    action: 256,
                    value: 128,
                    inhibitory: 640,
                    max_synapses: 300_000,
                    ..Config::default()
                }
            } else if args.get(2).is_some_and(|s| s == "small") {
                Config {
                    inputs: 512,
                    ..Config::small(42)
                }
            } else {
                Config {
                    inputs: 512,
                    ..Config::default()
                }
            };
            let brain = Assistant::new(
                config,
                vec![
                    "(a+b)".into(),
                    "(a-b)".into(),
                    "(a*b)".into(),
                    "(a/b)".into(),
                    "force".into(),
                    "speed".into(),
                    "kinetic".into(),
                    "greeting".into(),
                    "identity".into(),
                    "capabilities".into(),
                    "unknown".into(),
                ],
            )?;
            brain.save(Path::new(required(1)?))?;
        }
        "probe" => {
            let mut brain = Assistant::load(Path::new(required(1)?))?;
            println!("{}", brain.probe(&args[2..].join(" "))?);
        }
        "calibrate" => {
            let dir = Path::new(required(1)?);
            let path = Path::new(required(2)?);
            let mut brain = Assistant::load(path)?;
            brain.calibrate(&read_lessons(&if dir.is_file() {
                dir.to_path_buf()
            } else {
                dir.join("dev.jsonl")
            })?)?;
            brain.save(path)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&brain.score(&read_lessons(&if dir.is_file() {
                    dir.to_path_buf()
                } else {
                    dir.join("dev.jsonl")
                })?)?)
                .map_err(|e| e.to_string())?
            );
        }
        "serve" => {
            let socket = args
                .get(2)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(cortex_brain::circuit::ipc::default_socket);
            cortex_brain::circuit::ipc::serve(Path::new(required(1)?), &socket)?;
        }
        "train" => {
            let dir = Path::new(required(1)?);
            let path = Path::new(required(2)?);
            let manifest: serde_json::Value = serde_json::from_reader(
                std::fs::File::open(dir.join("manifest.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let labels: Vec<String> =
                serde_json::from_value(manifest["catalog"].clone()).map_err(|e| e.to_string())?;
            let mut brain = Assistant::new(
                Config {
                    inputs: 512,
                    ..Config::default()
                },
                labels,
            )?;
            let train = read_lessons(&dir.join("train.jsonl"))?;
            let dev = read_lessons(&dir.join("dev.jsonl"))?;
            let epochs = args
                .get(3)
                .map(|s| s.parse())
                .transpose()
                .map_err(|_| "Invalid epoch count.")?
                .unwrap_or(100);
            let losses = brain.train(&train, epochs)?;
            brain.calibrate(&dev)?;
            brain.save(path)?;
            let report = serde_json::json!({"epochs":epochs,"loss":losses,"threshold":brain.threshold(),"train":brain.score(&train)?,"dev":brain.score(&dev)?,"manifest":manifest,"scope":"Plastic output synapses over spikes; recurrence and sensory encoding fixed. Exact arithmetic and English rendering supplied. No pretrained model."});
            std::fs::write(
                dir.join("training-report.json"),
                serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
        }
        "evaluate" => {
            let dir = Path::new(required(1)?);
            let mut brain = Assistant::load(Path::new(required(2)?))?;
            let report = serde_json::json!({"test":brain.score(&read_lessons(&dir.join("test.jsonl"))?)?,"broad":brain.score(&read_lessons(&dir.join("broad.jsonl"))?)?,"threshold":brain.threshold()});
            std::fs::write(
                dir.join("evaluation-report.json"),
                serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
        }
        "ask" => {
            let mut brain = Assistant::load(Path::new(required(1)?))?;
            let result = brain.ask(&args[2..].join(" "))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
        }
        "inspect" => {
            let brain = Assistant::load(Path::new(required(1)?))?;
            std::fs::write(
                required(2)?,
                serde_json::to_vec_pretty(&brain.wiring()).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            println!("Saved every simulated neuron, dendrite and connection.");
        }
        "chat" => {
            let path = Path::new(required(1)?);
            let mut brain = Assistant::load(path)?;
            println!(
                "Circuit conversation. /why, /english QUESTION, /teach PROGRAM | question, /replay, /status, /quit. Program selection uses supplied arithmetic/rendering; /english uses the experimental trained token decoder."
            );
            let mut last = None;
            print!("> ");
            io::stdout().flush().map_err(|e| e.to_string())?;
            for line in io::stdin().lock().lines() {
                let line = line.map_err(|e| e.to_string())?;
                if line.trim() == "/quit" {
                    break;
                }
                if let Some(value) = line.strip_prefix("/teach ") {
                    let (label, text) = value
                        .split_once('|')
                        .ok_or("Use /teach PROGRAM | question.")?;
                    brain.teach(text.trim(), label.trim())?;
                    brain.consolidate(path)?;
                    brain.save(path)?;
                    println!("Updated output synapses; saved.");
                } else if let Some(reward) = line.strip_prefix("/feedback ") {
                    brain.feedback(reward.trim().parse::<f32>().map_err(|e| e.to_string())?)?;
                    brain.consolidate(path)?;
                    brain.save(path)?;
                    println!("Feedback learned; saved.");
                } else if line == "/replay" {
                    let count = brain.replay()?;
                    brain.consolidate(path)?;
                    brain.save(path)?;
                    println!("Replayed {count} user-corrected experiences; saved.");
                } else if let Some(question) = line.strip_prefix("/english ") {
                    match brain.generate(question, 64) {
                        Ok(v) => println!("{}", v["text"].as_str().unwrap_or_default()),
                        Err(e) => eprintln!("{e}"),
                    }
                } else if line == "/status" {
                    println!(
                        "{} neurons; {} synapses; {} updates; confidence threshold {}; programs {}",
                        brain.network().neurons().len(),
                        brain.network().synapses().len(),
                        brain.updates,
                        brain.threshold(),
                        brain.labels().join(", ")
                    );
                } else if line == "/why" {
                    if let Some(ref r) = last {
                        let r: &cortex_brain::circuit::language::Response = r;
                        println!("{}", r.evidence.join("\n"));
                    } else {
                        println!("Ask a question first.");
                    }
                } else {
                    let result = brain.ask(&line)?;
                    println!(
                        "{} [score {:.3}; {}]",
                        result.text, result.confidence, result.program
                    );
                    last = Some(result);
                }
                print!("> ");
                io::stdout().flush().map_err(|e| e.to_string())?;
            }
        }
        _ => return Err("Unknown command.".into()),
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

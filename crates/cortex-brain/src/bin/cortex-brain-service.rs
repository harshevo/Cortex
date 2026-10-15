//! The brain's background service, and a few commands to look at it and
//! teach it from a terminal.

#[cfg(unix)]
fn main() {
    use cortex_brain::service::{self, Request};
    let result = if let Some(result) = service::serve_argument() {
        result.map(|_| String::new())
    } else {
        let mut args: Vec<_> = std::env::args().skip(1).collect();
        let selected_file = if args.first().is_some_and(|arg| arg == "--file") {
            if args.len() < 3 {
                eprintln!("Use --file <absolute brain file> <command>");
                std::process::exit(2);
            }
            let file = std::path::PathBuf::from(&args[1]);
            if !file.is_absolute() {
                eprintln!("Give an absolute brain file path");
                std::process::exit(2);
            }
            args.drain(..2);
            Some(file)
        } else {
            None
        };
        let rest = || args[1..].join(" ");
        let pretty =
            |v: serde_json::Value| serde_json::to_string_pretty(&v).map_err(std::io::Error::from);
        selected_file.map_or_else(service::default_file,Ok).and_then(|file| {
            let ask = |request: Request| {
                service::ensure(&file)?;
                service::request(&file, &request)
            };
            match args.first().map(String::as_str).unwrap_or("status") {
                "teach-start" => {
                    let mut config = service::teacher::Config::selected_in_desktop()?;
                    if args.len() > 1 {
                        config.topic = rest();
                    }
                    ask(Request::TeachStart { config }).map(|v| v.to_string())
                }
                "teach-start-json" => {
                    let config = serde_json::from_str(args.get(1).ok_or_else(|| {
                        std::io::Error::other("Give a teacher configuration JSON")
                    })?)?;
                    ask(Request::TeachStart { config }).map(|v| v.to_string())
                }
                "train" => service::ensure(&file).and_then(|_| train(&file, &args[1..])).map(|_| String::new()),
                "train-status" => service::request(&file, &Request::TrainStatus).and_then(pretty),
                "train-stop" => service::request(&file, &Request::TrainStop).map(|status| {
                    format!("Training stopped and saved; train on the same file goes on from here.\n{}", line(&status))
                }),
                "teach-stop" => service::request(&file, &Request::TeachStop).map(|v| v.to_string()),
                "teach-status" => service::request(&file, &Request::TeachStatus { after: 0 })
                    .map(|v| v.to_string()),
                "chat" => service::ensure(&file).and_then(|_| chat(&file)).map(|_| String::new()),
                "ask" | "say" => ask(Request::Converse { text: rest() }).map(|v| v.to_string()),
                "learn" => ask(Request::Learn {
                    text: rest(),
                    source: "command line".into(),
                    correction: false,
                })
                .map(|v| v.to_string()),
                "study" => ask(Request::Study {text:rest(),source:"command line".into(),max_passes:4}).and_then(pretty),
                "foundations" => {
                    service::ensure(&file)?;
                    let mut count=0;
                    for line in include_str!("../brain/foundations.jsonl").lines() {
                        let value:serde_json::Value=serde_json::from_str(line)?;
                        if let Some(text)=value["text"].as_str() {
                            ask(Request::Study { text:text.into(),source:"bundled introductory curriculum".into(),max_passes:4 })?;
                            count+=1;
                        }
                    }
                    Ok(format!("Studied {count} introductory lessons. They are supplied knowledge; held-out accuracy must be measured separately."))
                }
                "correct" => ask(Request::Learn {
                    text: rest(),
                    source: "command line".into(),
                    correction: true,
                })
                .map(|v| v.to_string()),
                "recall" => ask(Request::Recall { cue: rest() }).map(|v| v.to_string()),
                "think" => ask(Request::Think).map(|v| v.to_string()),
                "dream" => ask(Request::Dream).map(|v| v.to_string()),
                "reflect" => ask(Request::Reflect).map(|v| v.to_string()),
                "brain" => ask(Request::Progress).and_then(pretty),
                "start" => service::ensure(&file).map(|_| "Brain service started".into()),
                "status" => service::request(&file, &Request::Status).and_then(pretty),
                "import" => args
                    .get(1)
                    .ok_or_else(|| std::io::Error::other("Give a trajectory file or directory"))
                    .and_then(|source| service::import(&file, std::path::Path::new(source)))
                    .map(|count| format!("Queued {count} new dialogue events for the brain")),
                #[cfg(target_os = "macos")]
                "install" => service::install(&std::env::current_exe()?, &file)
                    .map(|p| format!("Installed the brain service: {}", p.display())),
                #[cfg(target_os = "macos")]
                "stop" => service::stop(&file)
                    .map(|_| "Brain service stopped; what it learned is kept".into()),
                _ => Err(std::io::Error::other(
                    "Use [--file <absolute brain file>] chat, foundations, study <lesson>, start, status, import <trajectories>, install, stop, teach-start [topic], \
                     teach-status, teach-stop, train <lessons file> [--sleep-every N] [--restart] [--detach], \
                     train-status, train-stop, say <words>, learn <text>, correct <text>, recall <cue>, \
                     think, dream, reflect, or brain",
                )),
            }
        })
    };
    match result {
        Ok(text) if !text.is_empty() => println!("{text}"),
        Ok(_) => {}
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

/// Starts training the service on a file of lessons, one a line, and
/// follows it until it ends. Leaving stops only the following.
#[cfg(unix)]
fn train(file: &std::path::Path, args: &[String]) -> std::io::Result<()> {
    use cortex_brain::service::{self, Request};
    let usage = || {
        std::io::Error::other("Use train <lessons file> [--sleep-every N] [--restart] [--detach]")
    };
    let (mut source, mut sleep_every, mut restart, mut detach) = (None, None, false, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--restart" => restart = true,
            "--detach" => detach = true,
            "--sleep-every" => {
                sleep_every = Some(
                    args.next()
                        .and_then(|n| n.parse::<u64>().ok())
                        .ok_or_else(usage)?,
                )
            }
            _ if source.is_none() && !arg.starts_with("--") => {
                source = Some(std::fs::canonicalize(arg)?)
            }
            _ => return Err(usage()),
        }
    }
    let source = source.ok_or_else(usage)?;
    let status = service::request(
        file,
        &Request::TrainStart {
            source,
            sleep_every,
            restart,
        },
    )?;
    let resumed = status["lessons"].as_u64().unwrap_or(0) > 0;
    println!(
        "{} {}; it trains in the background service.",
        if resumed {
            "Going on with training from"
        } else {
            "Training from"
        },
        status["source"].as_str().unwrap_or_default()
    );
    if detach {
        println!("Follow it with train-status; stop it with train-stop.");
        return Ok(());
    }
    println!("Ctrl-C stops following, not training; train-stop stops it.");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(10));
        // A long lesson can hold the service for a moment: try again.
        let Ok(status) = service::request(file, &Request::TrainStatus) else {
            continue;
        };
        println!("{}", line(&status));
        match status["state"].as_str() {
            Some("training") => {}
            Some("finished") => {
                println!("Finished; the brain is saved.");
                return Ok(());
            }
            other => {
                let problem = status["problem"].as_str().unwrap_or("");
                return Err(std::io::Error::other(format!(
                    "Training {}: {problem}",
                    other.unwrap_or("ended")
                )));
            }
        }
    }
}

/// One line of training progress.
#[cfg(unix)]
fn line(status: &serde_json::Value) -> String {
    let duration = |seconds: f64| {
        let seconds = seconds as u64;
        if seconds >= 3600 {
            format!("{}h{:02}m", seconds / 3600, seconds / 60 % 60)
        } else {
            format!("{}m{:02}s", seconds / 60, seconds % 60)
        }
    };
    let number = |v: &serde_json::Value| v.as_f64().unwrap_or(0.0);
    let mut parts = vec![
        format!("{:5.1}%", number(&status["percent"])),
        format!("{} lessons", status["lessons"]),
        format!("{:.2}M words", number(&status["words"]) / 1e6),
        duration(number(&status["seconds"])),
    ];
    if let Some(pace) = status["words_per_second"].as_f64() {
        parts.push(format!("{pace:.0} words/s"));
    }
    if let Some(left) = status["seconds_left_at_this_pace"].as_f64() {
        parts.push(format!("~{} left at this pace", duration(left)));
    }
    parts.push(format!(
        "{:.2} of {:.1} GB",
        number(&status["footprint"]) / 1e9,
        number(&status["budget"]) / 1e9
    ));
    if let Some(right) = status["next_word_right_recently"].as_f64() {
        parts.push(format!("next word right {:.0}%", right * 100.0));
    }
    parts.join("  ")
}

/// Talks with the brain from a terminal, a line at a time, until the input
/// ends or /quit. What it is told it learns, as in any conversation.
#[cfg(unix)]
fn chat(file: &std::path::Path) -> std::io::Result<()> {
    use cortex_brain::service::{self, Request};
    use std::io::{BufRead, Write};
    println!(
        "Talking to your brain; it learns from what you say. After a reply, /right keeps it and \
         /wrong <better answer> teaches the answer it should have given.\n\
         Also /teach <text>, /recall <cue>, /think, /dream, /reflect, /progress, and /quit."
    );
    let ask = |request: Request| service::request(file, &request);
    let session = format!(
        "terminal-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let mut last: Option<(String, String)> = None;
    let mut input = std::io::stdin().lock();
    loop {
        print!("you> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            println!();
            return Ok(());
        }
        let line = line.trim();
        let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim().to_string();
        let learn = |text: String, correction: bool| {
            ask(Request::Learn {
                text,
                source: "human feedback".into(),
                correction,
            })
        };
        let said = match command {
            "" => continue,
            "/quit" | "/exit" => return Ok(()),
            "/right" | "/wrong" => match &last {
                None => Err(std::io::Error::other("Say something to it first")),
                Some(_) if command == "/wrong" && rest.is_empty() => Err(std::io::Error::other(
                    "Give the answer it should have said: /wrong <better answer>",
                )),
                Some((heard, reply)) => {
                    let wrong = command == "/wrong";
                    let answer = if wrong { &rest } else { reply };
                    learn(format!("<user> {heard} <brain> {answer}"), wrong).map(|_| {
                        if wrong {
                            "It learned the better answer."
                        } else {
                            "It learned that its answer was right."
                        }
                        .to_string()
                    })
                }
            },
            "/teach" if !rest.is_empty() => ask(Request::Learn {
                text: rest,
                source: "chat".into(),
                correction: false,
            })
            .map(|_| "Learned.".to_string()),
            "/recall" if !rest.is_empty() => ask(Request::Recall { cue: rest }).and_then(|value| {
                let recall: cortex_brain::Recall = serde_json::from_value(value)?;
                Ok(match recall.engram {
                    Some(_) => format!(
                        "{}\n      (memory: \"{}\"; {})",
                        recall.recalled, recall.remembered, recall.why
                    ),
                    None => format!("…\n      (it remembers nothing for that; {})", recall.why),
                })
            }),
            "/think" | "/dream" | "/reflect" => ask(match command {
                "/think" => Request::Think,
                "/dream" => Request::Dream,
                _ => Request::Reflect,
            })
            .and_then(|value| {
                let thought: cortex_brain::Thought = serde_json::from_value(value)?;
                let text = if thought.text.is_empty() {
                    "…"
                } else {
                    &thought.text
                };
                let new = if thought.novel {
                    "; a sentence it was never taught"
                } else {
                    ""
                };
                Ok(format!(
                    "{text}\n      ({}{new}; {})",
                    thought.kind, thought.why
                ))
            }),
            "/progress" => ask(Request::Progress).and_then(|value| {
                serde_json::to_string_pretty(&value).map_err(std::io::Error::from)
            }),
            _ if command.starts_with('/') => Err(std::io::Error::other(
                "Use /right, /wrong <better answer>, /teach <text>, /recall <cue>, /think, /dream, /reflect, /progress or /quit",
            )),
            _ => ask(Request::Chat {
                session: session.clone(),
                text: line.to_string(),
            })
            .and_then(|value| {
                let said: cortex_brain::Utterance = serde_json::from_value(value)?;
                let text = if said.text.is_empty() {
                    "…"
                } else {
                    &said.text
                };
                let new = if said.novel {
                    format!("; new, never taught: {}", said.new_sentences.join("; "))
                } else {
                    String::new()
                };
                let reply = format!("{text}\n      ({}{new})", said.why);
                last = Some((line.to_string(), said.text));
                Ok(reply)
            }),
        };
        match said {
            Ok(text) => println!("brain> {text}"),
            Err(error) => println!("       {error}"),
        }
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("The brain service currently requires Unix.");
    std::process::exit(1);
}

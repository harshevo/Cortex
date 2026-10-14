#[cfg(unix)]
fn main() {
    use cortex_living::service::{self, Request};
    use serde_json::json;
    let result = if let Some(result) = service::serve_argument() {
        result.map(|_| String::new())
    } else {
        let args: Vec<_> = std::env::args().skip(1).collect();
        service::default_file().and_then(|file| match args.first().map(String::as_str).unwrap_or("status") {
            "teach-start" => {
                service::ensure(&file)?;
                let mut config = cortex_living::teacher::Config::selected_in_desktop()?;
                if args.len() > 1 { config.topic = args[1..].join(" "); }
                service::request(&file, &Request::TeachStart { config }).map(|v| v.to_string())
            },
            "teach-start-json" => {
                let config = serde_json::from_str(args.get(1).ok_or_else(|| std::io::Error::other("Give a teacher configuration JSON"))?)?;
                service::ensure(&file)?;
                service::request(&file, &Request::TeachStart { config }).map(|v| v.to_string())
            },
            "teach-stop" => service::request(&file, &Request::TeachStop).map(|v| v.to_string()),
            "teach-status" => service::request(&file, &Request::TeachStatus { after:0 }).map(|v| v.to_string()),
            "ask" | "say" => {
                service::ensure(&file)?;
                service::request(&file, &Request::NeuralConverse { text:args[1..].join(" ") }).map(|v| v.to_string())
            },
            "learn" => {
                service::ensure(&file)?;
                service::request(&file, &Request::NeuralLearn { text:args[1..].join(" "), source:"command line".into(), correction:false }).map(|v| v.to_string())
            },
            "recall" => {
                service::ensure(&file)?;
                service::request(&file, &Request::NeuralRecall { cue:args[1..].join(" ") }).map(|v| v.to_string())
            },
            "brain" => {
                service::ensure(&file)?;
                service::request(&file, &Request::NeuralProgress).and_then(|v| serde_json::to_string_pretty(&v).map_err(std::io::Error::from))
            },
            "start" => service::ensure(&file).map(|_| "Living network service started".into()),
            "status" => service::request(&file, &Request::Snapshot).and_then(|value| serde_json::to_string_pretty(&json!({"background":value["background"],"language":value["language"],"network":value["meters"]})).map_err(std::io::Error::from)),
            "inspect" => {
                let query = args.get(1).map(|s| serde_json::from_str(s)).transpose()?.unwrap_or_else(|| json!({}));
                service::request(&file, &Request::Language { query }).and_then(|v| serde_json::to_string_pretty(&v).map_err(std::io::Error::from))
            },
            "import" => args.get(1).ok_or_else(|| std::io::Error::other("Give a trajectory file or directory"))
                .and_then(|source| service::import(&file, std::path::Path::new(source)))
                .map(|count| format!("Queued {count} new dialogue events for learning")),
            #[cfg(target_os="macos")]
            "install" => service::install(&std::env::current_exe()?, &file).map(|p| format!("Installed persistent living service: {}", p.display())),
            #[cfg(target_os="macos")]
            "stop" => service::stop(&file).map(|_| "Living service stopped; learned state retained".into()),
            _ => Err(std::io::Error::other("Use start, status, inspect '<JSON>', import <trajectories>, install, stop, teach-start [topic], teach-status, teach-stop, say <words>, learn <text>, recall <cue>, or brain")),
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

#[cfg(not(unix))]
fn main() {
    eprintln!("The persistent living service currently requires Unix.");
    std::process::exit(1);
}

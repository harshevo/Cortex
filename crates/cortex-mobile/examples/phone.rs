//! A phone without a screen, for trying the core against a real desktop
//! app: `cargo run -p cortex-mobile --example phone -- <code file> [instruction]`.
//!
//! Pairs with the code in the file, prints what the screens would be told,
//! sends the instruction to the first chat, and allows whatever its tools
//! ask for.

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cortex_mobile::{Answer, Change, Connection, Cortex, ItemView, Listener};

struct Print(Mutex<Sender<Vec<Change>>>);

impl Listener for Print {
    fn changed(&self, changes: Vec<Change>) {
        let _ = self.0.lock().unwrap().send(changes);
    }

    fn open_address(&self, address: String) {
        println!("open: {address}");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);

    let code_file = args.next().expect("the file with the pairing code");
    let instruction = args.next();

    let folder = std::env::temp_dir().join(format!("cortex-phone-{}", std::process::id()));

    std::fs::create_dir_all(&folder).unwrap();

    let (sender, changes) = channel();

    let cortex = Cortex::new(
        folder.display().to_string(),
        "Test phone".to_string(),
        Arc::new(Print(Mutex::new(sender))),
    );

    let code = loop {
        match std::fs::read_to_string(&code_file) {
            Ok(code) if !code.is_empty() => break code,
            _ => std::thread::sleep(Duration::from_millis(200)),
        }
    };

    cortex.pair(code);

    let started = Instant::now();
    let mut sent = false;
    let mut shown = 0;
    let mut live_changes = 0;
    let mut toggled = false;

    while started.elapsed() < Duration::from_secs(60) {
        let Ok(batch) = changes.recv_timeout(Duration::from_millis(500)) else {
            continue;
        };

        let state = cortex.state();

        for change in &batch {
            if matches!(change, Change::Live { .. }) {
                live_changes += 1;
            }
        }

        if batch.contains(&Change::State) {
            println!(
                "state: {:?}, {} chats, {} projects",
                state.connection,
                state.chats.len(),
                state.projects.len()
            );
        }

        let Some(chat) = state.chats.first() else {
            continue;
        };

        if !matches!(state.connection, Connection::Connected { .. }) {
            continue;
        }

        if !toggled {
            toggled = true;

            let settings = cortex.settings(chat.id.clone());

            println!(
                "settings: model {} ({}), {} capabilities, {} on",
                settings.model,
                settings.provider,
                settings.capabilities.len(),
                settings.capabilities.iter().filter(|c| c.on).count()
            );
        }

        if chat.ready && !sent {
            sent = true;

            if let Some(instruction) = &instruction {
                println!("-> submit: {instruction}");

                cortex.submit(chat.id.clone(), instruction.clone(), Vec::new());
            }
        }

        let live = cortex.live(chat.id.clone());

        for approval in &live.approvals {
            println!("?? {}: {}  -> allow", approval.tool, approval.summary);

            cortex.answer(chat.id.clone(), approval.id, Answer::Allow);
        }

        let items = cortex.items(chat.id.clone());

        for item in items.iter().skip(shown) {
            match item {
                ItemView::User { text, .. } => println!("user: {text}"),
                ItemView::Assistant { text, .. } => println!("assistant: {}", first_line(text)),

                ItemView::Tools { tools, .. } => {
                    for tool in tools {
                        println!("tool: {} {}", tool.name, tool.summary);
                    }
                }

                ItemView::Checks { passed, .. } => println!("checks passed: {passed}"),
                ItemView::Summary {
                    headline, steps, ..
                } => {
                    println!("summary: {headline} in {steps} steps");
                }

                ItemView::Notice { text, .. } => println!("notice: {}", first_line(text)),
                ItemView::Briefing { source, .. } => println!("briefing: {source}"),
                ItemView::Review { .. } => println!("review"),
            }
        }

        // A group of tools grows in place; print it again when it has.
        shown = items.len().saturating_sub(1);

        if sent
            && instruction.is_some()
            && !live.running
            && items.len() > 1
            && items
                .iter()
                .any(|item| matches!(item, ItemView::Summary { .. }))
        {
            break;
        }
    }

    let memory = cortex
        .state()
        .chats
        .first()
        .map(|chat| cortex.memory(chat.id.clone()))
        .unwrap_or_default();

    println!(
        "done: {live_changes} live changes, memory {} nodes {} links",
        memory.nodes.len(),
        memory.links.len()
    );
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

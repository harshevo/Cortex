//! The browser's remote-control protocol (the Chrome DevTools Protocol),
//! spoken over a pair of pipes.
//!
//! Each message is one JSON object followed by a zero byte. A command has
//! an `id`, and its answer carries the same one; anything without an `id`
//! is an event the browser reports on its own.

use std::collections::HashMap;
use std::io::{PipeReader, PipeWriter, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use cortex_core::ToolError;
use serde_json::{Value, json};
use tokio::sync::oneshot;

/// How long the browser has to answer one command.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// Called with each event: its name, its details, and the page it is from.
pub type OnEvent = Box<dyn Fn(&Outbox, &str, &Value, Option<&str>) + Send>;

/// The writing half, which the event handler may also use to answer the
/// browser without waiting (to dismiss a dialog, say).
pub struct Outbox {
    writer: Mutex<PipeWriter>,
    next_id: AtomicU64,
}

impl Outbox {
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn write(
        &self,
        id: u64,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> std::io::Result<()> {
        let mut message = json!({ "id": id, "method": method, "params": params });

        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }

        let mut bytes = message.to_string().into_bytes();
        bytes.push(0);

        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        writer.write_all(&bytes)?;
        writer.flush()
    }

    /// An outbox for a test, and the pipe end that shows what it sent.
    #[cfg(test)]
    pub fn detached() -> (Self, PipeReader) {
        let (reader, writer) = std::io::pipe().unwrap();

        let outbox = Self {
            writer: Mutex::new(writer),
            next_id: AtomicU64::new(1),
        };

        (outbox, reader)
    }

    /// Sends a command whose answer nobody waits for.
    pub fn post(&self, method: &str, params: Value, session: Option<&str>) {
        let _ = self.write(self.next_id(), method, params, session);
    }
}

pub struct Connection {
    outbox: Arc<Outbox>,
    pending: Pending,
    closed: Arc<AtomicBool>,
}

impl Connection {
    /// Starts reading what the browser says. The reading is done on a
    /// thread of its own, which ends when the browser closes its pipe.
    pub fn open(writer: PipeWriter, reader: PipeReader, on_event: OnEvent) -> Self {
        let outbox = Arc::new(Outbox {
            writer: Mutex::new(writer),
            next_id: AtomicU64::new(1),
        });

        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));

        std::thread::Builder::new()
            .name("cortex-browser".to_string())
            .spawn({
                // Not a share of its own: once the connection is dropped
                // the pipe must close, which is what stops the browser.
                let outbox = Arc::downgrade(&outbox);
                let pending = pending.clone();
                let closed = closed.clone();

                move || {
                    read_messages(reader, |message| {
                        dispatch(&outbox, &pending, &on_event, message);
                    });

                    closed.store(true, Ordering::Relaxed);

                    // Dropping the senders tells everyone still waiting.
                    lock(&pending).clear();
                }
            })
            .expect("a thread can be started");

        Self {
            outbox,
            pending,
            closed,
        }
    }

    /// Whether the browser has gone away.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Sends a command and waits for its answer. `session` names the page
    /// it is for; without one it is for the browser as a whole.
    pub async fn send(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, ToolError> {
        let (sender, receiver) = oneshot::channel();

        let id = self.outbox.next_id();

        // Registered before the command is written, so the answer cannot
        // arrive before there is somewhere to put it.
        lock(&self.pending).insert(id, sender);

        if self.outbox.write(id, method, params, session).is_err() {
            lock(&self.pending).remove(&id);

            return Err(gone());
        }

        let answer = tokio::time::timeout(ANSWER_TIMEOUT, receiver).await;

        match answer {
            Ok(Ok(Ok(result))) => Ok(result),

            Ok(Ok(Err(complaint))) => Err(ToolError::Failed(complaint)),

            Ok(Err(_)) => Err(gone()),

            Err(_) => {
                lock(&self.pending).remove(&id);

                Err(ToolError::Failed(format!(
                    "the browser did not answer within {} seconds",
                    ANSWER_TIMEOUT.as_secs()
                )))
            }
        }
    }
}

fn gone() -> ToolError {
    ToolError::Failed("the browser has closed; `open` a page to start it again".to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Calls `handle` with each zero-terminated message until the pipe ends.
fn read_messages(mut reader: impl Read, mut handle: impl FnMut(&[u8])) {
    let mut buffer = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];

    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };

        let mut rest = &chunk[..read];

        while let Some(end) = rest.iter().position(|byte| *byte == 0) {
            buffer.extend_from_slice(&rest[..end]);
            handle(&buffer);
            buffer.clear();

            rest = &rest[end + 1..];
        }

        buffer.extend_from_slice(rest);
    }
}

fn dispatch(outbox: &Weak<Outbox>, pending: &Pending, on_event: &OnEvent, message: &[u8]) {
    let Ok(message) = serde_json::from_slice::<Value>(message) else {
        return;
    };

    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        if let (Some(method), Some(outbox)) = (
            message.get("method").and_then(Value::as_str),
            outbox.upgrade(),
        ) {
            on_event(
                &outbox,
                method,
                &message["params"],
                message.get("sessionId").and_then(Value::as_str),
            );
        }

        return;
    };

    let Some(waiting) = lock(pending).remove(&id) else {
        return;
    };

    let answer = match message.get("error") {
        Some(error) => Err(error["message"]
            .as_str()
            .unwrap_or("the browser refused the command")
            .to_string()),

        None => Ok(message["result"].clone()),
    };

    let _ = waiting.send(answer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_split_at_zero_bytes_wherever_the_reads_fall() {
        // Three messages, delivered three bytes at a time.
        struct Slow(Vec<u8>);

        impl Read for Slow {
            fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
                let take = self.0.len().min(3).min(into.len());

                into[..take].copy_from_slice(&self.0[..take]);
                self.0.drain(..take);

                Ok(take)
            }
        }

        let mut seen = Vec::new();

        read_messages(Slow(b"{\"a\":1}\0{\"b\":22}\0\0{}".to_vec()), |message| {
            seen.push(String::from_utf8(message.to_vec()).unwrap());
        });

        // What follows the last zero byte is not a whole message yet.
        assert_eq!(seen, ["{\"a\":1}", "{\"b\":22}", ""]);
    }

    #[tokio::test]
    async fn an_answer_reaches_the_command_it_belongs_to_and_events_reach_the_handler() {
        let (browser_reads, we_write) = std::io::pipe().unwrap();
        let (we_read, mut browser_writes) = std::io::pipe().unwrap();

        let events = Arc::new(Mutex::new(Vec::new()));

        let connection = Connection::open(
            we_write,
            we_read,
            Box::new({
                let events = events.clone();

                move |_, method, params, session| {
                    events
                        .lock()
                        .unwrap()
                        .push(format!("{method} {params} {}", session.unwrap_or("-")));
                }
            }),
        );

        // A stand-in browser: reports an event, then answers the command.
        let browser = std::thread::spawn(move || {
            let mut asked = Vec::new();

            read_messages(browser_reads, |message| {
                let command: Value = serde_json::from_slice(message).unwrap();

                asked.push(command["method"].as_str().unwrap().to_string());

                let event =
                    json!({ "method": "Page.loaded", "params": { "n": 1 }, "sessionId": "s" });
                let answer = match command["method"].as_str() {
                    Some("Fail") => {
                        json!({ "id": command["id"], "error": { "message": "no such thing" } })
                    }
                    _ => json!({ "id": command["id"], "result": { "echo": command["params"] } }),
                };

                for reply in [event, answer] {
                    browser_writes
                        .write_all(reply.to_string().as_bytes())
                        .unwrap();
                    browser_writes.write_all(&[0]).unwrap();
                }
            });

            asked
        });

        let answer = connection
            .send("Echo", json!({ "x": 5 }), None)
            .await
            .unwrap();

        assert_eq!(answer["echo"]["x"], 5);

        let refused = connection.send("Fail", json!({}), Some("s")).await;

        assert!(refused.unwrap_err().to_string().contains("no such thing"));

        assert_eq!(events.lock().unwrap()[0], "Page.loaded {\"n\":1} s");

        // Closing our end is what tells the browser to stop.
        drop(connection);

        assert_eq!(browser.join().unwrap(), ["Echo", "Fail"]);
    }
}

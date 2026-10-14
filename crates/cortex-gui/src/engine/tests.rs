use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::settings::Provider;

struct ScriptedServer {
    address: String,
    requests: mpsc::Receiver<Value>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ScriptedServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (sender, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let sender = sender.clone();
                        std::thread::spawn(move || answer(stream, sender));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("scripted server accept failed: {error}"),
                }
            }
        });
        Self {
            address,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for ScriptedServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn answer(mut stream: TcpStream, sender: mpsc::Sender<Value>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let (body, length) = loop {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end]);
            let length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            break (end + 4, length);
        }
    };
    while bytes.len() < body + length {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
    }
    let request: Value = serde_json::from_slice(&bytes[body..body + length]).unwrap();
    let slow = request.to_string().contains("slow task");
    sender.send(request).unwrap();
    if slow {
        std::thread::sleep(Duration::from_millis(250));
    }
    let response = json!({
        "id":"scripted", "object":"chat.completion", "model":"scripted",
        "choices":[{"index":0,"message":{"role":"assistant","content":"Work complete."},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}
    }).to_string();
    let http = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    );
    let _ = stream.write_all(http.as_bytes());
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("engine did not reach the expected state");
}

fn launch(place: &std::path::Path, server: &ScriptedServer) -> Launch {
    let mut settings = Settings {
        provider: Provider::Local,
        ..Settings::default()
    };
    settings.local.model = "scripted".into();
    settings.local.base_url = server.address.clone();
    settings.capabilities.clear();
    Launch {
        place: place.into(),
        settings,
        api_key: None,
        look_up_key: false,
        demo: false,
        fresh: true,
        open: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forked_workers_deliver_while_idle_and_running_and_close_cleanly() {
    let folder = tempfile::tempdir().unwrap();
    let server = ScriptedServer::start();
    let first = Engine::start_without_embeddings(&Handle::current(), egui::Context::default());
    let second = first.fork();
    assert_ne!(first.agent_id(), second.agent_id());
    first.launch(launch(folder.path(), &server));
    second.launch(launch(folder.path(), &server));
    let mut first_ready = false;
    let mut second_ready = false;
    until(|| {
        for update in first.updates() {
            match update {
                Update::Ready(_) => first_ready = true,
                Update::Unavailable(error) => panic!("{error}"),
                _ => {}
            }
        }
        for update in second.updates() {
            match update {
                Update::Ready(_) => second_ready = true,
                Update::Unavailable(error) => panic!("{error}"),
                _ => {}
            }
        }
        first_ready && second_ready
    })
    .await;

    first
        .member
        .send(second.agent_id(), "idle collaboration request")
        .unwrap();
    let mut idle_request = None;
    until(|| {
        idle_request = server.requests.try_recv().ok();
        idle_request.is_some()
    })
    .await;
    assert!(
        idle_request
            .unwrap()
            .to_string()
            .contains("idle collaboration request")
    );
    let mut second_idle = false;
    until(|| {
        second_idle |= second
            .updates()
            .any(|update| matches!(update, Update::Idle));
        second_idle
    })
    .await;

    first.submit("slow task", Vec::new());
    let mut slow_started = false;
    until(|| {
        while let Ok(request) = server.requests.try_recv() {
            slow_started |= request.to_string().contains("slow task");
        }
        slow_started
    })
    .await;
    second.submit("independent task", Vec::new());
    second
        .member
        .send(first.agent_id(), "running collaboration request")
        .unwrap();
    let mut saw_independent = false;
    let mut saw_running_message = false;
    until(|| {
        while let Ok(request) = server.requests.try_recv() {
            let request = request.to_string();
            saw_independent |= request.contains("independent task");
            saw_running_message |= request.contains("running collaboration request");
        }
        saw_independent && saw_running_message
    })
    .await;

    let second_member = second.member.clone();
    drop(second);
    assert!(first.member.send(&second_member.id, "closed").is_err());
    second_member.attach(Some(folder.path().into()));
    assert!(
        first
            .member
            .send(&second_member.id, "cannot resurrect")
            .is_err()
    );
}

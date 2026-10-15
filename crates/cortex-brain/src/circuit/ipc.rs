//! Local IPC for the live circuit and desktop view.
use super::{Live, language::Assistant, network::Rng};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const MAX_REQUEST: u64 = 16 * 1024;
const MAX_REPLY: u64 = 16 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Live { since: u64 },
    State,
    Activity { enabled: bool },
    Plasticity { enabled: bool },
    Dialogue { text: String },
    Chat { text: String },
    Teach { text: String, label: String },
    Replay,
    Feedback { reward: f32 },
}

pub fn default_socket() -> PathBuf {
    std::env::var_os("CORTEX_CIRCUIT_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".cortex/brain/circuit.sock")
        })
}

pub fn call(socket: &Path, request: &Request, timeout: Duration) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| format!("Cannot reach the live brain at {}: {e}", socket.display()))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_REQUEST {
        return Err("Request exceeds 16 KiB.".into());
    }
    stream.write_all(&bytes).map_err(|e| e.to_string())?;
    let mut reply = Vec::new();
    BufReader::new(stream)
        .take(MAX_REPLY + 1)
        .read_until(b'\n', &mut reply)
        .map_err(|e| e.to_string())?;
    if reply.len() as u64 > MAX_REPLY || reply.last() != Some(&b'\n') {
        return Err("Incomplete or oversized brain reply.".into());
    }
    let value: Value = serde_json::from_slice(&reply).map_err(|e| e.to_string())?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.into());
    }
    value
        .get("result")
        .cloned()
        .ok_or("Missing brain reply.".into())
}

struct Shared {
    brain: Mutex<Assistant>,
    live: Arc<Mutex<Live>>,
    path: PathBuf,
    activity: Mutex<Background>,
}

struct Background {
    enabled: bool,
    plasticity_enabled: bool,
    cycles: u64,
    drive: f32,
    rng: Rng,
    error: Option<String>,
}

impl Background {
    fn new() -> Self {
        Self {
            enabled: true,
            plasticity_enabled: true,
            cycles: 0,
            drive: 0.08,
            rng: Rng::new(731),
            error: None,
        }
    }
    fn advance(&mut self, brain: &mut Assistant) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let resting = (self.cycles / 300) % 2 == 1;
        let probability = self.drive * if resting { 0.25 } else { 1.0 };
        let frames: Vec<Vec<usize>> = (0..10)
            .map(|_| {
                (0..brain.network().config().inputs)
                    .filter(|_| self.rng.unit() < probability)
                    .collect()
            })
            .collect();
        brain.sense(&frames)?;
        let active = brain
            .network()
            .neurons()
            .iter()
            .filter(|n| n.trace > 0.01)
            .count() as f32
            / brain.network().neurons().len() as f32;
        self.drive = (self.drive + (0.2 - active) * 0.001).clamp(0.01, 0.25);
        self.cycles += 1;
        Ok(())
    }
    fn status(&self) -> serde_json::Value {
        serde_json::json!({"enabled":self.enabled,"plasticity_enabled":self.plasticity_enabled,"cycles":self.cycles,"drive":self.drive,
            "phase":if (self.cycles / 300) % 2 == 1 {"rest"} else {"explore"},"error":self.error,
            "scope":"Seeded stochastic background input and bounded activity regulation; not an external observation, learning label, or evidence of awareness."})
    }
}

pub fn serve(path: &Path, socket: &Path) -> Result<(), String> {
    let parent = socket
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(socket.with_extension("sock.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock()
        .map_err(|_| "A circuit service already owns this socket.".to_string())?;
    let mut brain = Assistant::load(path)?;
    let live = brain.watch();
    let shared = Arc::new(Shared {
        brain: Mutex::new(brain),
        live,
        path: path.to_path_buf(),
        activity: Mutex::new(Background::new()),
    });
    // Only remove a stale socket after acquiring the lifetime lock.
    if socket.exists() {
        fs::remove_file(socket).map_err(|e| e.to_string())?;
    }
    let listener = UnixListener::bind(socket).map_err(|e| e.to_string())?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    let background = Arc::downgrade(&shared);
    std::thread::Builder::new()
        .name("circuit-background".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let Some(shared) = background.upgrade() else {
                    break;
                };
                if let Ok(mut brain) = shared.brain.try_lock() {
                    let mut activity = shared.activity.lock().unwrap_or_else(|e| e.into_inner());
                    if let Err(error) = activity.advance(&mut brain) {
                        activity.error = Some(error);
                        activity.enabled = false;
                    }
                    let consolidate = activity.enabled
                        && activity.plasticity_enabled
                        && activity.cycles > 0
                        && activity.cycles.is_multiple_of(300);
                    drop(activity);
                    if consolidate && let Err(error) = brain.consolidate(&shared.path) {
                        shared
                            .activity
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .error = Some(error);
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;
    println!(
        "Live circuit IPC: {} · checkpoint {}",
        socket.display(),
        path.display()
    );
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            continue;
        };
        if active.fetch_add(1, Ordering::Relaxed) >= 16 {
            active.fetch_sub(1, Ordering::Relaxed);
            continue;
        }
        let shared = Arc::clone(&shared);
        let active_thread = Arc::clone(&active);
        if std::thread::Builder::new()
            .name("circuit-request".into())
            .spawn(move || {
                answer(stream, &shared);
                active_thread.fetch_sub(1, Ordering::Relaxed);
            })
            .is_err()
        {
            active.fetch_sub(1, Ordering::Relaxed);
        }
    }
    Ok(())
}

fn answer(mut stream: UnixStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let result = (|| {
        let mut bytes = Vec::new();
        BufReader::new(&mut stream)
            .take(MAX_REQUEST + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_REQUEST || bytes.last() != Some(&b'\n') {
            return Err("Incomplete or oversized request.".into());
        }
        let request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        dispatch(shared, request)
    })();
    let value = match result {
        Ok(value) => json!({"result":value}),
        Err(error) => json!({"error":error}),
    };
    if let Ok(mut bytes) = serde_json::to_vec(&value) {
        bytes.push(b'\n');
        let _ = stream.write_all(&bytes);
    }
}

fn dispatch(shared: &Shared, request: Request) -> Result<Value, String> {
    if let Request::Live { since } = request {
        let mut value = shared
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .since(since);
        if let Some(frames) = value["frames"].as_array_mut()
            && frames.len() > 256
        {
            frames.drain(..frames.len() - 256);
        }
        value["background_activity"] = shared
            .activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status();
        return Ok(value);
    }
    if let Request::Activity { enabled } = request {
        let mut activity = shared.activity.lock().unwrap_or_else(|e| e.into_inner());
        activity.enabled = enabled;
        activity.error = None;
        return Ok(activity.status());
    }
    if let Request::Plasticity { enabled } = request {
        let mut activity = shared.activity.lock().unwrap_or_else(|e| e.into_inner());
        activity.plasticity_enabled = enabled;
        return Ok(activity.status());
    }
    if let Request::State = request {
        let brain = shared
            .brain
            .try_lock()
            .map_err(|_| "Brain is busy; retry its state shortly.".to_string())?;
        let mut value = brain.state();
        value["background_activity"] = shared
            .activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status();
        value["checkpoint"] = json!(shared.path);
        return Ok(value);
    }
    let mut brain = shared.brain.lock().unwrap_or_else(|e| e.into_inner());
    match request {
        Request::Dialogue { text } => brain.generate_continuing(&text, 64),
        Request::Chat { text } => {
            serde_json::to_value(brain.ask(&text)?).map_err(|e| e.to_string())
        }
        Request::Teach { text, label } => {
            brain.teach(&text, &label)?;
            brain.consolidate(&shared.path)?;
            brain.save(&shared.path)?;
            Ok(json!({"saved":true,"updates":brain.updates}))
        }
        Request::Feedback { reward } => {
            brain.feedback_checkpointed(reward, &shared.path)?;
            Ok(json!({"saved":true,"adaptation":brain.state()["adaptation"]}))
        }
        Request::Replay => {
            let count = brain.replay()?;
            brain.consolidate(&shared.path)?;
            brain.save(&shared.path)?;
            Ok(json!({"replayed":count,"updates":brain.updates}))
        }
        Request::Live { .. }
        | Request::Activity { .. }
        | Request::Plasticity { .. }
        | Request::State => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::Config;
    #[test]
    fn the_live_record_follows_the_circuit_as_it_runs() {
        let mut brain =
            Assistant::new(Config::small(9), vec!["greeting".into(), "unknown".into()]).unwrap();
        let live = brain.watch();
        let _ = brain.ask("hello there");
        let live = live.lock().unwrap();
        assert!(live.frame > 0);
        assert_eq!(live.voltages.len(), brain.populations().len());
        let all = live.since(0);
        let later = live.since(live.frame - 1);
        assert_eq!(all["frames"].as_array().unwrap().len(), live.frames.len());
        assert_eq!(later["frames"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn background_advances_and_spikes_without_teaching_and_can_pause() {
        let mut brain =
            Assistant::new(Config::small(9), vec!["greeting".into(), "unknown".into()]).unwrap();
        let mut activity = Background::new();
        for _ in 0..100 {
            activity.advance(&mut brain).unwrap();
        }
        assert_eq!(brain.network().ticks(), 1000);
        assert!(brain.network().neurons().iter().any(|n| n.spikes > 0));
        assert_eq!(brain.updates, 0);
        activity.enabled = false;
        activity.advance(&mut brain).unwrap();
        assert_eq!(brain.network().ticks(), 1000);
    }

    #[test]
    fn ipc_reads_live_activity_and_reports_invalid_messages() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("brain.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let mut brain =
            Assistant::new(Config::small(9), vec!["greeting".into(), "unknown".into()]).unwrap();
        let live = brain.watch();
        let mut activity = Background::new();
        activity.advance(&mut brain).unwrap();
        let shared = Shared {
            brain: Mutex::new(brain),
            live,
            path: dir.path().join("brain.json"),
            activity: Mutex::new(activity),
        };
        let worker = std::thread::spawn(move || {
            for _ in 0..4 {
                let (stream, _) = listener.accept().unwrap();
                answer(stream, &shared);
            }
        });
        let live = call(&socket, &Request::Live { since: 0 }, Duration::from_secs(2)).unwrap();
        assert!(live["frame"].as_u64().unwrap() > 0);
        assert!(live["background_activity"]["enabled"].as_bool().unwrap());
        let paused = call(
            &socket,
            &Request::Activity { enabled: false },
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(paused["enabled"], false);
        let state = call(&socket, &Request::State, Duration::from_secs(2)).unwrap();
        assert_eq!(state["background_activity"]["enabled"], false);
        let mut client = UnixStream::connect(&socket).unwrap();
        client.write_all(b"{\"command\":\"bogus\"}\n").unwrap();
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        assert!(serde_json::from_str::<Value>(&line).unwrap()["error"].is_string());
        worker.join().unwrap();
    }
    #[test]
    fn telemetry_and_pause_do_not_wait_for_generation() {
        let mut brain =
            Assistant::new(Config::small(9), vec!["greeting".into(), "unknown".into()]).unwrap();
        let shared = Shared {
            live: brain.watch(),
            brain: Mutex::new(brain),
            path: PathBuf::from("unused.json"),
            activity: Mutex::new(Background::new()),
        };
        let _generating = shared.brain.lock().unwrap();
        assert!(dispatch(&shared, Request::State).is_err());
        assert!(dispatch(&shared, Request::Live { since: 0 }).is_ok());
        assert_eq!(
            dispatch(&shared, Request::Activity { enabled: false }).unwrap()["enabled"],
            false
        );
    }
}

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cortex_core::{Adviser, Event};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Counsel, Living, Network, Snapshot};

const MAX_EVENT: u64 = 512 * 1024;
const MAX_REPLY: u64 = 16 * 1024 * 1024;
const LABEL: &str = "dev.cortex.living-network";

#[derive(Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    TeachStart {
        config: crate::teacher::Config,
    },
    TeachStop,
    TeachStatus {
        after: u64,
    },
    BrainLesson {
        id: String,
        session: String,
        lesson: crate::tutoring::Lesson,
        source: String,
    },
    BrainAsk {
        session: String,
        question: String,
        learn_user: bool,
    },
    BrainFeedback {
        attempt: String,
        correct: bool,
        feedback: String,
        correction: Option<String>,
        source: String,
    },
    Hear {
        session: String,
        speaker: String,
        text: String,
    },
    /// The standalone brain: learn text (twice when it is a correction),
    /// say what follows a prompt, answer a turn of conversation, recall
    /// the memory a cue brings back, and its state for the view and the
    /// teacher.
    NeuralLearn {
        text: String,
        source: String,
        #[serde(default)]
        correction: bool,
    },
    NeuralSay {
        prompt: String,
    },
    NeuralConverse {
        text: String,
    },
    NeuralRecall {
        cue: String,
    },
    NeuralSnapshot,
    NeuralProgress,
    Snapshot,
    Flush,
    Suggestion,
    Hunch,
    Inspect {
        query: Value,
    },
    Language {
        query: Value,
    },
    Consolidate {
        milliseconds: u64,
    },
    Shutdown,
}

#[derive(Serialize, Deserialize)]
struct Delivery {
    id: String,
    session: String,
    event: Event,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    value: Value,
    error: Option<String>,
}

fn directory(file: &Path) -> &Path {
    file.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn socket(file: &Path) -> PathBuf {
    file.with_extension("sock")
}

fn inbox(file: &Path) -> PathBuf {
    file.with_extension("inbox")
}

fn archive(file: &Path) -> PathBuf {
    file.with_extension("experience")
}

fn private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// Nobody must have used the brain for this long before it sleeps, and
/// then it sleeps a cycle this often.
const NEURAL_QUIET: Duration = Duration::from_secs(5);
const NEURAL_SLEEP: Duration = Duration::from_secs(10);

/// Where the standalone brain is kept, beside the network.
pub fn neural_file(file: &Path) -> PathBuf {
    file.with_file_name("brain.bin")
}

/// The saved brain, or a new one. One that cannot be read is moved aside,
/// not overwritten.
fn open_brain(file: &Path) -> cortex_neural::Brain {
    if !file.exists() {
        return cortex_neural::Brain::new();
    }
    match cortex_neural::Brain::load(file) {
        Ok(brain) => brain,
        Err(error) => {
            let aside = file.with_extension(format!(
                "rejected-{}.bin",
                cortex_core::trajectory::unix_millis()
            ));
            eprintln!(
                "warning: {} could not be read ({error}); kept as {} and starting a new brain",
                file.display(),
                aside.display()
            );
            let _ = fs::rename(file, aside);
            cortex_neural::Brain::new()
        }
    }
}

pub fn default_file() -> io::Result<PathBuf> {
    Ok(std::env::home_dir()
        .ok_or_else(|| io::Error::other("Cannot locate home directory"))?
        .join(".cortex/living/network.json"))
}

pub fn request(file: &Path, request: &Request) -> io::Result<Value> {
    let mut stream = UnixStream::connect(socket(file))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut bytes = serde_json::to_vec(request)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)?;
    let mut line = Vec::new();
    BufReader::new(stream)
        .take(MAX_REPLY)
        .read_until(b'\n', &mut line)?;
    if line.last() != Some(&b'\n') {
        return Err(io::Error::other("Incomplete service response"));
    }
    let reply: Reply = serde_json::from_slice(&line)?;
    if let Some(error) = reply.error {
        return Err(io::Error::other(error));
    }
    Ok(reply.value)
}

pub fn ensure(file: &Path) -> io::Result<()> {
    if request(file, &Request::Snapshot).is_ok() {
        return Ok(());
    }
    private_directory(directory(file))?;
    #[cfg(target_os = "macos")]
    if default_file().is_ok_and(|default| file == default)
        && let Some(home) = std::env::home_dir()
        && home
            .join(format!("Library/LaunchAgents/{LABEL}.plist"))
            .is_file()
    {
        let domain = domain()?;
        let job = format!("{domain}/{LABEL}");
        let started = Command::new("/bin/launchctl")
            .args(["kickstart", &job])
            .output()?;
        if !started.status.success() {
            let loaded = Command::new("/bin/launchctl")
                .arg("bootstrap")
                .arg(&domain)
                .arg(home.join(format!("Library/LaunchAgents/{LABEL}.plist")))
                .output()?;
            if !loaded.status.success() {
                return Err(io::Error::other(
                    String::from_utf8_lossy(&loaded.stderr).to_string(),
                ));
            }
        }
        return wait_ready(file);
    }
    let installed = std::env::home_dir().map(|p| p.join(".cortex/bin/cortex-living-service"));
    let executable = installed
        .filter(|p| p.is_file())
        .unwrap_or(std::env::current_exe()?);
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory(file).join("service.log"))?;
    Command::new(executable)
        .arg("--living-service")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .process_group(0)
        .spawn()?;
    wait_ready(file)
}

fn wait_ready(file: &Path) -> io::Result<()> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(8) {
        if request(file, &Request::Snapshot).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::other(format!(
        "Living service did not start; see {}",
        directory(file).join("service.log").display()
    )))
}

pub struct Client {
    file: PathBuf,
    cached: Mutex<Snapshot>,
    neural: Mutex<Option<cortex_neural::NeuralSnapshot>>,
}

impl Client {
    pub fn connect(file: &Path) -> io::Result<Arc<Self>> {
        ensure(file)?;
        private_directory(&inbox(file))?;
        let snapshot = serde_json::from_value(request(file, &Request::Snapshot)?)?;
        let client = Arc::new(Self {
            file: file.to_path_buf(),
            cached: Mutex::new(snapshot),
            neural: Mutex::new(None),
        });
        let weak = Arc::downgrade(&client);
        std::thread::Builder::new()
            .name("living-snapshot".into())
            .spawn(move || {
                let mut last_retry = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let Some(client) = weak.upgrade() else {
                        break;
                    };
                    if let Ok(neural) = client
                        .request(&Request::NeuralSnapshot)
                        .and_then(|v| serde_json::from_value(v).map_err(io::Error::from))
                    {
                        *client.neural.lock().unwrap_or_else(|p| p.into_inner()) = Some(neural);
                    }
                    match client
                        .request(&Request::Snapshot)
                        .and_then(|v| serde_json::from_value(v).map_err(io::Error::from))
                    {
                        Ok(snapshot) => {
                            *client.cached.lock().unwrap_or_else(|p| p.into_inner()) = snapshot
                        }
                        Err(error) => {
                            let mut cached =
                                client.cached.lock().unwrap_or_else(|p| p.into_inner());
                            cached.background.running = false;
                            cached.background.problem = Some(error.to_string());
                            cached.background.cycles += 1;
                            drop(cached);
                            if last_retry.elapsed() >= Duration::from_secs(15) {
                                let _ = ensure(&client.file);
                                last_retry = Instant::now();
                            }
                        }
                    }
                }
            })?;
        Ok(client)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.cached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The standalone brain as last fetched.
    pub fn neural(&self) -> Option<cortex_neural::NeuralSnapshot> {
        self.neural.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn stamp(&self) -> (u64, u64, bool) {
        let snapshot = self.cached.lock().unwrap_or_else(|p| p.into_inner());
        (
            snapshot.moment,
            snapshot
                .meters
                .replays
                .wrapping_add(snapshot.background.cycles),
            snapshot.resting,
        )
    }

    pub fn request(&self, query: &Request) -> io::Result<Value> {
        request(&self.file, query)
    }

    pub fn record(&self, session: &str, event: &Event) {
        if let Err(error) = self.enqueue(session, event) {
            let mut cached = self.cached.lock().unwrap_or_else(|p| p.into_inner());
            cached.background.problem = Some(format!("Could not retain learning event: {error}"));
            eprintln!("warning: could not retain living network event: {error}");
        }
    }

    fn enqueue(&self, session: &str, event: &Event) -> io::Result<()> {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!(
            "{now:020}-{:010}-{:010}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        let mut event = event.clone();
        match &mut event {
            Event::ModelResponded { message, .. } => message.blocks.retain(|block| {
                matches!(
                    block,
                    cortex_core::Block::Text { .. } | cortex_core::Block::ToolCall(_)
                )
            }),
            Event::ToolFinished { result, .. } => {
                result.output = result.output.chars().take(256).collect();
                result.images.clear();
            }
            _ => {}
        }
        let delivery = Delivery {
            id: id.clone(),
            session: session.to_string(),
            event,
        };
        enqueue(&self.file, &delivery)?;
        Ok(())
    }
}

fn enqueue(file: &Path, delivery: &Delivery) -> io::Result<bool> {
    let name = format!("{}.json", delivery.id);
    if archive(file).join(&name).exists() || inbox(file).join(&name).exists() {
        return Ok(false);
    }
    let bytes = serde_json::to_vec(delivery)?;
    if bytes.len() as u64 > MAX_EVENT {
        return Err(io::Error::other("Learning event exceeds 512 KiB"));
    }
    let folder = inbox(file);
    let mut temporary = tempfile::NamedTempFile::new_in(&folder)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(folder.join(name)).map_err(|e| e.error)?;
    Ok(true)
}

pub fn import(file: &Path, source: &Path) -> io::Result<usize> {
    ensure(file)?;
    private_directory(&inbox(file))?;
    let source = source.canonicalize()?;
    let mut files = if source.is_file() {
        vec![source]
    } else {
        fs::read_dir(source)?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "jsonl"))
            .collect()
    };
    files.sort();
    let mut imported = 0;
    for path in files {
        let mut hash = 14695981039346656037_u64;
        for byte in path.as_os_str().as_encoded_bytes() {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(1099511628211);
        }
        let session = format!("history-{hash:016x}");
        let mut reader = BufReader::new(File::open(&path)?);
        let mut index = 0_u64;
        loop {
            let mut line = Vec::new();
            if (&mut reader).take(MAX_REPLY).read_until(b'\n', &mut line)? == 0 {
                break;
            }
            index += 1;
            if line.last() != Some(&b'\n') && !reader.fill_buf()?.is_empty() {
                return Err(io::Error::other(format!(
                    "Oversized trajectory record at {}:{index}",
                    path.display()
                )));
            }
            let value: Value = serde_json::from_slice(&line).map_err(|e| {
                io::Error::other(format!(
                    "Unreadable trajectory {}:{index}: {e}",
                    path.display()
                ))
            })?;
            let mut event: Event =
                serde_json::from_value(value.get("event").cloned().unwrap_or(value))?;
            if let Event::ModelResponded { message, .. } = &mut event {
                message
                    .blocks
                    .retain(|block| matches!(block, cortex_core::Block::Text { .. }));
                if message.text().trim().is_empty() {
                    continue;
                }
            }
            if !matches!(
                &event,
                Event::RunStarted { .. }
                    | Event::UserInterjected { .. }
                    | Event::ModelResponded { .. }
            ) {
                continue;
            }
            let delivery = Delivery {
                id: format!("{session}-{index:020}"),
                session: session.clone(),
                event,
            };
            imported += usize::from(enqueue(file, &delivery)?);
        }
    }
    Ok(imported)
}

fn pending(file: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files: Vec<_> = fs::read_dir(inbox(file))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    files.sort();
    Ok(files)
}

fn drain(file: &Path, living: &Living) -> io::Result<usize> {
    let files = pending(file)?;
    let mut learned = Vec::new();
    for path in files.iter().take(32) {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("Missing delivery filename"))?;
        if archive(file).join(name).exists() {
            fs::remove_file(path)?;
            continue;
        }
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_EVENT + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_EVENT {
            fs::rename(path, path.with_extension("rejected"))?;
            eprintln!(
                "warning: retained oversized learning event {}",
                path.display()
            );
            continue;
        }
        let delivery: Delivery = match serde_json::from_slice(&bytes) {
            Ok(delivery) => delivery,
            Err(error) => {
                fs::rename(path, path.with_extension("rejected"))?;
                eprintln!(
                    "warning: retained malformed learning event {} for inspection: {error}",
                    path.display()
                );
                continue;
            }
        };
        if path.file_stem().and_then(|s| s.to_str()) != Some(&delivery.id) {
            fs::rename(path, path.with_extension("rejected"))?;
            eprintln!(
                "warning: retained learning event with inconsistent identity {}",
                path.display()
            );
            continue;
        }
        living.deliver(&delivery.id, &delivery.session, &delivery.event);
        learned.push(path);
    }
    if !learned.is_empty() {
        living.checkpoint()?;
        for path in learned {
            fs::rename(
                path,
                archive(file).join(
                    path.file_name()
                        .ok_or_else(|| io::Error::other("Missing delivery filename"))?,
                ),
            )?;
        }
    }
    Ok(files.len().saturating_sub(32))
}

pub fn run(file: &Path) -> io::Result<()> {
    private_directory(directory(file))?;
    private_directory(&inbox(file))?;
    private_directory(&archive(file))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(file.with_extension("service.lock"))?;
    lock.try_lock().map_err(|error| {
        io::Error::other(format!(
            "A living service already owns this network: {error}"
        ))
    })?;
    if file.exists() {
        let network: Network = serde_json::from_reader(File::open(file)?).map_err(|error| {
            io::Error::other(format!(
                "Refusing to overwrite unreadable network {}: {error}",
                file.display()
            ))
        })?;
        if !network.is_sound() {
            return Err(io::Error::other(
                "Refusing to load invalid neural parameters",
            ));
        }
    }
    let path = socket(file);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let living = Arc::new(Living::open(Some(file.to_path_buf())));
    let counsel = Counsel(living.clone());
    let brain_file = neural_file(file);
    let mut brain = open_brain(&brain_file);
    let mut brain_dirty = false;
    let mut brain_saved = Instant::now();
    let mut brain_used = Instant::now();
    let mut brain_slept = Instant::now();
    let teacher = crate::teacher::Manager::open(file)?;
    let mut last_cycle = Instant::now() - Duration::from_secs(1);
    let mut last_save = Instant::now();
    let mut dirty = false;
    loop {
        if last_cycle.elapsed() >= Duration::from_secs(1) {
            let (count, mut problem) = match drain(file, &living) {
                Ok(count) => (count, None),
                Err(error) => (
                    pending(file).map_or(0, |files| files.len()),
                    Some(error.to_string()),
                ),
            };
            if dirty && last_save.elapsed() >= Duration::from_secs(5) {
                match living.checkpoint() {
                    Ok(()) => {
                        dirty = false;
                        last_save = Instant::now();
                    }
                    Err(error) => problem = Some(error.to_string()),
                }
            }
            dirty |= living.cycle(count, problem);
            last_cycle = Instant::now();
            // The brain sleeps when nobody has used it for a while: it
            // replays a memory, and now and then tests a recall and prunes.
            if brain_used.elapsed() >= NEURAL_QUIET && brain_slept.elapsed() >= NEURAL_SLEEP {
                brain.sleep();
                brain_slept = Instant::now();
                brain_dirty = true;
            }
            if brain_dirty && brain_saved.elapsed() >= Duration::from_secs(60) {
                if let Err(error) = brain.save(&brain_file) {
                    eprintln!("warning: cannot save {}: {error}", brain_file.display());
                }
                brain_dirty = false;
                brain_saved = Instant::now();
            }
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                stream.set_write_timeout(Some(Duration::from_secs(1)))?;
                let mut line = Vec::new();
                let read = BufReader::new(&stream)
                    .take(MAX_EVENT)
                    .read_until(b'\n', &mut line);
                let query: io::Result<Request> = read.and_then(|_| {
                    if line.last() != Some(&b'\n') {
                        return Err(io::Error::other("Incomplete request"));
                    }
                    serde_json::from_slice(&line).map_err(io::Error::from)
                });
                let stop = matches!(query, Ok(Request::Shutdown));
                if query
                    .as_ref()
                    .is_ok_and(|q| !matches!(q, Request::Snapshot))
                    && let Err(error) = drain(file, &living)
                {
                    let reply = Reply {
                        value: Value::Null,
                        error: Some(error.to_string()),
                    };
                    let _ = serde_json::to_writer(&mut stream, &reply);
                    let _ = stream.write_all(b"\n");
                    continue;
                }
                let response = query.and_then(|query| match query {
                    Request::TeachStart { config } => teacher.start(config),
                    Request::TeachStop => teacher.stop(),
                    Request::TeachStatus { after } => Ok(teacher.status(after)),
                    Request::BrainLesson {
                        id,
                        session,
                        lesson,
                        source,
                    } => {
                        if id.len() > 256 || session.len() > 256 || source.len() > 256 {
                            return Err(io::Error::other("Invalid teaching identifiers"));
                        }
                        living.teach(&id, &session, &lesson, &source)?;
                        Ok(json!({"learned":true,"language":living.snapshot().language}))
                    }
                    Request::BrainAsk {
                        session,
                        question,
                        learn_user,
                    } => serde_json::to_value(living.answer(&session, &question, learn_user)?)
                        .map_err(io::Error::from),
                    Request::BrainFeedback {
                        attempt,
                        correct,
                        feedback,
                        correction,
                        source,
                    } => serde_json::to_value(living.grade(
                        &attempt,
                        correct,
                        &feedback,
                        correction.as_deref(),
                        &source,
                    )?)
                    .map_err(io::Error::from),
                    Request::Hear {
                        session,
                        speaker,
                        text,
                    } => {
                        if session.len() > 256
                            || !matches!(speaker.as_str(), "user" | "assistant")
                            || text.len() > 64 * 1024
                        {
                            return Err(io::Error::other("Invalid conversation line"));
                        }
                        living.hear(&session, &speaker, &text);
                        dirty = true;
                        Ok(json!(true))
                    }
                    Request::NeuralLearn {
                        text,
                        source,
                        correction,
                    } => {
                        if text.len() > 8192 || source.len() > 256 {
                            return Err(io::Error::other("Give a lesson of at most 8,192 bytes"));
                        }
                        brain_used = Instant::now();
                        brain_dirty = true;
                        let id = if correction {
                            brain.correct(&text, &source)
                        } else {
                            brain.learn(&text, &source)
                        };
                        Ok(json!({"engram": id, "progress": brain.progress()}))
                    }
                    Request::NeuralSay { prompt } => {
                        if prompt.len() > 4096 {
                            return Err(io::Error::other("Give a prompt of at most 4,096 bytes"));
                        }
                        brain_used = Instant::now();
                        serde_json::to_value(brain.say(&prompt)).map_err(io::Error::from)
                    }
                    Request::NeuralConverse { text } => {
                        if text.trim().is_empty() || text.len() > 4096 {
                            return Err(io::Error::other("Say something of at most 4,096 bytes"));
                        }
                        brain_used = Instant::now();
                        brain_dirty = true;
                        serde_json::to_value(brain.converse(&text)).map_err(io::Error::from)
                    }
                    Request::NeuralRecall { cue } => {
                        if cue.trim().is_empty() || cue.len() > 4096 {
                            return Err(io::Error::other("Give a cue of at most 4,096 bytes"));
                        }
                        brain_used = Instant::now();
                        brain_dirty = true;
                        serde_json::to_value(brain.recall(&cue)).map_err(io::Error::from)
                    }
                    Request::NeuralSnapshot => {
                        serde_json::to_value(brain.snapshot()).map_err(io::Error::from)
                    }
                    Request::NeuralProgress => Ok(brain.progress()),
                    Request::Snapshot => {
                        let mut snapshot = living.snapshot();
                        snapshot.teaching = teacher.summary();
                        serde_json::to_value(snapshot).map_err(io::Error::from)
                    }
                    Request::Flush | Request::Shutdown => {
                        while drain(file, &living)? > 0 {}
                        living.checkpoint()?;
                        brain.save(&brain_file)?;
                        brain_dirty = false;
                        Ok(json!(true))
                    }
                    Request::Suggestion => Ok(json!(living.suggestion())),
                    Request::Hunch => {
                        dirty = true;
                        Ok(json!(living.hunch()))
                    }
                    Request::Inspect { query } => {
                        Ok(counsel.inspect(&query).unwrap_or(Value::Null))
                    }
                    Request::Language { query } => Ok(living.language(&query)),
                    Request::Consolidate { milliseconds } => Ok(json!(
                        living.sleep(Duration::from_millis(milliseconds.min(500)))
                    )),
                });
                let success = response.is_ok();
                let reply = match response {
                    Ok(value) => Reply { value, error: None },
                    Err(error) => Reply {
                        value: Value::Null,
                        error: Some(error.to_string()),
                    },
                };
                if let Ok(mut bytes) = serde_json::to_vec(&reply) {
                    bytes.push(b'\n');
                    let _ = stream.write_all(&bytes);
                }
                if stop && success {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(error) => return Err(error),
        }
    }
    fs::remove_file(path)?;
    teacher.cancel_runtime();
    drop(lock);
    Ok(())
}

pub fn serve_argument() -> Option<io::Result<()>> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.to_str()? != "--living-service" {
        return None;
    }
    Some(
        args.next()
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("Missing network path"))
            .and_then(|file| run(&file)),
    )
}

#[cfg(target_os = "macos")]
fn domain() -> io::Result<String> {
    let output = Command::new("/usr/bin/id").arg("-u").output()?;
    if !output.status.success() {
        return Err(io::Error::other("Cannot locate user login domain"));
    }
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

#[cfg(target_os = "macos")]
pub fn install(executable: &Path, file: &Path) -> io::Result<PathBuf> {
    let home =
        std::env::home_dir().ok_or_else(|| io::Error::other("Cannot locate home directory"))?;
    let bin = home.join(".cortex/bin");
    private_directory(&bin)?;
    private_directory(directory(file))?;
    let agents = home.join("Library/LaunchAgents");
    fs::create_dir_all(&agents)?;
    let target = bin.join("cortex-living-service");
    if executable != target {
        let mut copy = tempfile::NamedTempFile::new_in(&bin)?;
        io::copy(&mut File::open(executable)?, &mut copy)?;
        copy.as_file().sync_all()?;
        fs::set_permissions(copy.path(), fs::Permissions::from_mode(0o700))?;
        copy.persist(&target).map_err(|e| e.error)?;
    }
    let domain = domain()?;
    let _ = request(file, &Request::Flush);
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .output();
    let _ = request(file, &Request::Shutdown);
    let xml = |path: &Path| {
        path.to_string_lossy()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let log = directory(file).join("service.log");
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array><string>{}</string><string>--living-service</string><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>ProcessType</key><string>Background</string><key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string></dict></plist>\n",
        xml(&target),
        xml(file),
        xml(&log),
        xml(&log)
    );
    let path = agents.join(format!("{LABEL}.plist"));
    fs::write(&path, plist)?;
    let output = Command::new("/bin/launchctl")
        .arg("bootstrap")
        .arg(domain)
        .arg(&path)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    wait_ready(file)?;
    Ok(path)
}

#[cfg(target_os = "macos")]
pub fn stop(file: &Path) -> io::Result<()> {
    let _ = request(file, &Request::Flush);
    let output = Command::new("/bin/launchctl")
        .args(["bootout", &format!("{}/{LABEL}", domain()?)])
        .output()?;
    match request(file, &Request::Shutdown) {
        Ok(_) => Ok(()),
        Err(error)
            if output.status.success()
                || matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::TrajectorySink;

    #[test]
    fn service_retains_sessions_replays_without_clients_and_restarts() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("network.json");
        let server_file = file.clone();
        let server = std::thread::spawn(move || run(&server_file));
        for _ in 0..100 {
            if request(&file, &Request::Snapshot).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let remote = Living::connect(&file).unwrap();
        remote.record(&Event::RunStarted {
            instruction: "a robin is a bird. a bird is an animal.".into(),
            provider: "test".into(),
            model: "test".into(),
        });
        remote.checkpoint().unwrap();
        assert_eq!(
            request(
                &file,
                &Request::Language {
                    query: json!({"subject":"robin","category":"animal"})
                }
            )
            .unwrap()["inference"]["supported"],
            true
        );
        drop(remote);
        std::thread::sleep(Duration::from_millis(4200));
        let snapshot: Snapshot =
            serde_json::from_value(request(&file, &Request::Snapshot).unwrap()).unwrap();
        assert!(snapshot.background.running);
        assert!(snapshot.background.cycles >= 3);
        assert!(snapshot.language.replays > 0);
        assert_eq!(snapshot.language.messages, 1);
        assert!(run(&file).is_err(), "second writer must be rejected");
        request(&file, &Request::Shutdown).unwrap();
        server.join().unwrap().unwrap();
        let restored = Living::open(Some(file));
        assert_eq!(restored.snapshot().language.messages, 1);
        assert_eq!(
            restored.snapshot().language.replays,
            snapshot.language.replays
        );
        assert!(!restored.snapshot().background.running);
    }

    #[test]
    fn corrupt_checkpoint_is_preserved_and_duplicate_deliveries_are_idempotent() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("network.json");
        fs::write(&file, "broken").unwrap();
        assert!(run(&file).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "broken");
        let living = Living::open(None);
        let event = Event::RunStarted {
            instruction: "hello world".into(),
            provider: "test".into(),
            model: "test".into(),
        };
        living.deliver("one", "default", &event);
        living.deliver("one", "default", &event);
        assert_eq!(living.snapshot().language.messages, 1);
    }

    #[test]
    fn archived_dialogue_is_imported_once_and_bad_inbox_records_do_not_block_it() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("network.json");
        private_directory(&inbox(&file)).unwrap();
        private_directory(&archive(&file)).unwrap();
        let event = Event::RunStarted {
            instruction: "remember words and patterns".into(),
            provider: "test".into(),
            model: "test".into(),
        };
        let delivery = Delivery {
            id: "history-one".into(),
            session: "one".into(),
            event,
        };
        assert!(enqueue(&file, &delivery).unwrap());
        fs::write(inbox(&file).join("000-broken.json"), "broken").unwrap();
        let living = Living::open(Some(file.clone()));
        assert_eq!(drain(&file, &living).unwrap(), 0);
        assert!(inbox(&file).join("000-broken.rejected").exists());
        assert!(archive(&file).join("history-one.json").exists());
        assert!(!enqueue(&file, &delivery).unwrap());
        assert_eq!(living.snapshot().language.messages, 1);
        assert_eq!(Living::open(Some(file)).snapshot().language.messages, 1);
    }

    #[test]
    fn large_snapshots_are_delivered_completely() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("network.json");
        let living = Living::open(Some(file.clone()));
        for batch in 0..14 {
            let instruction = (0..72)
                .map(|word| format!("concept{}", batch * 72 + word))
                .collect::<Vec<_>>()
                .join(" ");
            living.deliver(
                &format!("history-{batch}"),
                "large",
                &Event::RunStarted {
                    instruction,
                    provider: "test".into(),
                    model: "test".into(),
                },
            );
        }
        living.checkpoint().unwrap();
        let expected = living.snapshot().neurons.len();
        assert!(expected > 1000);
        let server_file = file.clone();
        let server = std::thread::spawn(move || run(&server_file));
        let mut snapshot = None;
        for _ in 0..100 {
            if let Ok(value) = request(&file, &Request::Snapshot) {
                snapshot = Some(value);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let snapshot: Snapshot =
            serde_json::from_value(snapshot.expect("large snapshot did not arrive")).unwrap();
        assert_eq!(snapshot.neurons.len(), expected);
        assert_eq!(snapshot.language.vocabulary, 1008);
        request(&file, &Request::Shutdown).unwrap();
        server.join().unwrap().unwrap();
    }
}

//! The brain's home between uses: a background process that holds the
//! one brain, answers requests on a socket, lives the events agents leave
//! in its inbox, and, when nobody is using it, lets its mind wander and
//! sleeps.
//!
//! Events are written to the inbox as files, so that none is lost while
//! the service is down; each is moved to the archive once the brain that
//! lived it has been saved.

pub mod teacher;
pub mod training;

use std::collections::HashSet;
use std::ffi::OsString;
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

use cortex_core::Event;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::brain::{Brain, NeuralSnapshot};
use crate::protocol;
pub use crate::protocol::Request;

const MAX_EVENT: u64 = 512 * 1024;
const MAX_REPLY: u64 = 64 * 1024 * 1024;
const LABEL: &str = "dev.cortex.brain";

/// The agent that ran the second network this service replaced.
#[cfg(target_os = "macos")]
const RETIRED_LABEL: &str = "dev.cortex.living-network";

/// The flag the service runs under, in any binary that can be it.
const FLAG: &str = "--brain-service";

/// Nobody must have used the brain for this long before its mind runs on
/// its own, and then it has a thought or sleeps a cycle this often.
const QUIET: Duration = Duration::from_secs(5);
const IDLE: Duration = Duration::from_secs(3);

/// How often a changed brain is saved, and while it trains from a file,
/// when saving a large brain often would slow training down.
const SAVED_EVERY: Duration = Duration::from_secs(60);
const SAVED_TRAINING: Duration = Duration::from_secs(600);

/// How long training learns before the service looks for requests again.
const TRAINING_SLICE: Duration = Duration::from_millis(200);

/// Events lived from the inbox at a time.
const DRAINED: usize = 32;

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

/// Where the brain is kept.
/// The note left beside a brain's checkpoint when the brain is retired
/// in favour of another. While it is there the service is not started.
pub const RETIREMENT: &str = "legacy-retired.json";

pub fn default_file() -> io::Result<PathBuf> {
    Ok(home()?.join(".cortex/brain/brain.bin"))
}

fn home() -> io::Result<PathBuf> {
    std::env::home_dir().ok_or_else(|| io::Error::other("Cannot locate home directory"))
}

/// A brain kept where the second network's service kept it is moved
/// here, mind and all, the first time.
fn adopt(file: &Path) -> io::Result<()> {
    if file.exists() || default_file().ok().as_deref() != Some(file) {
        return Ok(());
    }
    let old = home()?.join(".cortex/living/brain.bin");
    if !old.is_file() {
        return Ok(());
    }
    fs::copy(&old, file)?;
    let mind = crate::brain::mind_file(&old);
    if mind.is_file() {
        fs::copy(&mind, crate::brain::mind_file(file))?;
    }
    let teaching = old.with_file_name("network.teaching.json");
    if teaching.is_file() {
        fs::copy(&teaching, file.with_extension("teaching.json"))?;
    }
    eprintln!("adopted the brain at {}", old.display());
    Ok(())
}

/// The saved brain, or a new one. One that cannot be read is moved aside,
/// not overwritten.
fn open_brain(file: &Path) -> Brain {
    let mut brain = if !file.exists() {
        Brain::new()
    } else {
        match Brain::load(file) {
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
                Brain::new()
            }
        }
    };
    brain.mean(&crate::senses::WORTH);
    brain
}

pub fn request(file: &Path, request: &Request) -> io::Result<Value> {
    let mut stream = UnixStream::connect(socket(file))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
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

/// Starts the service for `file` if it is not running.
pub fn ensure(file: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    if default_file().is_ok_and(|default| file == default)
        && let Ok(home) = home()
        && home
            .join(format!("Library/LaunchAgents/{RETIRED_LABEL}.plist"))
            .is_file()
    {
        retire(&domain()?);
    }
    if request(file, &Request::Status).is_ok() {
        return Ok(());
    }
    // A retired brain stays retired: starting it again would begin an
    // empty one in place of the checkpoint that was moved to a backup.
    if directory(file).join(RETIREMENT).is_file() {
        return Err(io::Error::other(format!(
            "This brain was retired; see {}",
            directory(file).join(RETIREMENT).display()
        )));
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
    let installed = std::env::home_dir().map(|p| p.join(".cortex/bin/cortex-brain-service"));
    let executable = installed
        .filter(|p| p.is_file())
        .unwrap_or(std::env::current_exe()?);
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory(file).join("service.log"))?;
    Command::new(executable)
        .arg(FLAG)
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
        if request(file, &Request::Status).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::other(format!(
        "The brain service did not start; see {}",
        directory(file).join("service.log").display()
    )))
}

/// A process's way to the brain in the service: requests, events left in
/// the inbox, and the brain's state fetched every second for drawing.
pub struct Client {
    file: PathBuf,
    cached: Mutex<NeuralSnapshot>,
    status: Mutex<Value>,
    problem: Mutex<Option<String>>,
}

impl Client {
    pub fn connect(file: &Path) -> io::Result<Arc<Self>> {
        ensure(file)?;
        private_directory(&inbox(file))?;
        let snapshot = serde_json::from_value(request(file, &Request::Snapshot)?)?;
        let client = Arc::new(Self {
            file: file.to_path_buf(),
            cached: Mutex::new(snapshot),
            status: Mutex::new(Value::Null),
            problem: Mutex::new(None),
        });
        let weak = Arc::downgrade(&client);
        std::thread::Builder::new()
            .name("brain-snapshot".into())
            .spawn(move || {
                let mut last_retry = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let Some(client) = weak.upgrade() else {
                        break;
                    };
                    let fetched = client.request(&Request::Snapshot).and_then(|v| {
                        serde_json::from_value::<NeuralSnapshot>(v).map_err(io::Error::from)
                    });
                    match fetched {
                        Ok(snapshot) => {
                            *client.cached.lock().unwrap_or_else(|p| p.into_inner()) = snapshot;
                            if let Ok(status) = client.request(&Request::Status) {
                                *client.problem.lock().unwrap_or_else(|p| p.into_inner()) =
                                    status["problem"].as_str().map(str::to_string);
                                *client.status.lock().unwrap_or_else(|p| p.into_inner()) = status;
                            }
                        }
                        Err(error) => {
                            *client.problem.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(error.to_string());
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

    /// The brain as last fetched.
    pub fn snapshot(&self) -> NeuralSnapshot {
        self.cached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The service's status as last fetched: the teacher, events waiting.
    pub fn status(&self) -> Value {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn problem(&self) -> Option<String> {
        self.problem
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn stamp(&self) -> (u64, u64, bool) {
        let snapshot = self.cached.lock().unwrap_or_else(|p| p.into_inner());
        (
            snapshot.clock,
            snapshot
                .mind
                .thoughts
                .last()
                .map_or(0, |t| t.number)
                .wrapping_add(snapshot.stats.replays),
            snapshot.mind.felt.asleep,
        )
    }

    pub fn request(&self, query: &Request) -> io::Result<Value> {
        request(&self.file, query)
    }

    /// Leaves an event in the inbox for the brain to live.
    pub fn record(&self, session: &str, event: &Event) {
        if let Err(error) = self.enqueue(session, event) {
            *self.problem.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(format!("Could not keep an event for the brain: {error}"));
            eprintln!("warning: could not keep an event for the brain: {error}");
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
            id,
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

/// Leaves what was said in saved trajectories in the inbox, for the brain
/// to hear. Each line is left once, however often it is imported.
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

/// What the service has lived from the inbox and not yet saved: moved to
/// the archive only once the brain is saved, so that a crash in between
/// lives them again rather than losing them.
#[derive(Default)]
struct Unsaved {
    lived: Vec<PathBuf>,
    names: HashSet<OsString>,
}

impl Unsaved {
    /// Moves what was lived to the archive; the brain has been saved.
    fn archive(&mut self, file: &Path) -> io::Result<()> {
        for path in self.lived.drain(..) {
            if let Some(name) = path.file_name() {
                fs::rename(&path, archive(file).join(name))?;
            }
        }
        self.names.clear();
        Ok(())
    }
}

/// Lives up to [`DRAINED`] events from the inbox. Returns how many more
/// are waiting.
fn drain(file: &Path, brain: &mut Brain, unsaved: &mut Unsaved) -> io::Result<usize> {
    let files: Vec<PathBuf> = pending(file)?
        .into_iter()
        .filter(|path| path.file_name().is_none_or(|n| !unsaved.names.contains(n)))
        .collect();
    for path in files.iter().take(DRAINED) {
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
            eprintln!("warning: kept aside oversized event {}", path.display());
            continue;
        }
        let delivery: Delivery = match serde_json::from_slice(&bytes) {
            Ok(delivery) => delivery,
            Err(error) => {
                fs::rename(path, path.with_extension("rejected"))?;
                eprintln!(
                    "warning: kept aside malformed event {} for inspection: {error}",
                    path.display()
                );
                continue;
            }
        };
        if path.file_stem().and_then(|s| s.to_str()) != Some(&delivery.id) {
            fs::rename(path, path.with_extension("rejected"))?;
            eprintln!(
                "warning: kept aside event with inconsistent identity {}",
                path.display()
            );
            continue;
        }
        // Imported history is only heard; a live run is lived.
        if delivery.id.starts_with("history-") {
            protocol::heard(brain, &delivery.event);
        } else {
            protocol::lived(brain, &delivery.session, &delivery.event);
        }
        unsaved.names.insert(name.to_os_string());
        unsaved.lived.push(path.clone());
    }
    Ok(files.len().saturating_sub(DRAINED))
}

/// Saves the brain, how far its training has come, and archives what it
/// lived.
fn save(
    file: &Path,
    brain: &Brain,
    unsaved: &mut Unsaved,
    training: &training::Training,
) -> io::Result<()> {
    brain.save(file)?;
    training.persist()?;
    unsaved.archive(file)
}

/// Runs the service for the brain kept in `file` until it is asked to
/// shut down.
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
        io::Error::other(format!("A brain service already owns this brain: {error}"))
    })?;
    adopt(file)?;
    let path = socket(file);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let mut brain = open_brain(file);
    let teacher = teacher::Manager::open(file)?;
    let mut training = training::Training::open(file);
    let mut unsaved = Unsaved::default();
    let mut dirty = false;
    let mut saved = Instant::now();
    let mut used = Instant::now();
    let mut idled = Instant::now();
    let mut last_cycle = Instant::now() - Duration::from_secs(1);
    let mut waiting = 0;
    let mut problem: Option<String> = None;
    loop {
        if last_cycle.elapsed() >= Duration::from_secs(1) {
            last_cycle = Instant::now();
            match drain(file, &mut brain, &mut unsaved) {
                Ok(more) => {
                    waiting = more;
                    if !unsaved.lived.is_empty() {
                        dirty = true;
                        used = Instant::now();
                    }
                }
                Err(error) => problem = Some(error.to_string()),
            }
            // Left alone, its mind wanders, and once drowsy it sleeps:
            // replays memories, prunes, and dreams.
            if used.elapsed() >= QUIET && idled.elapsed() >= IDLE {
                brain.idle();
                idled = Instant::now();
                dirty = true;
            }
            let every = if training.active() {
                SAVED_TRAINING
            } else {
                SAVED_EVERY
            };
            if dirty && saved.elapsed() >= every {
                match save(file, &brain, &mut unsaved, &training) {
                    Ok(()) => problem = None,
                    Err(error) => {
                        eprintln!("warning: cannot save {}: {error}", file.display());
                        problem = Some(error.to_string());
                    }
                }
                dirty = false;
                saved = Instant::now();
            }
        }
        if training.active() {
            let ended = training.step(&mut brain, TRAINING_SLICE);
            dirty = true;
            used = Instant::now();
            if ended {
                match save(file, &brain, &mut unsaved, &training) {
                    Ok(()) => problem = None,
                    Err(error) => problem = Some(error.to_string()),
                }
                dirty = false;
                saved = Instant::now();
            }
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                stream.set_write_timeout(Some(Duration::from_secs(5)))?;
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
                let response = query.and_then(|query| {
                    if protocol::changes(&query) {
                        used = Instant::now();
                        dirty = true;
                    }
                    match query {
                        Request::TeachStart { config } => teacher.start(config),
                        Request::TeachStop => teacher.stop(),
                        Request::TeachStatus { after } => Ok(teacher.status(after)),
                        Request::TrainStart {
                            source,
                            sleep_every,
                            restart,
                        } => training
                            .start(&source, sleep_every, restart)
                            .map(|_| training.status(&brain)),
                        Request::TrainStop => {
                            training.stop();
                            save(file, &brain, &mut unsaved, &training)?;
                            dirty = false;
                            saved = Instant::now();
                            Ok(training.status(&brain))
                        }
                        Request::TrainStatus => Ok(training.status(&brain)),
                        Request::Status => Ok(json!({
                            "protocol_version": 2,
                            "checkpoint_version": 2,
                            "running": true,
                            "teaching": teacher.summary(),
                            "training": training.summary(),
                            "waiting": waiting,
                            "problem": problem,
                        })),
                        Request::Flush | Request::Shutdown => {
                            while drain(file, &mut brain, &mut unsaved)? > 0 {}
                            save(file, &brain, &mut unsaved, &training)?;
                            dirty = false;
                            saved = Instant::now();
                            Ok(json!(true))
                        }
                        other => protocol::respond(&mut brain, &other),
                    }
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
                if !training.active() {
                    std::thread::sleep(Duration::from_millis(20))
                }
            }
            Err(error) => return Err(error),
        }
    }
    fs::remove_file(path)?;
    teacher.cancel_runtime();
    drop(lock);
    Ok(())
}

/// Runs the service when this process was started as it.
pub fn serve_argument() -> Option<io::Result<()>> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.to_str()? != FLAG {
        return None;
    }
    Some(
        args.next()
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("Missing brain path"))
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

/// Stops the second network's agent for good, if it is still installed.
/// What it learned is left where it was.
#[cfg(target_os = "macos")]
fn retire(domain: &str) {
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &format!("{domain}/{RETIRED_LABEL}")])
        .output();
    if let Ok(home) = home() {
        let _ = fs::remove_file(home.join(format!("Library/LaunchAgents/{RETIRED_LABEL}.plist")));
        let _ = fs::remove_file(home.join(".cortex/bin/cortex-living-service"));
    }
}

/// Installs the service as a login agent that keeps the brain running.
#[cfg(target_os = "macos")]
pub fn install(executable: &Path, file: &Path) -> io::Result<PathBuf> {
    let home = home()?;
    let bin = home.join(".cortex/bin");
    private_directory(&bin)?;
    private_directory(directory(file))?;
    let agents = home.join("Library/LaunchAgents");
    fs::create_dir_all(&agents)?;
    let target = bin.join("cortex-brain-service");
    if executable != target {
        let mut copy = tempfile::NamedTempFile::new_in(&bin)?;
        io::copy(&mut File::open(executable)?, &mut copy)?;
        copy.as_file().sync_all()?;
        fs::set_permissions(copy.path(), fs::Permissions::from_mode(0o700))?;
        copy.persist(&target).map_err(|e| e.error)?;
    }
    let domain = domain()?;
    retire(&domain);
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
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array><string>{}</string><string>{FLAG}</string><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>ProcessType</key><string>Background</string><key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string></dict></plist>\n",
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
    if default_file().is_ok_and(|default| file != default) {
        return request(file, &Request::Shutdown).map(|_| ());
    }
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
    use crate::Link;
    use cortex_core::TrajectorySink;

    fn serve(file: &Path) -> std::thread::JoinHandle<io::Result<()>> {
        let server_file = file.to_path_buf();
        let server = std::thread::spawn(move || run(&server_file));
        for _ in 0..200 {
            if request(file, &Request::Status).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        server
    }

    #[test]
    fn a_retired_brain_is_not_started_again() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        std::fs::write(folder.path().join(RETIREMENT), "{}").unwrap();

        let error = ensure(&file).unwrap_err();

        assert!(error.to_string().contains("retired"));
        assert!(!file.exists());
        assert!(!folder.path().join("service.log").exists());
    }

    #[test]
    fn conversation_rehearsal_and_grounded_state_survive_service_restart() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        let server = serve(&file);
        let study = request(
            &file,
            &Request::Study {
                text: "force = mass * acceleration".into(),
                source: "human".into(),
                max_passes: 4,
            },
        )
        .unwrap();
        assert!(study["passes"].as_u64().unwrap() >= 1);
        for text in ["I have one apple", "I get three more"] {
            request(
                &file,
                &Request::Chat {
                    session: "test".into(),
                    text: text.into(),
                },
            )
            .unwrap();
        }
        request(&file, &Request::Shutdown).unwrap();
        server.join().unwrap().unwrap();
        let server = serve(&file);
        let reply = request(
            &file,
            &Request::Chat {
                session: "test".into(),
                text: "how many apples do I have?".into(),
            },
        )
        .unwrap();
        assert_eq!(reply["text"], "You have 4 apples.");
        let physics = request(
            &file,
            &Request::Chat {
                session: "test".into(),
                text: "what is force if mass is 2 kg and acceleration is 3 m/s^2?".into(),
            },
        )
        .unwrap();
        assert_eq!(physics["text"], "force = 6 N.");
        request(&file, &Request::Shutdown).unwrap();
        server.join().unwrap().unwrap();
    }

    #[test]
    fn the_service_lives_events_left_for_it_keeps_them_and_restarts() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        let server = serve(&file);
        let remote = Link::connect(&file).unwrap();
        assert!(remote.is_remote());
        remote.record(&Event::RunStarted {
            instruction: "a robin is a bird. a bird is an animal.".into(),
            provider: "test".into(),
            model: "test".into(),
        });
        remote.hear("chat", "user", "the market went up today");
        remote.checkpoint().unwrap();
        let snapshot: NeuralSnapshot =
            serde_json::from_value(request(&file, &Request::Snapshot).unwrap()).unwrap();
        assert!(snapshot.vocabulary > 8, "{}", snapshot.vocabulary);
        assert!(snapshot.mind.streams.contains(&"default".to_string()));
        assert!(fs::read_dir(archive(&file)).unwrap().count() >= 1);
        assert!(run(&file).is_err(), "a second service must be refused");
        drop(remote);
        request(&file, &Request::Shutdown).unwrap();
        server.join().unwrap().unwrap();
        let restored = Link::open(Some(file)).snapshot();
        assert_eq!(restored.vocabulary, snapshot.vocabulary);
        assert_eq!(restored.clock, snapshot.clock);
    }

    #[test]
    fn a_brain_that_cannot_be_read_is_kept_aside_not_overwritten() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        fs::write(&file, "broken").unwrap();
        let brain = open_brain(&file);
        assert_eq!(brain.memory_count(), 0);
        assert!(!file.exists());
        let aside: Vec<_> = fs::read_dir(folder.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("rejected"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(fs::read_to_string(aside[0].path()).unwrap(), "broken");
    }

    #[test]
    fn history_is_imported_once_and_bad_inbox_records_do_not_block_it() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        private_directory(&inbox(&file)).unwrap();
        private_directory(&archive(&file)).unwrap();
        let delivery = Delivery {
            id: "history-one".into(),
            session: "one".into(),
            event: Event::RunStarted {
                instruction: "remember words and patterns".into(),
                provider: "test".into(),
                model: "test".into(),
            },
        };
        assert!(enqueue(&file, &delivery).unwrap());
        fs::write(inbox(&file).join("000-broken.json"), "broken").unwrap();
        let mut brain = Brain::new();
        let mut unsaved = Unsaved::default();
        assert_eq!(drain(&file, &mut brain, &mut unsaved).unwrap(), 0);
        assert!(inbox(&file).join("000-broken.rejected").exists());
        // Lived, but not archived until the brain is saved.
        assert!(!archive(&file).join("history-one.json").exists());
        assert_eq!(drain(&file, &mut brain, &mut unsaved).unwrap(), 0);
        assert_eq!(brain.stats().lessons, 1);
        save(
            &file,
            &brain,
            &mut unsaved,
            &training::Training::open(&file),
        )
        .unwrap();
        assert!(archive(&file).join("history-one.json").exists());
        assert!(!enqueue(&file, &delivery).unwrap());
        assert_eq!(Brain::load(&file).unwrap().stats().lessons, 1);
    }

    #[test]
    fn the_service_trains_from_a_file_while_it_answers_and_keeps_its_place() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        let source = folder.path().join("lessons.txt");
        let lessons: Vec<String> = (0..60)
            .map(|i| {
                format!(
                    "<user> what comes after {i} <brain> {} comes after {i} .",
                    i + 1
                )
            })
            .collect();
        fs::write(&source, lessons.join("\n")).unwrap();
        let server = serve(&file);
        let train = |source: &Path| {
            request(
                &file,
                &Request::TrainStart {
                    source: source.to_path_buf(),
                    sleep_every: Some(20),
                    restart: false,
                },
            )
        };
        assert!(
            request(
                &file,
                &Request::TrainStart {
                    source: "lessons.txt".into(),
                    sleep_every: None,
                    restart: false
                }
            )
            .is_err()
        );
        train(&source).unwrap();
        // It answers while it trains.
        request(&file, &Request::Progress).unwrap();
        let mut status = Value::Null;
        for _ in 0..500 {
            status = request(&file, &Request::TrainStatus).unwrap();
            if status["state"] != "training" {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(status["state"], "finished", "{status}");
        assert_eq!(status["lessons"], 60);
        assert_eq!(status["sleeps"], 3);
        assert_eq!(
            request(&file, &Request::Status).unwrap()["training"]["percent"],
            100.0
        );
        // Finished, it was saved with how far it came.
        let saved: training::Progress =
            serde_json::from_slice(&fs::read(file.with_extension("training.json")).unwrap())
                .unwrap();
        assert_eq!(saved.lessons, 60);
        assert!(Brain::load(&file).unwrap().stats().lessons >= 60);

        // A long file, stopped part way.
        let long = folder.path().join("long.txt");
        let lessons: Vec<String> = (0..3000)
            .map(|i| format!("<story> the number {i} is here ."))
            .collect();
        fs::write(&long, lessons.join("\n")).unwrap();
        train(&long).unwrap();
        let stopped = request(&file, &Request::TrainStop).unwrap();
        assert_eq!(stopped["state"], "stopped");
        let lessons = stopped["lessons"].as_u64().unwrap();
        assert!(lessons > 0 && lessons < 3000, "{stopped}");
        request(&file, &Request::Shutdown).unwrap();
        server.join().unwrap().unwrap();

        // A stopped training goes on where it stopped after a restart.
        let server = serve(&file);
        let resumed = train(&long).unwrap();
        assert_eq!(resumed["lessons"].as_u64().unwrap(), lessons);
        request(&file, &Request::Shutdown).unwrap();
        server.join().unwrap().unwrap();
    }

    #[test]
    fn an_agent_sink_reaches_the_service() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        let server = serve(&file);
        let remote = Link::connect(&file).unwrap();
        let sink = remote.sink();
        sink.record(&Event::RunStarted {
            instruction: "fix the rounding".into(),
            provider: "test".into(),
            model: "test".into(),
        });
        remote.checkpoint().unwrap();
        let snapshot: NeuralSnapshot =
            serde_json::from_value(request(&file, &Request::Snapshot).unwrap()).unwrap();
        assert!(snapshot.mind.streams.contains(&sink.stream().to_string()));
        request(&file, &Request::Shutdown).unwrap();
        server.join().unwrap().unwrap();
    }
}

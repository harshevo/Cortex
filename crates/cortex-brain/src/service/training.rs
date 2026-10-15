//! Training from a file of lessons: the service learns them a slice at a
//! time between requests, so that it keeps answering while it trains,
//! sleeps every so many lessons, as a brain learning all day would, and
//! keeps its place in the file with the brain it saves, so that a restart
//! goes on from there.
//!
//! A lesson is a line: plain text, such as `<user> hi <brain> hello`, or a
//! JSON object with `text`, or with `user` and `assistant` (or `brain`).
//! Empty lines and lines starting with `#` are passed over.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::brain::Brain;
use crate::brain::words::{BRAIN, USER, tokenize};

/// Longest lesson learned whole, as for a lesson taught by request: the
/// rest is cut, so that no lesson keeps the service from answering.
const LONGEST: usize = 8192;

/// Sleep cycles after each stretch of lessons: enough for the cycle that
/// prunes and keeps the brain within its budget to come round.
const CYCLES: u32 = 6;

/// Lessons between sleeps, unless told otherwise.
pub const SLEEP_EVERY: u64 = 200;

/// How far back the pace is measured.
const PACE_WINDOW: f64 = 300.0;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Idle,
    Training,
    Finished,
    Stopped,
    Failed,
}

/// How far training has come. Saved with the brain, so `offset` is always
/// where the saved brain stopped reading.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    pub state: State,
    pub source: PathBuf,
    /// Size of the file when training began; a file that changes size is
    /// not resumed.
    pub size: u64,
    pub offset: u64,
    pub lessons: u64,
    pub words: u64,
    /// Lessons cut to [`LONGEST`] bytes, and lines that were not lessons.
    pub shortened: u64,
    pub skipped: u64,
    pub sleeps: u64,
    pub sleep_every: u64,
    /// Time spent training, across restarts.
    pub seconds: f64,
    pub problem: Option<String>,
}

struct Job {
    reader: BufReader<File>,
    since_sleep: u64,
    /// Seconds trained, words learned and bytes read, sampled now and then.
    pace: VecDeque<(f64, u64, u64)>,
}

pub struct Training {
    file: PathBuf,
    progress: Progress,
    job: Option<Job>,
}

impl Training {
    /// Training for the brain kept in `file`: none, unless it was training
    /// when the service stopped, in which case it goes on.
    pub fn open(file: &Path) -> Self {
        let mut training = Self {
            file: file.to_path_buf(),
            progress: Progress::default(),
            job: None,
        };
        let saved = fs_read(&training.path());
        match saved {
            Ok(Some(progress)) => training.progress = progress,
            Ok(None) => return training,
            Err(error) => {
                training.progress.problem =
                    Some(format!("Saved training progress is unreadable: {error}"));
                return training;
            }
        }
        if training.progress.state == State::Training
            && let Err(error) = training.resume()
        {
            training.fail(error);
        }
        training
    }

    fn path(&self) -> PathBuf {
        self.file.with_extension("training.json")
    }

    pub fn active(&self) -> bool {
        self.job.is_some()
    }

    /// Starts training from `source`, or goes on from where an earlier
    /// training from it stopped, unless `restart`.
    pub fn start(
        &mut self,
        source: &Path,
        sleep_every: Option<u64>,
        restart: bool,
    ) -> io::Result<()> {
        if self.active() {
            return Err(io::Error::other(
                "Training already runs; stop it before starting another",
            ));
        }
        if !source.is_absolute() {
            return Err(io::Error::other(
                "Give the lessons file as an absolute path",
            ));
        }
        let size = std::fs::metadata(source)?.len();
        let resumes = !restart
            && self.progress.source == source
            && self.progress.size == size
            && self.progress.offset < size
            && matches!(self.progress.state, State::Stopped | State::Failed);
        if !resumes {
            self.progress = Progress {
                source: source.to_path_buf(),
                size,
                ..Progress::default()
            };
        }
        self.progress.sleep_every = sleep_every.unwrap_or(SLEEP_EVERY).max(1);
        self.progress.state = State::Training;
        self.progress.problem = None;
        self.resume()
    }

    fn resume(&mut self) -> io::Result<()> {
        let mut file = File::open(&self.progress.source)?;
        let size = file.metadata()?.len();
        if size != self.progress.size {
            return Err(io::Error::other(
                "The lessons file changed since training began; start it again to train from the start",
            ));
        }
        file.seek(SeekFrom::Start(self.progress.offset))?;
        self.job = Some(Job {
            reader: BufReader::with_capacity(1 << 20, file),
            since_sleep: 0,
            pace: VecDeque::new(),
        });
        Ok(())
    }

    /// Stops training; what was learned is kept, and starting it again on
    /// the same file goes on from here.
    pub fn stop(&mut self) {
        if self.job.take().is_some() {
            self.progress.state = State::Stopped;
        }
    }

    fn fail(&mut self, error: io::Error) {
        self.job = None;
        self.progress.state = State::Failed;
        self.progress.problem = Some(error.to_string());
    }

    /// Learns lessons for about `slice`, reading at least a line. Returns whether
    /// training has ended, finished or failed, so the brain is saved now.
    pub fn step(&mut self, brain: &mut Brain, slice: Duration) -> bool {
        let started = Instant::now();
        let ended = match self.learn(brain, started, slice) {
            Ok(ended) => ended,
            Err(error) => {
                self.fail(error);
                true
            }
        };
        self.progress.seconds += started.elapsed().as_secs_f64();
        if let Some(job) = &mut self.job {
            let now = (
                self.progress.seconds,
                self.progress.words,
                self.progress.offset,
            );
            if job.pace.back().is_none_or(|last| now.0 - last.0 >= 5.0) {
                job.pace.push_back(now);
            }
            while job.pace.len() > 2 && now.0 - job.pace[0].0 > PACE_WINDOW {
                job.pace.pop_front();
            }
        }
        ended
    }

    fn learn(&mut self, brain: &mut Brain, started: Instant, slice: Duration) -> io::Result<bool> {
        let source = format!(
            "training: {}",
            self.progress
                .source
                .file_name()
                .map_or("lessons".into(), |n| n.to_string_lossy())
        );
        let source = cap(&source, 256).to_string();
        let mut line = Vec::new();
        loop {
            let Some(job) = &mut self.job else {
                return Ok(true);
            };
            line.clear();
            let read = job.reader.read_until(b'\n', &mut line)?;
            if read == 0 {
                self.job = None;
                self.progress.state = State::Finished;
                return Ok(true);
            }
            self.progress.offset += read as u64;
            let text = String::from_utf8_lossy(&line);
            match lesson(text.trim()) {
                Lesson::Nothing => {}
                Lesson::Unreadable => self.progress.skipped += 1,
                Lesson::Text(text) => {
                    let text = if text.len() > LONGEST {
                        self.progress.shortened += 1;
                        shorten(&text, LONGEST)
                    } else {
                        &text
                    };
                    self.progress.words += tokenize(text).len() as u64;
                    brain.learn(text, &source);
                    self.progress.lessons += 1;
                    job.since_sleep += 1;
                }
            }
            if job.since_sleep >= self.progress.sleep_every {
                job.since_sleep = 0;
                for _ in 0..CYCLES {
                    brain.sleep();
                }
                self.progress.sleeps += 1;
            }
            if started.elapsed() >= slice {
                return Ok(false);
            }
        }
    }

    /// Writes how far training has come. Called as the brain is saved, so
    /// the place in the file matches what the saved brain has learned.
    pub fn persist(&self) -> io::Result<()> {
        if self.progress.state == State::Idle {
            return Ok(());
        }
        let path = self.path();
        let folder = path
            .parent()
            .ok_or_else(|| io::Error::other("Missing training progress directory"))?;
        let mut temporary = tempfile::NamedTempFile::new_in(folder)?;
        temporary.write_all(&serde_json::to_vec(&self.progress)?)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|e| e.error)?;
        Ok(())
    }

    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    /// How training is going, with its recent pace and, at that pace, the
    /// time left (more in truth: each word costs more as the brain grows).
    pub fn status(&self, brain: &Brain) -> Value {
        let p = &self.progress;
        let pace = self.job.as_ref().and_then(|job| {
            let (first, last) = (job.pace.front()?, job.pace.back()?);
            let seconds = last.0 - first.0;
            (seconds > 0.0).then(|| {
                (
                    (last.1 - first.1) as f64 / seconds,
                    (last.2 - first.2) as f64 / seconds,
                )
            })
        });
        let left = pace
            .filter(|(_, bytes)| *bytes > 0.0)
            .map(|(_, bytes)| (p.size.saturating_sub(p.offset)) as f64 / bytes);
        let progress = brain.progress();
        json!({
            "state": p.state,
            "source": p.source,
            "percent": if p.size > 0 { (p.offset as f64 * 1000.0 / p.size as f64).round() / 10.0 } else { 0.0 },
            "lessons": p.lessons,
            "words": p.words,
            "shortened": p.shortened,
            "skipped": p.skipped,
            "sleeps": p.sleeps,
            "sleep_every": p.sleep_every,
            "seconds": p.seconds.round(),
            "words_per_second": pace.map(|(words, _)| words.round()),
            "seconds_left_at_this_pace": left.map(f64::round),
            "footprint": brain.footprint(),
            "budget": brain.budget(),
            "next_word_right_recently": progress["next_word_right_recently"],
            "problem": p.problem,
        })
    }

    /// What the service's status shows of training.
    pub fn summary(&self) -> Value {
        let p = &self.progress;
        json!({
            "state": p.state,
            "source": p.source,
            "lessons": p.lessons,
            "percent": if p.size > 0 { (p.offset as f64 * 1000.0 / p.size as f64).round() / 10.0 } else { 0.0 },
            "problem": p.problem,
        })
    }
}

fn fs_read(path: &Path) -> io::Result<Option<Progress>> {
    match File::open(path) {
        Ok(file) => Ok(Some(serde_json::from_reader(BufReader::new(file))?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

enum Lesson {
    Nothing,
    Unreadable,
    Text(String),
}

fn lesson(line: &str) -> Lesson {
    if line.is_empty() || line.starts_with('#') {
        return Lesson::Nothing;
    }
    if !line.starts_with('{') {
        return Lesson::Text(line.to_string());
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Lesson::Unreadable;
    };
    let field = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| value[*name].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    if let Some(text) = field(&["text"]) {
        return Lesson::Text(text.to_string());
    }
    match (field(&["user"]), field(&["assistant", "brain"])) {
        (Some(user), Some(reply)) => Lesson::Text(format!("{USER} {user} {BRAIN} {reply}")),
        _ => Lesson::Unreadable,
    }
}

/// `text` cut to at most `most` bytes, at the last space before it.
fn shorten(text: &str, most: usize) -> &str {
    let cut = cap(text, most);
    cut.rfind(char::is_whitespace).map_or(cut, |at| &cut[..at])
}

fn cap(text: &str, most: usize) -> &str {
    let mut end = most.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lessons(folder: &Path, lines: &[String]) -> PathBuf {
        let path = folder.join("lessons.txt");
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    #[test]
    fn lessons_are_read_as_text_or_json_and_the_rest_passed_over() {
        assert!(matches!(lesson(""), Lesson::Nothing));
        assert!(matches!(lesson("# a note"), Lesson::Nothing));
        assert!(matches!(lesson("{broken"), Lesson::Unreadable));
        assert!(matches!(lesson(r#"{"other": 1}"#), Lesson::Unreadable));
        let Lesson::Text(text) = lesson(r#"{"user": "hi", "assistant": "hello"}"#) else {
            panic!()
        };
        assert_eq!(text, "<user> hi <brain> hello");
        let Lesson::Text(text) = lesson(r#"{"text": "<story> a cat ."}"#) else {
            panic!()
        };
        assert_eq!(text, "<story> a cat .");
        assert_eq!(shorten("one two three", 9), "one two");
        assert_eq!(cap("héllo", 2), "h");
    }

    #[test]
    fn training_learns_a_file_sleeps_and_goes_on_from_where_it_was_saved() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        let mut lines: Vec<String> = (0..40)
            .map(|i| format!("<user> what is {i} plus one <brain> it is {} .", i + 1))
            .collect();
        lines.insert(3, String::new());
        lines.insert(5, "{broken".into());
        lines.push(format!("<story> {}", "word ".repeat(3000)));
        let source = lessons(folder.path(), &lines);

        let mut brain = Brain::new();
        let mut training = Training::open(&file);
        assert!(
            training
                .start(Path::new("relative.txt"), None, false)
                .is_err()
        );
        training.start(&source, Some(10), false).unwrap();
        // A slice reads at least one line, however short.
        assert!(!training.step(&mut brain, Duration::ZERO));
        assert_eq!(training.progress().lessons, 1);
        while training.progress().lessons < 20 {
            training.step(&mut brain, Duration::ZERO);
        }
        brain.save(&file).unwrap();
        training.persist().unwrap();
        let saved_at = training.progress().offset;

        // Lessons learned after the save are learned again after a restart.
        training.step(&mut brain, Duration::ZERO);
        let mut brain = Brain::load(&file).unwrap();
        let mut training = Training::open(&file);
        assert!(training.active());
        assert_eq!(training.progress().offset, saved_at);
        assert_eq!(training.progress().lessons, 20);
        while !training.step(&mut brain, Duration::from_millis(50)) {}

        let p = training.progress();
        assert_eq!(p.state, State::Finished);
        assert_eq!(p.lessons, 41);
        assert_eq!((p.skipped, p.shortened), (1, 1));
        assert_eq!(p.sleeps, 4);
        assert_eq!(p.offset, std::fs::metadata(&source).unwrap().len());
        assert!(brain.stats().lessons >= 41);
        assert!(brain.stats().sleeps >= 4 * CYCLES as u64);
        assert_eq!(training.status(&brain)["state"], "finished");
    }

    #[test]
    fn a_stopped_training_goes_on_unless_restarted_or_its_file_changed() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("brain.bin");
        let lines: Vec<String> = (0..10)
            .map(|i| format!("<story> the number {i} ."))
            .collect();
        let source = lessons(folder.path(), &lines);
        let mut brain = Brain::new();
        let mut training = Training::open(&file);
        training.start(&source, None, false).unwrap();
        training.step(&mut brain, Duration::ZERO);
        training.step(&mut brain, Duration::ZERO);
        training.stop();
        assert_eq!(training.progress().state, State::Stopped);
        assert!(training.start(&source, None, false).is_ok());
        assert_eq!(training.progress().lessons, 2);
        assert!(
            training.start(&source, None, true).is_err(),
            "one runs already"
        );
        training.stop();
        training.start(&source, None, true).unwrap();
        assert_eq!(training.progress().lessons, 0);

        training.step(&mut brain, Duration::ZERO);
        training.persist().unwrap();
        std::fs::write(&source, "<story> something else entirely .").unwrap();
        let reopened = Training::open(&file);
        assert!(!reopened.active());
        assert_eq!(reopened.progress().state, State::Failed);
        assert!(
            reopened
                .progress()
                .problem
                .as_deref()
                .unwrap()
                .contains("changed")
        );
    }
}

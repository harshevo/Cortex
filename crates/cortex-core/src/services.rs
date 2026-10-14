//! Where the Python services are, and how one is started.
//!
//! Three parts of Cortex are Python programs that answer on a local port:
//! the embedding model, the HOPE memory model and the mind. A packaged app
//! carries all of them in a `services` folder next to its program:
//!
//! ```text
//! services/
//!   python/    an interpreter of its own, with every library installed
//!   ml/        `cortex_ml` and the trained HOPE weights
//!   brain/     the `cortex.brain` package the mind is
//!   models/    the embedding model's weights
//! ```
//!
//! Nothing in there is written to while the app runs, so the folder can
//! sit somewhere read-only and be moved with the app to another machine.
//!
//! Run from a checkout, there is no such folder. Then the services are
//! started from `ml/` and from the `brain` project beside the checkout,
//! with the Python environment those keep.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Names another `services` folder than the one beside the program.
pub const SERVICES_VARIABLE: &str = "CORTEX_SERVICES";

/// Names the interpreter to use instead of the one that would be found.
pub const PYTHON_VARIABLE: &str = "CORTEX_PYTHON";

/// The folder the embedding model's weights are in, inside `models`.
const EMBEDDING_MODEL: &str = "embeddinggemma-2";

/// How far up from the program a checkout is looked for. A test binary
/// sits five folders below the folder that holds `cortex` and `brain`.
const CHECKOUT_DEPTH: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Embedding,
    Hope,
    Mind,
}

impl Service {
    fn module(self) -> &'static str {
        match self {
            Self::Embedding => "cortex_ml.embedding.server",
            Self::Hope => "cortex_ml.hope.server",
            Self::Mind => "cortex.brain.server",
        }
    }

    /// The file whose presence says a folder holds this service.
    fn mark(self) -> &'static str {
        match self {
            Self::Embedding => "cortex_ml/embedding/server.py",
            Self::Hope => "cortex_ml/hope/server.py",
            Self::Mind => "cortex/brain/server.py",
        }
    }

    /// Where what the service prints is kept.
    pub fn log(self) -> Option<PathBuf> {
        let home = std::env::home_dir()?.join(".cortex");

        Some(match self {
            Self::Embedding => home.join("logs/embedding-service.log"),
            Self::Hope => home.join("logs/hope-service.log"),
            Self::Mind => home.join("mind/server.log"),
        })
    }

    /// The folder the service is run from: the one named in the
    /// environment, else the packaged one, else the checkout's.
    pub fn folder(self) -> Result<PathBuf, String> {
        let (variable, packaged_name, checkout_name) = match self {
            Self::Embedding | Self::Hope => ("CORTEX_ML_DIR", "ml", "ml"),
            Self::Mind => ("CORTEX_BRAIN_DIR", "brain", "brain"),
        };

        let named = std::env::var_os(variable).map(PathBuf::from);

        let shipped = packaged().map(|services| services.join(packaged_name));

        // Where the mind was kept before the app carried it.
        let installed = (self == Self::Mind)
            .then(std::env::home_dir)
            .flatten()
            .map(|home| home.join(".cortex/brain"));

        let checkout = std::env::current_exe()
            .ok()
            .into_iter()
            .flat_map(|program| {
                program
                    .ancestors()
                    .skip(1)
                    .take(CHECKOUT_DEPTH)
                    .map(|folder| folder.join(checkout_name))
                    .collect::<Vec<_>>()
            });

        named
            .into_iter()
            .chain(shipped)
            .chain(installed)
            .chain(checkout)
            .find(|folder| folder.join(self.mark()).is_file())
            .ok_or_else(|| {
                format!(
                    "the files of `{}` were not found. They belong in the `services` folder \
                     beside the program; set {variable} to the folder that holds them",
                    self.module()
                )
            })
    }

    /// The command that runs the service on `port`. Where its output goes
    /// is left to the caller.
    pub fn command(self, port: u16) -> Result<Command, String> {
        let folder = self.folder()?;
        let packaged = packaged();

        let mut command = self.interpreter(packaged.as_deref(), &folder);

        command
            .current_dir(&folder)
            .env("PYTHONUNBUFFERED", "1")
            .args(["-m", self.module(), "--port", &port.to_string()]);

        match self {
            Self::Embedding => {
                command
                    .env("CORTEX_CPU_THREADS", "2")
                    .env("OMP_NUM_THREADS", "2")
                    .env("TOKENIZERS_PARALLELISM", "false");

                let weights = packaged
                    .as_deref()
                    .map(|services| services.join("models").join(EMBEDDING_MODEL))
                    .filter(|weights| weights.is_dir());

                // With the weights in the package nothing is fetched, so
                // the service starts with no network at all.
                if let Some(weights) = weights {
                    command
                        .env("CORTEX_EMBEDDING_MODEL", weights)
                        .env("HF_HUB_OFFLINE", "1")
                        .env("TRANSFORMERS_OFFLINE", "1");
                }
            }

            Self::Hope => {
                let weights = folder.join("hope.pt");

                if weights.is_file() {
                    command.arg("--weights").arg(weights);
                }
            }

            Self::Mind => {
                // The package is not written to, so what the mind would
                // keep beside its own code goes with its memory instead.
                if let Some(home) = std::env::home_dir().filter(|_| packaged.is_some()) {
                    command.env("CORTEX_RUNS", home.join(".cortex/mind/runs"));
                }
            }
        }

        if packaged.is_some() {
            // The packaged interpreter is complete. Nothing of a Python
            // the person has installed may leak into it, and it must not
            // leave compiled files in the package.
            command
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .env("PYTHONNOUSERSITE", "1")
                .env_remove("PYTHONPATH")
                .env_remove("PYTHONHOME");
        }

        hide_window(&mut command);

        Ok(command)
    }

    fn interpreter(self, packaged: Option<&Path>, folder: &Path) -> Command {
        if let Some(named) = std::env::var_os(PYTHON_VARIABLE).filter(|named| !named.is_empty()) {
            return Command::new(named);
        }

        if let Some(services) = packaged {
            return Command::new(packaged_python(services));
        }

        if self == Self::Mind {
            return Command::new(if cfg!(windows) { "python" } else { "python3" });
        }

        let own = folder.join(if cfg!(windows) {
            ".venv/Scripts/python.exe"
        } else {
            ".venv/bin/python"
        });

        if own.is_file() {
            return Command::new(own);
        }

        // A checkout whose environment was never made: `uv` makes it.
        let uv = [
            Some(PathBuf::from("/opt/homebrew/bin/uv")),
            Some(PathBuf::from("/usr/local/bin/uv")),
            std::env::home_dir().map(|home| home.join(".local/bin/uv")),
        ]
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("uv"));

        let mut command = Command::new(uv);

        command.args(["run", "--project"]).arg(folder).arg("python");

        command
    }

    /// Starts the service on `port` and leaves it running after this
    /// process ends, so the next one finds it already awake. What it
    /// prints goes to [`Service::log`].
    pub fn start_detached(self, port: u16) -> Result<(), String> {
        let mut command = self.command(port)?;

        command.stdin(Stdio::null());

        match self.open_log() {
            Some((out, err)) => command.stdout(out).stderr(err),
            None => command.stdout(Stdio::null()).stderr(Stdio::null()),
        };

        // Its own process group, so that Ctrl-C in the terminal stops the
        // agent and leaves the service running for the next one.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);

        command
            .spawn()
            .map(drop)
            .map_err(|error| format!("cannot run {:?}: {error}", command.get_program()))
    }

    fn open_log(self) -> Option<(std::fs::File, std::fs::File)> {
        let path = self.log()?;

        std::fs::create_dir_all(path.parent()?).ok()?;

        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;

        Some((log.try_clone().ok()?, log))
    }
}

/// The `services` folder this program was packaged with, when it was.
pub fn packaged() -> Option<PathBuf> {
    let named = std::env::var_os(SERVICES_VARIABLE).map(PathBuf::from);

    let program = std::env::current_exe().ok();
    let beside = program.as_deref().and_then(Path::parent);
    let above = beside.and_then(Path::parent);

    let places = [
        named,
        // Windows and Linux: a folder holding the program and `services`.
        beside.map(|folder| folder.join("services")),
        // macOS: `Cortex.app/Contents/MacOS` beside `Contents/Resources`.
        above.map(|folder| folder.join("Resources/services")),
        // Linux, installed under a prefix: `bin` beside `lib/cortex`.
        above.map(|folder| folder.join("lib/cortex/services")),
    ];

    places
        .into_iter()
        .flatten()
        .find(|services| packaged_python(services).is_file())
}

fn packaged_python(services: &Path) -> PathBuf {
    services.join(if cfg!(windows) {
        "python/python.exe"
    } else {
        "python/bin/python3"
    })
}

/// On Windows a console program started by a window app opens a console
/// window of its own unless it is told not to.
pub fn hide_window(command: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        std::os::windows::process::CommandExt::creation_flags(command, CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    /// Run from this checkout, each service is found in its own project.
    #[test]
    fn a_checkout_finds_the_embedding_service_in_ml() {
        let folder = Service::Embedding.folder().unwrap();

        assert!(folder.join("pyproject.toml").is_file());

        let command = Service::Embedding.command(8765).unwrap();
        let arguments = arguments(&command);

        assert!(arguments.ends_with(&[
            "-m".to_string(),
            "cortex_ml.embedding.server".to_string(),
            "--port".to_string(),
            "8765".to_string(),
        ]));
        assert_eq!(command.get_current_dir(), Some(folder.as_path()));
    }

    #[test]
    fn the_hope_service_is_given_its_trained_weights() {
        let folder = Service::Hope.folder().unwrap();

        if !folder.join("hope.pt").is_file() {
            return;
        }

        let arguments = arguments(&Service::Hope.command(8766).unwrap());

        assert!(arguments.contains(&"--weights".to_string()));
        assert!(arguments.last().unwrap().ends_with("hope.pt"));
    }

    #[test]
    fn a_folder_without_an_interpreter_is_not_a_package() {
        let folder = tempfile::tempdir().unwrap();

        assert!(!packaged_python(folder.path()).is_file());
    }
}

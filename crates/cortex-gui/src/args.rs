//! Command-line arguments.
//!
//! The app is set up from its settings page. The `cortex` command's own
//! arguments are accepted too: whatever is given explicitly is taken over
//! into the settings, so a command line that works for `cortex run` also
//! sets up the app.

use std::path::PathBuf;

use clap::Parser;
use cortex_cli::args::SessionArgs;

#[derive(Parser, Clone)]
#[command(name = "cortex-gui", version, about = "The Cortex desktop app.")]
pub struct Args {
    #[command(flatten)]
    pub session: SessionArgs,

    /// Use a stand-in model that replays a fixed task in a throwaway
    /// project. Needs no network and no key; for trying the app and for
    /// its tests. The tools, memory and checks it exercises are real.
    #[arg(long)]
    pub demo: bool,

    /// Open two independent chats side by side.
    #[arg(long)]
    pub split: bool,

    #[command(flatten)]
    pub rehearsal: RehearsalArgs,
}

/// Hooks for driving the app without a person, used to test it.
#[derive(clap::Args, Clone, Default)]
pub struct RehearsalArgs {
    /// Send this instruction as soon as the window opens.
    #[arg(long, hide = true)]
    pub autorun: Option<String>,

    /// Draw in this colour theme, without saving the choice.
    #[arg(long, hide = true)]
    pub theme: Option<String>,

    /// Start with the files of the project shown on the right.
    #[arg(long, hide = true)]
    pub open_files: bool,

    /// Start with this file open in the files panel.
    #[arg(long, hide = true)]
    pub open_file: Option<PathBuf>,

    /// Start on the memory space instead of the conversation.
    #[arg(long, hide = true)]
    pub open_memory: bool,

    /// Start on the living network instead of the conversation.
    #[arg(long, hide = true)]
    pub open_living: bool,

    /// Select the newest episode whenever memory changes.
    #[arg(long, hide = true)]
    pub select_newest: bool,

    /// Start on the settings page.
    #[arg(long, hide = true)]
    pub open_settings: bool,

    /// Press stop this many seconds after the window opens.
    #[arg(long, hide = true)]
    pub stop_after: Option<f32>,

    /// Press "Continue" once, the first time a stopped run offers it.
    #[arg(long, hide = true)]
    pub continue_when_offered: bool,

    /// Say this to the run in progress, `--steer-after` seconds in.
    #[arg(long, hide = true)]
    pub steer: Option<String>,

    #[arg(long, hide = true, default_value_t = 2.0)]
    pub steer_after: f32,

    /// Type on the keyboard, as `<seconds>:<text>`: the text is typed
    /// that many seconds in, wherever the keyboard focus is. A text that
    /// ends in `\n` is followed by the Return key. May be repeated.
    #[arg(long = "type", hide = true)]
    pub typed: Vec<String>,

    /// Attach this picture to the `--autorun` instruction. May be repeated.
    #[arg(long, hide = true)]
    pub attach: Vec<PathBuf>,

    /// Once the first run has finished, open its session from the
    /// sidebar and send this as the next instruction.
    #[arg(long, hide = true)]
    pub reopen_then: Option<String>,

    /// Save a picture of the window here, then quit.
    #[arg(long, hide = true)]
    pub screenshot: Option<PathBuf>,

    /// Seconds to wait before taking the picture.
    #[arg(long, hide = true, default_value_t = 6.0)]
    pub screenshot_after: f32,
}

//! Driving the app without a person, to test it: send an instruction when
//! the window opens, and save a picture of the window after a while.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, ViewportCommand};

use crate::args::RehearsalArgs;

pub struct Rehearsal {
    args: RehearsalArgs,
    started: Instant,
    picture_requested: bool,
    brought_forward: bool,

    /// Frames drawn so far, to report the frame rate.
    frames: u32,
}

impl Rehearsal {
    pub fn new(args: RehearsalArgs) -> Self {
        Self {
            args,
            started: Instant::now(),
            picture_requested: false,
            brought_forward: false,
            frames: 0,
        }
    }

    pub fn opens_files(&self) -> bool {
        self.args.open_files || self.args.open_file.is_some()
    }

    pub fn opens_file(&self) -> Option<PathBuf> {
        self.args.open_file.clone()
    }

    /// Whether a theme was named to try out, which opens the settings
    /// on their appearance.
    pub fn tries_theme(&self) -> bool {
        self.args.theme.is_some()
    }

    pub fn opens_memory(&self) -> bool {
        self.args.open_memory
    }

    pub fn opens_living(&self) -> bool {
        self.args.open_living
    }

    /// The trading tab to start on, if any.
    pub fn opens_trading(&self) -> Option<&str> {
        self.args.open_trading.as_deref()
    }

    pub fn opens_settings(&self) -> bool {
        self.args.open_settings
    }

    pub fn opens_remote(&self) -> bool {
        self.args.open_remote
    }

    pub fn selects_newest(&self) -> bool {
        self.args.select_newest
    }

    /// The instruction to send on opening. Yields it once.
    pub fn take_instruction(&mut self) -> Option<String> {
        self.args.autorun.take()
    }

    /// The pictures to send with that instruction. Yields them once; one
    /// that cannot be read is reported and left out.
    pub fn take_attachments(&mut self) -> Vec<cortex_core::Image> {
        std::mem::take(&mut self.args.attach)
            .iter()
            .filter_map(|path| match crate::attachment::load(path) {
                Ok(attachment) => Some(attachment.image),

                Err(error) => {
                    eprintln!("warning: {error:#}");

                    None
                }
            })
            .collect()
    }

    /// Whether the moment to press stop has come. True once.
    pub fn take_stop(&mut self) -> bool {
        let due = self
            .args
            .stop_after
            .is_some_and(|seconds| self.started.elapsed().as_secs_f32() >= seconds);

        if due {
            self.args.stop_after = None;
        }

        due
    }

    /// What to say to the run in progress, when its moment has come.
    /// Yields it once.
    pub fn take_steer(&mut self) -> Option<String> {
        let due = self.started.elapsed().as_secs_f32() >= self.args.steer_after;

        self.args.steer.take_if(|_| due)
    }

    /// The keys whose moment has come, as the events a keyboard would
    /// have caused. Each `--type` is typed once.
    pub fn take_typed(&mut self) -> Vec<egui::Event> {
        let elapsed = self.started.elapsed().as_secs_f32();

        let mut events = Vec::new();

        self.args.typed.retain(|typed| {
            let (seconds, text) = typed.split_once(':').unwrap_or(("0", typed));

            if seconds.parse::<f32>().unwrap_or(0.0) > elapsed {
                return true;
            }

            let text = text.replace("\\n", "\n");

            if let Some(line) = text.strip_suffix('\n') {
                events.push(egui::Event::Text(line.to_string()));

                events.push(egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            } else {
                events.push(egui::Event::Text(text));
            }

            false
        });

        events
    }

    /// The instruction to send after reopening the session. Yields it
    /// once.
    pub fn take_reopen(&mut self) -> Option<String> {
        self.args.reopen_then.take()
    }

    pub fn reopens(&self) -> bool {
        self.args.reopen_then.is_some()
    }

    /// Whether to press "Continue" now that it is on offer. True once.
    pub fn take_continue(&mut self) -> bool {
        std::mem::take(&mut self.args.continue_when_offered)
    }

    /// Takes the picture when its time comes, then closes the window.
    pub fn advance(&mut self, context: &egui::Context) {
        let Some(path) = self.args.screenshot.clone() else {
            return;
        };

        // A window behind others is not drawn, and a window that is not
        // drawn cannot be pictured.
        if !self.brought_forward {
            self.brought_forward = true;

            context.send_viewport_cmd(ViewportCommand::Focus);
        }

        self.frames += 1;

        let wait = Duration::from_secs_f32(self.args.screenshot_after);

        if !self.picture_requested {
            match wait.checked_sub(self.started.elapsed()) {
                Some(remaining) => context.request_repaint_after(remaining),

                None => {
                    self.picture_requested = true;

                    let seconds = self.started.elapsed().as_secs_f32();

                    eprintln!(
                        "{} frames in {seconds:.1} s: {:.0} frames per second",
                        self.frames,
                        self.frames as f32 / seconds
                    );

                    context.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
                }
            }

            return;
        }

        let picture = context.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(Arc::clone(image)),
                _ => None,
            })
        });

        match picture {
            Some(picture) => {
                if let Err(error) = save(&picture, &path) {
                    eprintln!("error: cannot save {}: {error}", path.display());
                }

                context.send_viewport_cmd(ViewportCommand::Close);
            }

            None => context.request_repaint(),
        }
    }
}

fn save(picture: &ColorImage, path: &Path) -> image::ImageResult<()> {
    let [width, height] = picture.size;

    image::save_buffer(
        path,
        picture.as_raw(),
        width as u32,
        height as u32,
        image::ExtendedColorType::Rgba8,
    )
}

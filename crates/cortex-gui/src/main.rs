//! The Cortex desktop app.

// A release build on Windows is a window app: without this a console
// window opens beside it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod args;
mod attachment;
mod background;
mod clipboard;
mod details;
mod engine;
mod environment;
mod format;
mod menu_bar;
mod performance;
mod rehearsal;
mod session;
mod settings;
mod theme;
mod views;
mod workspace;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::parser::ValueSource;
use clap::{CommandFactory, FromArgMatches};
use eframe::egui;

use crate::app::CortexApp;
use crate::args::Args;
use crate::engine::Engine;
use crate::rehearsal::Rehearsal;
use crate::settings::Settings;

fn main() -> ExitCode {
    environment::adopt_shell_path();

    match run() {
        Ok(()) => ExitCode::SUCCESS,

        Err(error) => {
            eprintln!("error: {error:#}");

            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let matches = Args::command().get_matches();
    let args = Args::from_arg_matches(&matches)?;

    // The demo runs on defaults and saves nothing, so trying it leaves
    // the person's own settings as they were.
    let mut settings = if args.demo {
        Settings::default()
    } else {
        Settings::load()
    };

    settings.apply_command_line(&matches, &args.session);

    if !args.demo
        && let Err(error) = settings.save()
    {
        eprintln!("warning: the settings were not saved: {error:#}");
    }

    // A folder named on the command line is the one to start in.
    let open: Option<PathBuf> = (matches.value_source("dir") == Some(ValueSource::CommandLine))
        .then(|| args.session.dir.canonicalize().ok())
        .flatten();

    // The agent lives on this runtime; the window owns the main thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;

    let tried_theme = args.rehearsal.theme.clone();
    let window_size = args.rehearsal.window_size.as_deref().and_then(|size| {
        let (width, height) = size.split_once('x')?;

        Some([
            width.trim().parse::<f32>().ok()?,
            height.trim().parse::<f32>().ok()?,
        ])
    });
    let rehearsal = Rehearsal::new(args.rehearsal);
    let demo = args.demo;
    let split = args.split;

    let viewport = egui::ViewportBuilder::default()
        .with_title("Cortex")
        .with_inner_size(window_size.unwrap_or([1280.0, 820.0]))
        // Small enough to sit beside an editor: the sidebar steps aside
        // and the bars fold their controls into menus to fit.
        .with_min_inner_size([640.0, 480.0])
        // Open filling the screen; the size above is what the window
        // returns to when it is un-zoomed.
        .with_maximized(window_size.is_none())
        // Let the app draw its own top bar under the window's buttons.
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);

    let options = eframe::NativeOptions {
        viewport,
        wgpu_options: eframe::WgpuConfiguration::default()
            .with_surface_config(eframe::SurfaceConfig::LOW_LATENCY),
        ..Default::default()
    };

    let handle = runtime.handle().clone();

    eframe::run_native(
        "Cortex",
        options,
        Box::new(move |creation| {
            let render = creation
                .wgpu_render_state
                .as_ref()
                .ok_or("the app needs the wgpu renderer")?;
            let adapter = render.adapter.get_info();
            eprintln!(
                "UI renderer: {:?}, {}, {:?}",
                adapter.backend, adapter.name, adapter.device_type
            );

            // A theme that is no longer offered leaves the default in use.
            theme::palette::choose(&settings.theme);

            if let Some(theme) = &tried_theme {
                theme::palette::choose(theme);
            }

            theme::apply(&creation.egui_ctx);

            let engine = Engine::start(&handle, creation.egui_ctx.clone());

            Ok(Box::new(
                workspace::Workspace::new(
                    CortexApp::new(engine, settings, open, demo, rehearsal),
                    split,
                )
                .with_menu_bar(menu_bar::MenuBar::install(&creation.egui_ctx)),
            ))
        }),
    )
    .map_err(|error| anyhow!("the window could not be opened: {error}"))
}

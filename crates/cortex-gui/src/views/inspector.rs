//! The panel on the right that explains the run and what the project
//! has taught.
//!
//! **Run**: how far the run has got against what similar tasks took, its
//! spending against its budget, whether it moved to a stronger model,
//! the approvals given, and the files it changed.
//!
//! **Learned**: from the project's earlier runs, how each kind of task
//! goes, notes that never helped, and workflows offered as skills, which
//! are kept or dismissed here.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cortex_core::learn::{self, Learned};
use cortex_core::ledger::{CallCost, PriceTable};
use cortex_core::notes::{Note, Notes};
use eframe::egui::{self, Rect, RichText, ScrollArea, Sense, Ui, vec2};

use crate::session::Session;
use crate::theme::{self, palette, space};

/// How long what was learned is shown before it is read again.
const FRESH_FOR: Duration = Duration::from_secs(5);

/// Changed files listed before the rest are counted.
const MAX_FILES: usize = 10;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Run,
    Learned,
}

/// What was read about a project, and when.
struct Read {
    place: PathBuf,
    learned: Learned,
    notes: Vec<Note>,
    at: Instant,
}

/// The panel's state, kept between frames.
#[derive(Default)]
pub struct Inspector {
    pub visible: bool,
    tab: Tab,
    read: Option<Read>,

    /// The outcome of the last Accept or Dismiss.
    said: Option<String>,
}

/// The limits of a run, from the settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    pub cost: Option<f64>,
    pub tokens: Option<u64>,
    pub minutes: Option<u32>,
}

/// What the panel needs to know about the session.
pub struct Context<'a> {
    pub place: &'a Path,
    pub session: &'a Session,
    pub provider: &'a str,
    pub limits: Limits,
    pub prices: &'a PriceTable,
}

impl Inspector {
    /// Reads again soon, after a run has added to what can be learned.
    pub fn stale(&mut self) {
        self.read = None;
    }

    /// Opens the panel on the run.
    pub fn show_run(&mut self) {
        self.visible = true;
        self.tab = Tab::Run;
    }

    /// Opens the panel on what was learned.
    pub fn show_learned(&mut self) {
        self.visible = true;
        self.tab = Tab::Learned;
        self.stale();
    }

    pub fn show(&mut self, ui: &mut Ui, context: &Context<'_>) {
        ui.add_space(space::XS);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::L;

            for (tab, name) in [(Tab::Run, "Run"), (Tab::Learned, "Learned")] {
                let chosen = self.tab == tab;

                let text = RichText::new(name)
                    .font(theme::strong(theme::size::UI))
                    .color(if chosen {
                        palette::text()
                    } else {
                        palette::text_muted()
                    });

                if ui
                    .add(egui::Label::new(text).sense(Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    self.tab = tab;
                }
            }
        });

        ui.add_space(space::S);

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                Tab::Run => run(ui, context),
                Tab::Learned => self.learned(ui, context.place),
            });
    }

    fn learned(&mut self, ui: &mut Ui, place: &Path) {
        let fresh = self
            .read
            .as_ref()
            .is_some_and(|read| read.place == place && read.at.elapsed() < FRESH_FOR);

        if !fresh {
            self.read = Some(Read {
                place: place.to_path_buf(),
                learned: learn::for_project(place),
                notes: Notes::for_project(place).all(),
                at: Instant::now(),
            });
        }

        let Some(read) = &self.read else {
            return;
        };

        if read.learned.runs == 0 {
            muted(
                ui,
                "Nothing yet. Every run that changes the project is kept to learn from.",
            );

            return;
        }

        heading(ui, &format!("{} runs", read.learned.runs));

        for kind in &read.learned.kinds {
            ui.add_space(space::S);

            pair(
                ui,
                kind.kind.label(),
                &format!(
                    "{:.0}% · ~{} steps",
                    kind.pass_rate() * 100.0,
                    kind.median_steps
                ),
            );

            bar(ui, kind.pass_rate(), palette::accent());

            if !kind.helpful_tools.is_empty() {
                muted(
                    ui,
                    &format!("Passing runs used {}", kind.helpful_tools.join(", ")),
                );
            }
        }

        let doubtful: Vec<&Note> = read
            .learned
            .notes
            .iter()
            .filter(|value| value.doubtful())
            .filter_map(|value| read.notes.iter().find(|note| note.id == value.id))
            .collect();

        if !doubtful.is_empty() {
            heading(ui, "Notes that never helped");

            for note in doubtful {
                ui.add(egui::Label::new(
                    RichText::new(format!("#{} {}", note.id, note.text))
                        .font(theme::small())
                        .color(palette::text_secondary()),
                ));
                ui.add_space(space::XS);
            }
        }

        let mut accepted = None;
        let mut dismissed = None;

        if !read.learned.candidates.is_empty() {
            heading(ui, "Skills to keep");

            for candidate in &read.learned.candidates {
                pair(
                    ui,
                    &format!("/{}", candidate.name),
                    &format!("{} runs", candidate.runs),
                );

                ui.add(egui::Label::new(
                    RichText::new(candidate.steps.join(" → "))
                        .font(theme::mono())
                        .color(palette::text_secondary()),
                ));

                ui.add_space(space::XS);

                ui.horizontal(|ui| {
                    if ui.button("Keep").clicked() {
                        accepted = Some(candidate.clone());
                    }

                    if ui.button("Dismiss").clicked() {
                        dismissed = Some(candidate.name.clone());
                    }
                });

                ui.add_space(space::M);
            }
        }

        if let Some(candidate) = accepted {
            self.said = Some(match learn::accept(place, &candidate) {
                Ok(_) => format!("Saved. /{} starts it.", candidate.name),
                Err(error) => format!("Not saved: {error}"),
            });

            self.stale();
        }

        if let Some(name) = dismissed {
            self.said = Some(match learn::dismiss(place, &name) {
                Ok(()) => format!("{name} will not be offered again."),
                Err(error) => format!("Not dismissed: {error}"),
            });

            self.stale();
        }

        if let Some(said) = &self.said {
            ui.add_space(space::S);
            muted(ui, said);
        }
    }
}

fn run(ui: &mut Ui, context: &Context<'_>) {
    let session = context.session;
    let state = &session.run;

    if state.started.is_none() && session.model.is_none() {
        muted(ui, "Nothing has run yet in this session.");

        return;
    }

    let usage = session.stats.run_usage();
    let tokens = usage.total();

    let cost = session
        .model
        .as_deref()
        .map(|model| context.prices.cost(context.provider, model, &usage));

    heading(ui, if session.running { "Now" } else { "Last run" });

    let steps = match &state.estimate {
        Some(estimate) => format!("{} of ~{}", session.stats.step, estimate.steps.max(1)),
        None => session.stats.step.to_string(),
    };

    pair(ui, "Steps", &steps);

    let calls = match state.failed_calls {
        0 => state.calls.to_string(),
        failed => format!("{} · {failed} failed", state.calls),
    };

    pair(ui, "Tool calls", &calls);

    let spent = match cost {
        Some(CallCost::Usd(usd)) => format!("{} · ${usd:.2}", count(tokens)),
        _ => count(tokens),
    };

    pair(ui, "Tokens", &spent);

    let elapsed = state.elapsed().filter(|_| session.running);

    if let Some(elapsed) = elapsed {
        pair(ui, "Time", &format!("{}s", elapsed.as_secs()));
    }

    // The budget: the tightest of its limits.
    let limits = context.limits;

    let shares = [
        limits.tokens.map(|max| {
            (
                tokens as f64 / max.max(1) as f64,
                format!("{} tokens", count(max)),
            )
        }),
        match (limits.cost, cost) {
            (Some(max), Some(CallCost::Usd(usd))) => Some((usd / max, format!("${max:.2}"))),
            _ => None,
        },
        match (limits.minutes, elapsed) {
            (Some(max), Some(elapsed)) => Some((
                elapsed.as_secs_f64() / (f64::from(max) * 60.0),
                format!("{max} min"),
            )),
            _ => None,
        },
    ];

    let tightest = shares
        .into_iter()
        .flatten()
        .max_by(|a, b| a.0.total_cmp(&b.0));

    if let Some((share, limit)) = tightest {
        heading(ui, "Budget");

        let share = share.max(state.budget.map_or(0.0, |(share, _)| share));

        pair(
            ui,
            &format!("{:.0}% used", share * 100.0),
            &format!("of {limit}"),
        );

        let colour = match share {
            share if share >= 1.0 => palette::bad(),
            share if share >= cortex_core::budget::WARN_AT => palette::warning(),
            _ => palette::accent(),
        };

        bar(ui, share.min(1.0), colour);
    }

    if let Some(estimate) = &state.estimate {
        heading(ui, "Similar tasks");

        pair(
            ui,
            estimate.kind.label(),
            &format!(
                "{} runs · {:.0}% passed",
                estimate.samples,
                estimate.pass_rate * 100.0
            ),
        );

        if !estimate.helpful_tools.is_empty() {
            muted(
                ui,
                &format!("Passing runs used {}", estimate.helpful_tools.join(", ")),
            );
        }
    }

    if let Some((model, reason)) = &state.escalated {
        heading(ui, "Recovery");

        pair(ui, "Moved to", model);
        muted(ui, reason);
    }

    if state.approved + state.refused > 0 {
        heading(ui, "Approvals");

        pair(
            ui,
            &format!("{} allowed", state.approved),
            &format!("{} refused", state.refused),
        );
    }

    if !state.changed.is_empty() {
        heading(ui, &format!("Changed · {}", state.changed.len()));

        for file in state.changed.iter().take(MAX_FILES) {
            ui.add(
                egui::Label::new(
                    RichText::new(file)
                        .font(theme::mono())
                        .color(palette::text_secondary()),
                )
                .truncate(),
            );
        }

        let more = state.changed.len().saturating_sub(MAX_FILES);

        if more > 0 {
            muted(ui, &format!("{more} more"));
        }
    }
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(space::XL);
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::text_muted()),
    );
    ui.add_space(space::S);
}

/// A name on the left and its value on the right.
fn pair(ui: &mut Ui, name: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).font(theme::ui()).color(palette::text()));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .font(theme::ui())
                    .color(palette::text_secondary()),
            );
        });
    });
}

fn bar(ui: &mut Ui, share: f64, colour: egui::Color32) {
    ui.add_space(space::XS);

    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 2.0, palette::sunken());
    painter.rect_filled(
        Rect::from_min_size(
            rect.min,
            vec2(rect.width() * share.clamp(0.0, 1.0) as f32, rect.height()),
        ),
        2.0,
        colour,
    );

    ui.add_space(space::XS);
}

fn muted(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(
        RichText::new(text)
            .font(theme::small())
            .color(palette::text_muted()),
    ));
}

/// A count of tokens, short: 950, 12.4k, 3.1M.
fn count(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}k", tokens as f64 / 1_000.0),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

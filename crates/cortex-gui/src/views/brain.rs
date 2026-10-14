//! The network view's tabs: the standalone brain as it lives, every
//! number behind what it does, and the network that learns the coding
//! agent's habits.
//!
//! The brain tab draws every cell and synapse of its cortex (see
//! [`brain_map`]), lit by whatever each part is doing, beside a panel on
//! its mind: time as it felt, its state as it measures it, and its
//! thoughts, dreams, imaginings and reasoning.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use cortex_living::Living;
use cortex_living::neural::{NeuralSnapshot, Recall, Step, Thought};
use cortex_living::service::Request;
use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, RichText, ScrollArea, Sense, Ui, vec2};

use crate::theme::{self, palette};
use crate::views::brain_map::{self, MapView};

const PANEL_WIDTH: f32 = 350.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Brain,
    Metrics,
    Habits,
}

#[derive(Default)]
pub struct BrainView {
    pub tab: Tab,
    engram: Option<u64>,
    step: Option<u64>,
    map: MapView,
    seed: String,
    from: String,
    to: String,
    asked: Option<Receiver<Result<Thought, String>>>,
    answer: Option<Result<Thought, String>>,
    thought: Option<u64>,
}

impl BrainView {
    /// Asks the brain's mind in the background; the answer shows when it
    /// comes.
    fn ask(&mut self, living: Option<&Arc<Living>>, request: Request) {
        let Some(living) = living.cloned() else {
            self.answer = Some(Err("The background service is not running.".into()));
            return;
        };
        let (send, receive) = channel();
        self.asked = Some(receive);
        let _ = std::thread::Builder::new().name("brain-mind".into()).spawn(move || {
            let thought = living
                .neural_request(&request)
                .map_err(|e| e.to_string())
                .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()));
            let _ = send.send(thought);
        });
    }
}

/// The tab strip. Returns the tab to show.
pub fn tabs(ui: &mut Ui, view: &mut BrainView) -> Tab {
    ui.add_space(theme::space::S);
    ui.horizontal(|ui| {
        ui.add_space(theme::space::L);
        for (tab, name) in [
            (Tab::Brain, "Brain"),
            (Tab::Metrics, "Metrics"),
            (Tab::Habits, "Agent habits"),
        ] {
            let text = RichText::new(name).font(theme::ui()).color(if view.tab == tab {
                palette::text()
            } else {
                palette::text_secondary()
            });
            if ui.selectable_label(view.tab == tab, text).clicked() {
                view.tab = tab;
            }
        }
    });
    ui.add_space(theme::space::XS);
    view.tab
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).font(theme::strong(12.5)));
    ui.add_space(2.0);
}

fn muted(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(RichText::new(text).font(theme::small()).color(palette::text_muted()))
            .wrap(),
    );
}

fn plain(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(RichText::new(text).font(theme::small()).color(palette::text())).wrap(),
    );
}

fn row(ui: &mut Ui, name: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(name)
                .font(theme::small())
                .color(palette::text_secondary()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).font(theme::small()).color(palette::text()));
        });
    });
}

fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.0)
}

pub(crate) fn count(number: impl Into<u64>) -> String {
    let number = number.into();
    let digits = number.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The brain tab.
/// `teaching` is the teacher's state, from the background service.
pub fn show_brain(
    ui: &mut Ui,
    view: &mut BrainView,
    brain: &NeuralSnapshot,
    teaching: &serde_json::Value,
    living: Option<&Arc<Living>>,
) {
    if let Some(asked) = &view.asked
        && let Ok(answer) = asked.try_recv()
    {
        view.answer = Some(answer);
        view.asked = None;
    }
    egui::Panel::right(ui.id().with("brain panel"))
        .exact_size(PANEL_WIDTH)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(palette::sidebar())
                .inner_margin(egui::Margin::symmetric(16, 12)),
        )
        .show(ui, |ui| {
            ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| brain_panel(ui, view, brain, teaching, living));
        });
    let engram = view.engram;
    brain_map::show(ui, &mut view.map, brain, living, engram);
}

fn brain_panel(
    ui: &mut Ui,
    view: &mut BrainView,
    brain: &NeuralSnapshot,
    teaching: &serde_json::Value,
    living: Option<&Arc<Living>>,
) {
    ui.set_width(PANEL_WIDTH - 32.0);
    ui.label(RichText::new("Brain").font(theme::strong(15.0)));
    muted(
        ui,
        "Columns of spiking cells whose dendrites learn what comes next. It is taught by the \
         LLM teacher and by you, keeps memories as the cells that held them, thinks and imagines \
         when left alone, and sleeps and dreams when tired. Each of these is the same cortex run \
         a different way; the colours on the left show which is using which cells. It is not a \
         language model, and its self-model is a measurement, not a feeling.",
    );

    mind_section(ui, view, brain, living);

    let stats = &brain.stats;
    heading(ui, "How it is doing");
    row(
        ui,
        "Next word right (recent)",
        &format!(
            "{}  vs {} guessing",
            percent(stats.recent_right),
            percent(stats.recent_baseline)
        ),
    );
    row(ui, "Recall brings back", &percent(stats.recall_fidelity));
    row(
        ui,
        "Said · new sentences",
        &format!("{} · {}", stats.spoken, stats.novel),
    );
    row(
        ui,
        "Memories · lessons",
        &format!("{} · {}", brain.engram_count, stats.lessons),
    );
    muted(
        ui,
        "Talk to it with /brain <words>, ask it to remember with /recall <cue>, teach it with \
         /teach snn+dann.",
    );

    crate::views::living::teacher(ui, teaching);

    heading(ui, "It said");
    if brain.said.is_empty() {
        muted(ui, "Nothing yet.");
    }
    for said in brain.said.iter().rev().take(6) {
        ui.add_space(3.0);
        muted(ui, &crate::format::one_line(&said.prompt, 60));
        let words = if said.text.is_empty() { "…" } else { &said.text };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(words).font(theme::small()).color(palette::text()));
            if said.novel {
                ui.label(
                    RichText::new("new")
                        .font(theme::strong(11.0))
                        .color(palette::good()),
                );
            }
        });
    }

    heading(ui, "Curious about");
    if brain.curious.is_empty() {
        muted(ui, "Nothing yet: words it meets twice and is still surprised by land here.");
    }
    for curious in brain.curious.iter().take(8) {
        row(
            ui,
            &curious.word,
            &format!(
                "surprised {}{}",
                percent(curious.surprise),
                if curious.progress > 0.01 {
                    format!(", learning ({} better)", percent(curious.progress))
                } else {
                    String::new()
                }
            ),
        );
    }

    heading(ui, "Memories");
    muted(ui, "Each is the set of cells that held it. Click one to light its columns.");
    if brain.engrams.is_empty() {
        muted(ui, "None yet.");
    }
    for engram in brain.engrams.iter().take(30) {
        let chosen = view.engram == Some(engram.id);
        ui.add_space(4.0);
        let response = ui.add(
            egui::Label::new(
                RichText::new(crate::format::one_line(&engram.text, 90))
                    .font(theme::small())
                    .color(if chosen { palette::good() } else { palette::text() }),
            )
            .sense(Sense::click())
            .wrap(),
        );
        if response.clicked() {
            view.engram = if chosen { None } else { Some(engram.id) };
        }
        muted(
            ui,
            &format!(
                "#{} · {} cells in {} columns · strength {:.1} · recalled {} · replayed {}{}",
                engram.id,
                engram.cells,
                engram.columns.len(),
                engram.strength,
                engram.recalls,
                engram.replays,
                engram
                    .fidelity
                    .map(|f| format!(" · last recall {}", percent(f)))
                    .unwrap_or_default()
            ),
        );
    }

    if let Some(recall) = brain.recalls.last() {
        heading(ui, "Latest recall");
        recall_card(ui, recall);
    }
}

/// Its mind: what it is doing, time as it felt, what it says of itself,
/// ways to ask it to think, and every thought it has had.
fn mind_section(ui: &mut Ui, view: &mut BrainView, brain: &NeuralSnapshot, living: Option<&Arc<Living>>) {
    let mind = &brain.mind;
    let felt = &mind.felt;
    heading(ui, "Mind now");
    let state = if felt.asleep {
        format!("Asleep: {}, cycle {}", felt.stage, felt.cycle)
    } else if mind.me.doing.is_empty() {
        "Awake".to_string()
    } else {
        format!("Awake: {}", brain_map::meaning(&mind.me.doing))
    };
    ui.label(
        RichText::new(state)
            .font(theme::strong(12.5))
            .color(if mind.me.doing.is_empty() { palette::text() } else { brain_map::colour(&mind.me.doing) }),
    );
    if !felt.present.is_empty() {
        plain(ui, &format!("In the present: {}", felt.present.iter().cloned().collect::<Vec<_>>().join(" ")));
    }
    let pressure = (felt.pressure / cortex_living::neural::mind::DROWSY).clamp(0.0, 1.0);
    row(ui, "Sleep pressure", &format!("{} · {} nights slept", percent(pressure), felt.nights));
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(bar, CornerRadius::same(2), palette::border());
    ui.painter().rect_filled(
        Rect::from_min_size(bar.min, vec2(bar.width() * pressure, bar.height())),
        CornerRadius::same(2),
        brain_map::colour("dream"),
    );

    heading(ui, "Time as it felt");
    muted(
        ui,
        "A brain has no clock: it builds time from change. Each step counts for more the more it \
         surprised (new things stretch time), far less in replay and dreams (which run compressed), \
         and a surprise well beyond the usual cuts experience into a new episode.",
    );
    row(ui, "Felt · steps lived", &format!("{:.0} moments · {}", felt.moments, count(felt.steps)));
    row(
        ui,
        "A step feels like",
        &format!(
            "{:.2} moments ({})",
            felt.rate,
            if felt.rate > 1.4 { "stretching" } else if felt.rate < 0.6 { "rushing" } else { "even" }
        ),
    );
    if felt.born_ms > 0 {
        let lived_ms = felt.now.ended_ms.saturating_sub(felt.born_ms).max(1);
        row(ui, "Felt per clock minute", &format!("{:.0} moments", felt.moments / (lived_ms as f64 / 60_000.0)));
    }
    row(ui, "Episode now", &format!("#{} from \u{201c}{}\u{201d}", felt.now.id, felt.now.opening));
    episodes(ui, brain);

    heading(ui, "What it says of itself");
    plain(ui, &mind.me.report);
    row(
        ui,
        "Guesses this sure come true",
        &mind.expected.map_or("no record yet".into(), percent),
    );
    row(
        ui,
        "Knows when it knows",
        &mind.insight.map_or("no record yet".into(), |i| format!("{:+.0} points when surer", i * 100.0)),
    );

    heading(ui, "Ask its mind");
    ui.horizontal_wrapped(|ui| {
        for (label, process, request) in [
            ("Think", "think", Request::NeuralThink),
            ("Dream", "dream", Request::NeuralDream),
            ("Reflect", "aware", Request::NeuralReflect),
            ("Hunch", "intuit", Request::NeuralIntuit { cue: view.seed.clone() }),
            ("Imagine", "imagine", Request::NeuralImagine { seed: view.seed.clone() }),
        ] {
            let button = egui::Button::new(RichText::new(label).font(theme::small()).color(brain_map::colour(process)));
            if ui.add_enabled(view.asked.is_none(), button).clicked() {
                view.ask(living, request);
            }
        }
    });
    ui.add(egui::TextEdit::singleline(&mut view.seed).hint_text("seed or cue (optional)").font(theme::small()));
    ui.horizontal(|ui| {
        let half = (ui.available_width() - 70.0) / 2.0;
        ui.add(egui::TextEdit::singleline(&mut view.from).hint_text("from").desired_width(half).font(theme::small()));
        ui.add(egui::TextEdit::singleline(&mut view.to).hint_text("to").desired_width(half).font(theme::small()));
        let ready = view.asked.is_none() && !view.from.trim().is_empty() && !view.to.trim().is_empty();
        let button = egui::Button::new(RichText::new("Reason").font(theme::small()).color(brain_map::colour("reason")));
        if ui.add_enabled(ready, button).clicked() {
            let request = Request::NeuralReason { from: view.from.clone(), to: view.to.clone() };
            view.ask(living, request);
        }
    });
    if view.asked.is_some() {
        muted(ui, "Thinking…");
    }
    if let Some(answer) = &view.answer {
        match answer {
            Ok(thought) => thought_card(ui, thought, true),
            Err(problem) => muted(ui, problem),
        }
    }

    heading(ui, "Thoughts, dreams and imaginings");
    muted(ui, "Everything its mind did, latest first, whether asked or on its own. Click one for how it got there.");
    if mind.thoughts.is_empty() {
        muted(ui, "None yet: left alone for a few seconds, it begins to think.");
    }
    for thought in mind.thoughts.iter().rev() {
        let open = view.thought == Some(thought.number);
        let response = egui::Frame::new()
            .fill(if open { palette::surface() } else { Color32::TRANSPARENT })
            .corner_radius(CornerRadius::same(theme::radius::S))
            .inner_margin(egui::Margin::symmetric(4, 3))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                thought_card(ui, thought, open);
            })
            .response
            .interact(Sense::click());
        if response.clicked() {
            view.thought = if open { None } else { Some(thought.number) };
        }
    }
}

fn thought_card(ui: &mut Ui, thought: &Thought, open: bool) {
    let process = if thought.kind == "reflect" { "aware" } else { thought.kind.as_str() };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&thought.kind).font(theme::strong(11.0)).color(brain_map::colour(process)));
        if !thought.seed.is_empty() {
            ui.label(RichText::new(crate::format::one_line(&thought.seed, 40)).font(theme::small()).color(palette::text_muted()));
        }
        if thought.novel {
            ui.label(RichText::new("new").font(theme::strong(11.0)).color(palette::good()));
        }
    });
    let text = if thought.text.is_empty() { "…" } else { &thought.text };
    if open {
        plain(ui, text);
    } else {
        plain(ui, &crate::format::one_line(text, 120));
    }
    if open {
        if !thought.path.is_empty() {
            let links: Vec<String> = thought
                .path
                .iter()
                .map(|(word, share)| format!("{word} {}", percent(*share)))
                .collect();
            muted(ui, &format!("{}: {}", if thought.kind == "reason" { "Links" } else { "Weighed" }, links.join(" → ")));
        }
        if !thought.sources.is_empty() {
            muted(
                ui,
                &format!(
                    "From memories {}",
                    thought.sources.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
                ),
            );
        }
        muted(ui, &thought.why);
    }
}

/// The latest episodes twice: as long as they felt, and as long as they
/// took by the clock. Where the rows differ, time stretched or shrank.
fn episodes(ui: &mut Ui, brain: &NeuralSnapshot) {
    let felt = &brain.mind.felt;
    let mut list: Vec<&cortex_living::neural::Episode> = felt.episodes.iter().rev().take(40).collect();
    list.reverse();
    list.push(&felt.now);
    let total_felt: f64 = list.iter().map(|e| e.felt).sum::<f64>().max(1e-6);
    let total_clock: f64 = list
        .iter()
        .map(|e| e.ended_ms.saturating_sub(e.began_ms).max(1) as f64)
        .sum::<f64>()
        .max(1.0);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let painter = ui.painter();
    let mut x_felt = rect.left();
    let mut x_clock = rect.left();
    let pointer = response.hover_pos();
    let mut hovered = None;
    for episode in &list {
        let colour = brain_map::colour(&episode.process);
        let wide_felt = (episode.felt / total_felt) as f32 * rect.width();
        let wide_clock = (episode.ended_ms.saturating_sub(episode.began_ms).max(1) as f64 / total_clock) as f32 * rect.width();
        let a = Rect::from_min_size(egui::pos2(x_felt, rect.top()), vec2(wide_felt.max(1.0), 12.0));
        let b = Rect::from_min_size(egui::pos2(x_clock, rect.top() + 18.0), vec2(wide_clock.max(1.0), 12.0));
        painter.rect_filled(a.shrink2(vec2(0.5, 0.0)), CornerRadius::same(1), colour.gamma_multiply(0.85));
        painter.rect_filled(b.shrink2(vec2(0.5, 0.0)), CornerRadius::same(1), colour.gamma_multiply(0.45));
        if pointer.is_some_and(|p| a.contains(p) || b.contains(p)) {
            hovered = Some(*episode);
        }
        x_felt += wide_felt;
        x_clock += wide_clock;
    }
    painter.text(rect.right_top(), Align2::RIGHT_TOP, "felt", theme::small(), palette::text_muted());
    painter.text(egui::pos2(rect.right(), rect.top() + 18.0), Align2::RIGHT_TOP, "clock", theme::small(), palette::text_muted());
    if let Some(episode) = hovered {
        response.on_hover_text(format!(
            "Episode {}: {}, from \u{201c}{}\u{201d}. {} steps, felt {:.1} moments over {:.1} s; mean surprise {}; opened by a surprise of {}.",
            episode.id,
            brain_map::meaning(&episode.process),
            episode.opening,
            episode.steps,
            episode.felt,
            episode.ended_ms.saturating_sub(episode.began_ms) as f64 / 1000.0,
            percent(episode.surprise),
            percent(episode.boundary)
        ));
    }
}

fn recall_card(ui: &mut Ui, recall: &Recall) {
    plain(ui, &format!("Cue: {}", recall.cue));
    match recall.engram {
        Some(id) => {
            plain(ui, &format!("Brought back: {}", recall.recalled));
            muted(ui, &format!("Memory {id} holds: {}", crate::format::one_line(&recall.remembered, 160)));
            row(ui, "Cells re-fired · came back", &format!("{} · {}", recall.reignited.len(), recall.returned.len()));
            row(ui, "Of stored cells, came back", &percent(recall.fidelity));
            row(ui, "Words right", &format!("{} of {}", recall.words_right, recall.words));
        }
        None => plain(ui, "Nothing came back."),
    }
    muted(ui, &recall.why);
}

/// The metrics tab: every counter, then every step with why.
pub fn show_metrics(ui: &mut Ui, view: &mut BrainView, brain: &NeuralSnapshot) {
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(theme::space::S);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(20, 4))
            .show(ui, |ui| metrics(ui, view, brain));
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

fn metrics(ui: &mut Ui, view: &mut BrainView, brain: &NeuralSnapshot) {
    let stats = &brain.stats;
    let totals = &brain.totals;
    let width = ui.available_width();
    let columns = ((width / 300.0) as usize).clamp(1, 5);

    ui.columns(columns, |cols| {
        let mut at = 0;
        let mut group = |title: &str, rows: Vec<(&str, String)>| {
            let ui = &mut cols[at % columns];
            at += 1;
            heading(ui, title);
            for (name, value) in rows {
                row(ui, name, &value);
            }
        };
        group(
            "Growth",
            vec![
                ("Cortex columns", count(brain.area as u64)),
                (
                    "…grown beyond the first columns",
                    count(brain.area.saturating_sub(cortex_living::neural::words::START_COLUMNS) as u64),
                ),
                ("Memory used", format!("{} MB", brain.footprint >> 20)),
                ("Budget it may grow into", format!("{:.1} GB", brain.budget as f64 / f64::from(1u32 << 30))),
                ("Budget used", percent(brain.footprint as f32 / brain.budget.max(1) as f32)),
            ],
        );
        group(
            "Neurons",
            vec![
                ("Cells alive", count(brain.cells as u64)),
                ("Cells born · died", format!("{} · {}", count(totals.cells_grown), count(totals.cells_died))),
                ("Spikes", count(totals.spikes)),
                ("Steps lived", count(totals.steps)),
            ],
        );
        group(
            "Dendrites and synapses",
            vec![
                ("Dendritic branches", count(brain.branches as u64)),
                ("Branches grown · pruned", format!("{} · {}", count(totals.segments_grown), count(totals.segments_pruned))),
                ("Dendritic spikes", count(totals.dendritic_spikes)),
                ("Synapses (connected)", format!("{} ({})", count(brain.synapses as u64), count(brain.connected as u64))),
                ("Synapses grown · pruned", format!("{} · {}", count(totals.synapses_grown), count(totals.synapses_pruned))),
                ("Strengthened · weakened", format!("{} · {}", count(totals.strengthened), count(totals.weakened))),
            ],
        );
        group(
            "Learning",
            vec![
                ("Lessons", count(stats.lessons)),
                ("Words learned", count(stats.tokens_learned)),
                ("Vocabulary", count(brain.vocabulary as u64)),
                ("Next word right, all", if stats.scored == 0 { "–".into() } else { percent(stats.right as f32 / stats.scored as f32) }),
                ("…guessing the commonest", if stats.scored == 0 { "–".into() } else { percent(stats.baseline_right as f32 / stats.scored as f32) }),
                ("Next word right, recent", format!("{} vs {}", percent(stats.recent_right), percent(stats.recent_baseline))),
                ("Corrections", count(stats.corrections)),
                ("Words drifted", count(stats.drifted)),
            ],
        );
        let mind = &brain.mind;
        let kinds = |kind: &str| mind.thoughts.iter().filter(|t| t.kind == kind).count() as u64;
        group(
            "Mind",
            vec![
                ("Felt time", format!("{:.0} moments", mind.felt.moments)),
                ("Steps lived (felt)", count(mind.felt.steps)),
                ("A step feels like", format!("{:.2} moments", mind.felt.rate)),
                ("Episodes", count(mind.felt.now.id)),
                ("Sleep pressure · nights", format!("{:.2} · {}", mind.felt.pressure, mind.felt.nights)),
                ("Thoughts · dreams · imaginings", format!("{} · {} · {}", kinds("think"), kinds("dream"), kinds("imagine"))),
                ("Reasonings · hunches · decisions", format!("{} · {} · {}", kinds("reason"), kinds("intuit"), kinds("decide"))),
                ("Guesses this sure come true", mind.expected.map_or("–".into(), percent)),
                ("Map: places traded", count(mind.swaps)),
            ],
        );
        group(
            "Memory and sleep",
            vec![
                ("Memories", count(brain.engram_count as u64)),
                ("Forgotten", count(stats.engrams_forgotten)),
                ("Recalls", count(stats.recalls)),
                ("Recall brings back", percent(stats.recall_fidelity)),
                ("Sleeps · replays", format!("{} · {}", count(stats.sleeps), count(stats.replays))),
                ("Said · new sentences", format!("{} · {}", count(stats.spoken), count(stats.novel))),
            ],
        );
    });

    heading(ui, "Synapse strengths");
    muted(ui, &format!("How many synapses sit at each permanence. At {} and above a synapse is connected and counts towards a dendritic spike ({} needed).", cortex_living::neural::cortex::CONNECTED, cortex_living::neural::cortex::ACTIVATION));
    let (bars, _) = ui.allocate_exact_size(vec2(ui.available_width().min(520.0), 70.0), Sense::hover());
    let highest = brain.permanences.iter().copied().max().unwrap_or(1).max(1) as f32;
    let slot = bars.width() / 10.0;
    for (at, value) in brain.permanences.iter().enumerate() {
        let height = (bars.height() - 14.0) * (*value as f32 / highest);
        let bar = Rect::from_min_max(
            egui::pos2(bars.left() + at as f32 * slot + 2.0, bars.bottom() - 14.0 - height),
            egui::pos2(bars.left() + (at + 1) as f32 * slot - 2.0, bars.bottom() - 14.0),
        );
        let colour = if at >= 5 { palette::accent() } else { palette::text_muted().gamma_multiply(0.6) };
        ui.painter().rect_filled(bar, CornerRadius::same(2), colour);
        ui.painter().text(
            egui::pos2(bar.center().x, bars.bottom()),
            Align2::CENTER_BOTTOM,
            format!("{:.1}", at as f32 / 10.0),
            theme::small(),
            palette::text_muted(),
        );
    }

    heading(ui, "Every step, latest first");
    muted(ui, "learn: taught text · hear: something you said · speak: its own words · recall: remembering · replay: deep sleep · dream, think, imagine, reason, intuit: its mind. Click a step for its dendritic spikes and synaptic changes.");
    ui.add_space(4.0);
    for step in brain.trace.iter().rev() {
        step_row(ui, view, step);
    }

    if !brain.recalls.is_empty() {
        heading(ui, "Recalls, latest first");
        for recall in brain.recalls.iter().rev().take(10) {
            egui::Frame::new()
                .fill(palette::surface())
                .corner_radius(CornerRadius::same(theme::radius::M))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    recall_card(ui, recall);
                });
            ui.add_space(6.0);
        }
    }
}

fn mode_colour(mode: &str) -> Color32 {
    brain_map::colour(mode)
}

fn step_row(ui: &mut Ui, view: &mut BrainView, step: &Step) {
    let open = view.step == Some(step.number);
    let response = egui::Frame::new()
        .fill(if open { palette::surface() } else { Color32::TRANSPARENT })
        .corner_radius(CornerRadius::same(theme::radius::S))
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("#{}", step.number)).font(theme::mono()).color(palette::text_muted()));
                ui.label(RichText::new(&step.mode).font(theme::strong(11.5)).color(mode_colour(&step.mode)));
                ui.label(RichText::new(&step.token).font(theme::mono()).color(palette::text()));
                ui.label(
                    RichText::new(format!(
                        "{} primed · {} burst · surprise {}",
                        step.predicted,
                        step.bursting,
                        percent(step.surprise)
                    ))
                    .font(theme::small())
                    .color(palette::text_secondary()),
                );
                if !step.guesses.is_empty() {
                    let guesses: Vec<String> = step
                        .guesses
                        .iter()
                        .take(3)
                        .map(|(word, share, _)| format!("{word} {}", percent(*share)))
                        .collect();
                    ui.label(
                        RichText::new(format!("expected: {}", guesses.join(", ")))
                            .font(theme::small())
                            .color(palette::text_muted()),
                    );
                }
            });
            muted(ui, &step.why);
            if open {
                ui.add_space(4.0);
                let changes = &step.changes;
                plain(
                    ui,
                    &format!(
                        "Synapses strengthened {} · weakened {} · grown {} · branches grown {} · cells grown {} · branches punished for a wrong guess {} · cells spiking {}",
                        changes.strengthened,
                        changes.weakened,
                        changes.synapses_grown,
                        changes.segments_grown,
                        changes.cells_grown,
                        changes.punished,
                        step.spiking
                    ),
                );
                if step.branches.is_empty() {
                    muted(ui, "No dendritic spike: nothing is primed for the next word.");
                } else {
                    muted(ui, "Strongest dendritic spikes, priming the next word:");
                    for fire in &step.branches {
                        plain(
                            ui,
                            &format!(
                                "cell {} in column {}, branch {}: {} of its {} synapses connected to cells that just spiked ({} needed)",
                                fire.cell, fire.column, fire.segment, fire.matched, fire.synapses, fire.needed
                            ),
                        );
                    }
                }
                if !step.guesses.is_empty() {
                    let active = cortex_living::neural::words::ACTIVE;
                    muted(ui, &format!("Before it, each word scored by the share of its {active} columns that were primed:"));
                    for (word, share, hits) in &step.guesses {
                        plain(ui, &format!("{word}: {hits} of {active} = {}", percent(*share)));
                    }
                }
            }
        })
        .response
        .interact(Sense::click());
    if response.clicked() {
        view.step = if open { None } else { Some(step.number) };
    }
}

/// While the background service has not reported the brain yet.
pub fn waiting(ui: &mut Ui) {
    ui.add_space(ui.available_height() * 0.3);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("Waking the brain…").font(theme::strong(theme::size::TITLE)));
        ui.add_space(theme::space::XS);
        ui.label(
            RichText::new("It lives in its background service, which keeps running after Cortex closes.")
                .color(palette::text_muted()),
        );
    });
}

#[cfg(test)]
mod tests {
    use cortex_living::neural::Brain;

    use super::*;

    #[test]
    fn both_tabs_draw_an_empty_and_a_taught_brain() {
        let context = egui::Context::default();
        theme::apply(&context);
        let mut brain = Brain::new();
        let empty = brain.snapshot();
        for _ in 0..3 {
            brain.learn("<story> the cat sat on the mat .", "test");
        }
        brain.say("<story> the cat");
        brain.recall("the cat");
        brain.think();
        brain.dream();
        brain.reason("the cat", "mat");
        let taught = brain.snapshot();
        let mut view = BrainView::default();
        for snapshot in [&empty, &taught] {
            for tab in [Tab::Brain, Tab::Metrics] {
                view.tab = tab;
                view.step = snapshot.trace.last().map(|s| s.number);
                view.engram = snapshot.engrams.first().map(|e| e.id);
                let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                    egui::CentralPanel::default().show(ui, |ui| match tab {
                        Tab::Brain => show_brain(ui, &mut view, snapshot, &serde_json::Value::Null, None),
                        _ => show_metrics(ui, &mut view, snapshot),
                    });
                });
                output.textures_delta.clear();
            }
        }
    }
}

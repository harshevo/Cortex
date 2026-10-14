//! The trading chat: a header, the conversation (or long-term memory),
//! and a composer kept at the bottom.

use std::sync::Arc;

use cortex_market::{Snapshot, Source, chat::TradingChat};
use cortex_memory::experience::Domain;
use eframe::egui::{self, Align, CornerRadius, Layout, RichText, Stroke, Ui, vec2};

use super::widgets::{self, Glyph, Kind};
use crate::theme::{self, palette};
use crate::views::icons::Icon;
use crate::views::prose;

pub struct ChatView {
    pub draft: String,
    pub memory_query: String,
    pub show_memory: bool,
    pub show_readings: bool,
    pub domain: usize,
    focus: bool,

    /// How tall the composer was last frame, kept clear for it.
    composer_height: f32,
}

impl Default for ChatView {
    fn default() -> Self {
        Self {
            draft: String::new(),
            memory_query: String::new(),
            show_memory: false,
            show_readings: false,
            domain: 0,
            focus: true,
            composer_height: 120.0,
        }
    }
}

/// What the chat asks of the screen around it.
pub enum Action {
    Close,
}

const DOMAINS: [&str; 3] = ["Trading", "Coding", "General"];

pub fn show(
    ui: &mut Ui,
    chat: &Arc<TradingChat>,
    view: &mut ChatView,
    snapshot: &Snapshot,
) -> Option<Action> {
    let state = chat.snapshot();
    let mut action = None;

    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);

    // The header.
    ui.horizontal(|ui| {
        ui.set_min_height(widgets::HEIGHT + 8.0);

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(
                RichText::new(if view.show_memory { "Memory" } else { "Chat" })
                    .font(theme::strong(15.0))
                    .color(palette::text()),
            );
            ui.label(
                RichText::new(&state.model)
                    .font(theme::small())
                    .color(palette::text_muted()),
            );
        });

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            if widgets::icon_button(ui, Glyph::App(Icon::Close), "Close the chat  ⌘E", false)
                .clicked()
            {
                action = Some(Action::Close);
            }

            widgets::overflow(ui, "More", false, |ui| {
                if widgets::menu_item(
                    ui,
                    if view.show_memory {
                        "Back to the conversation"
                    } else {
                        "Search memory"
                    },
                    None,
                    None,
                    false,
                )
                .clicked()
                {
                    view.show_memory = !view.show_memory;
                }

                if widgets::menu_item(ui, "Network readings", None, None, view.show_readings)
                    .clicked()
                {
                    view.show_readings = !view.show_readings;
                }

                if !state.sessions.is_empty() {
                    widgets::menu_separator(ui);
                    widgets::caption(ui, "Earlier conversations");

                    for (at, session) in state.sessions.iter().enumerate() {
                        let name = format!(
                            "Conversation {} · {}",
                            at + 1,
                            session.chars().rev().take(6).collect::<String>()
                        );

                        if widgets::menu_item(ui, &name, None, None, session == &state.session)
                            .clicked()
                            && !state.running
                        {
                            chat.open(session.clone());
                        }
                    }
                }
            });

            if widgets::icon_button_kind(
                ui,
                Glyph::App(Icon::Plus),
                "New conversation",
                false,
                Kind::Ghost,
                !state.running,
            )
            .clicked()
            {
                chat.new_session();
                view.show_memory = false;
            }
        });
    });

    separator(ui);

    if view.show_readings {
        readings(ui, chat, snapshot);
    }

    if let Some(error) = &state.error {
        widgets::banner(ui, error, palette::bad());
    }

    // The middle: whatever is left once the composer has its room.
    let middle = (ui.available_height() - view.composer_height - 8.0).max(80.0);

    if view.show_memory {
        memory(ui, chat, view, &state, middle);
    } else {
        conversation(ui, chat, &state, middle);
    }

    // The composer, at the bottom.
    let top = ui.cursor().top();
    composer(ui, chat, view, &state, snapshot);
    view.composer_height = ui.cursor().top() - top;

    action
}

fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, palette::border()));
}

/// What the networks currently read, for the one watched.
fn readings(ui: &mut Ui, chat: &Arc<TradingChat>, snapshot: &Snapshot) {
    let source = snapshot
        .source
        .as_ref()
        .map(Source::key)
        .unwrap_or_else(|| "*".into());
    let reading = chat.learning.reading(
        Domain::Trading,
        &format!("{source}:{}", snapshot.interval.label()),
    );

    egui::Frame::new()
        .fill(palette::surface())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;

            widgets::caption(ui, "Network readings");

            let row = |ui: &mut Ui, name: &str, value: String| {
                super::panel::row(ui, name, &value, palette::text());
            };

            for (name, candles) in &snapshot.networks {
                if name == "ENS" {
                    continue;
                }

                if let Some(candle) = candles.first() {
                    row(
                        ui,
                        &format!("{name} next close"),
                        format!(
                            "{:.2}  ({:.2}–{:.2})",
                            candle.close, candle.close_low, candle.close_high
                        ),
                    );
                }
            }

            row(
                ui,
                "DANN",
                format!(
                    "{} neurons · {} branches · {} synapses",
                    reading.neurons, reading.branches, reading.synapses
                ),
            );
            row(
                ui,
                "SNN",
                format!("{} neurons · {} spikes", reading.snn_neurons, reading.spikes),
            );
            row(
                ui,
                "Seen",
                format!(
                    "{} · {} replayed · gen {}",
                    reading.observed, reading.replays, reading.generation
                ),
            );

            if reading.scored > 0 {
                let mean = |at: usize| reading.mean_errors[at] / reading.scored as f64;

                row(
                    ui,
                    "Next-state MSE",
                    format!(
                        "LNN {:.4} · DANN {:.4} · SNN {:.4}",
                        mean(0),
                        mean(1),
                        mean(2)
                    ),
                );
                row(
                    ui,
                    "Persistence MSE",
                    format!("{:.4}", reading.baseline_error / reading.scored as f64),
                );
            }

            for (label, confidence) in &reading.expectations {
                row(ui, "Expects", format!("{label} · {:.0}%", confidence * 100.0));
            }

            if !reading.imagined.is_empty() {
                egui::CollapsingHeader::new(
                    RichText::new("Imagined scenarios").font(theme::small()),
                )
                .show(ui, |ui| {
                    for thought in &reading.imagined {
                        ui.label(
                            RichText::new(thought)
                                .font(theme::small())
                                .color(palette::text_secondary()),
                        );
                    }
                });
            }

            if let Some(problem) = chat.learning.problem() {
                ui.label(RichText::new(problem).font(theme::small()).color(palette::bad()));
            }
        });
}

fn conversation(
    ui: &mut Ui,
    chat: &Arc<TradingChat>,
    state: &cortex_market::chat::ChatSnapshot,
    height: f32,
) {
    egui::ScrollArea::vertical()
        .id_salt("trading conversation")
        .stick_to_bottom(true)
        .auto_shrink(false)
        .max_height(height)
        .min_scrolled_height(height)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;

            if state.messages.is_empty() && !state.running {
                ui.add_space((height * 0.35).max(16.0));
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Ask about the chart, order flow, signals, or past experience.")
                            .font(theme::ui())
                            .color(palette::text_muted()),
                    );
                });

                return;
            }

            ui.add_space(8.0);

            for message in &state.messages {
                match message.role.as_str() {
                    "user" => bubble(ui, &message.text),
                    role => {
                        let (name, colour) = match role {
                            "assistant" => ("Cortex", palette::text_muted()),
                            "cancelled" => ("Stopped · partial answer", palette::warning()),
                            _ => ("Request error", palette::bad()),
                        };

                        ui.label(RichText::new(name).font(theme::strong(12.0)).color(colour));

                        let prepared =
                            prose::prepare(ui.ctx(), &message.text, ui.available_width());
                        prose::paint(ui, &prepared);

                        if role == "assistant" {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;

                                if small(ui, "Correct").clicked() {
                                    chat.feedback(message.id, true);
                                }

                                if small(ui, "Incorrect").clicked() {
                                    chat.feedback(message.id, false);
                                }
                            });
                        }
                    }
                }

                ui.add_space(8.0);
            }

            if state.running {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new("Cortex")
                            .font(theme::strong(12.0))
                            .color(palette::text_muted()),
                    );
                });

                let prepared = prose::prepare(ui.ctx(), &state.streaming, ui.available_width());
                prose::paint(ui, &prepared);

                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        });
}

/// A message of the user's, on the right on a raised ground.
fn bubble(ui: &mut Ui, text: &str) {
    let most = (ui.available_width() * 0.85).max(120.0);

    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        egui::Frame::new()
            .fill(palette::raised())
            .corner_radius(CornerRadius {
                nw: 12,
                ne: 12,
                sw: 12,
                se: 4,
            })
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_max_width(most - 24.0);
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(text).font(theme::body()).color(palette::text()),
                        )
                        .wrap(),
                    );
                });
            });
    });
}

/// A quiet text button under an answer.
fn small(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .font(theme::small())
                .color(palette::text_muted()),
        )
        .frame(false)
        .min_size(vec2(0.0, 24.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn memory(
    ui: &mut Ui,
    chat: &Arc<TradingChat>,
    view: &mut ChatView,
    state: &cortex_market::chat::ChatSnapshot,
    height: f32,
) {
    let top = ui.cursor().top();

    ui.horizontal(|ui| {
        let field = ui.add(
            egui::TextEdit::singleline(&mut view.memory_query)
                .hint_text("Search long-term memory")
                .margin(egui::Margin::symmetric(12, 8))
                .desired_width((ui.available_width() - widgets::HEIGHT - 8.0).max(80.0)),
        );

        let enter = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));

        if widgets::icon_button_kind(
            ui,
            Glyph::Send,
            "Search",
            false,
            Kind::Secondary,
            !view.memory_query.trim().is_empty(),
        )
        .clicked()
            || enter
        {
            chat.search(view.memory_query.clone(), domain(view.domain));
        }
    });

    let rest = (height - (ui.cursor().top() - top)).max(60.0);

    egui::ScrollArea::vertical()
        .id_salt("trading memories")
        .auto_shrink(false)
        .max_height(rest)
        .min_scrolled_height(rest)
        .show(ui, |ui| {
            if state.memories.is_empty() {
                ui.add_space(16.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No memories found").color(palette::text_muted()));
                });
            }

            for memory in &state.memories {
                egui::Frame::new()
                    .fill(palette::surface())
                    .stroke(Stroke::new(1.0, palette::border()))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 4.0;

                        ui.label(
                            RichText::new(format!(
                                "#{} · {} · {}",
                                memory.id,
                                memory.domain.label(),
                                memory.kind
                            ))
                            .font(theme::strong(12.0))
                            .color(palette::text_secondary()),
                        );
                        ui.label(
                            RichText::new(format!("{} · {}", memory.scope, memory.available_ms))
                                .font(theme::small())
                                .color(palette::text_muted()),
                        );
                        ui.label(RichText::new(&memory.text).color(palette::text()));

                        ui.horizontal(|ui| {
                            if small(ui, "Discuss this").clicked() {
                                view.draft =
                                    format!("Explain memory #{} and how it applies now", memory.id);
                                view.show_memory = false;
                                view.focus = true;
                            }
                        });

                        egui::CollapsingHeader::new(
                            RichText::new("Source context").font(theme::small()),
                        )
                        .id_salt(("memory source", memory.id))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(
                                    serde_json::to_string_pretty(&memory.evidence)
                                        .unwrap_or_default(),
                                )
                                .font(theme::mono())
                                .color(palette::text_secondary()),
                            );
                        });
                    });

                ui.add_space(8.0);
            }
        });
}

fn composer(
    ui: &mut Ui,
    chat: &Arc<TradingChat>,
    view: &mut ChatView,
    state: &cortex_market::chat::ChatSnapshot,
    snapshot: &Snapshot,
) {
    // How fresh the context of the last answer is: data, so kept, small.
    if state.context_ms > 0 {
        let age = (cortex_market::clock::now_ms() - state.context_ms) / 1000;
        let advanced = snapshot.revision > state.context_revision;

        ui.label(
            RichText::new(format!(
                "Answer context {age}s old{}",
                if advanced { " · screen has moved on" } else { "" }
            ))
            .font(theme::small())
            .color(if advanced {
                palette::warning()
            } else {
                palette::text_muted()
            }),
        );
    }

    egui::Frame::new()
        .fill(palette::background())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin {
            left: 4,
            right: 8,
            top: 8,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            let response = ui.add(
                egui::TextEdit::multiline(&mut view.draft)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::symmetric(8, 4))
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .font(theme::body())
                    .hint_text("Ask Cortex…"),
            );

            if view.focus {
                response.request_focus();
                view.focus = false;
            }

            let enter = response.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;

                // Where the answer looks: a drop-up, the composer being
                // at the bottom.
                widgets::dropdown(
                    ui,
                    DOMAINS[view.domain.min(2)],
                    None,
                    Kind::Ghost,
                    false,
                    true,
                    |ui| {
                        ui.set_min_width(160.0);

                        for (at, name) in DOMAINS.into_iter().enumerate() {
                            if widgets::menu_item(ui, name, None, None, view.domain == at)
                                .clicked()
                            {
                                view.domain = at;
                            }
                        }
                    },
                )
                .0
                .on_hover_text("Which memories the answer draws on");

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if state.running {
                        if widgets::icon_button_kind(
                            ui,
                            Glyph::Stop,
                            "Stop",
                            false,
                            Kind::Secondary,
                            true,
                        )
                        .clicked()
                        {
                            chat.cancel();
                        }
                    } else {
                        let ready = !state.loading && !view.draft.trim().is_empty();

                        if widgets::icon_button_kind(
                            ui,
                            Glyph::Send,
                            "Send  ↵",
                            false,
                            Kind::Primary,
                            ready,
                        )
                        .clicked()
                            || (enter && ready)
                        {
                            let text = std::mem::take(&mut view.draft);
                            chat.submit(text.trim(), domain(view.domain));
                            response.request_focus();
                        }
                    }

                    if widgets::icon_button_kind(
                        ui,
                        Glyph::Refresh,
                        "Refresh insights: what changed on screen and in the networks",
                        false,
                        Kind::Ghost,
                        !state.loading && !state.running,
                    )
                    .clicked()
                    {
                        chat.submit(
                            "Refresh the current screen and neural signals; explain what changed and any invalidation conditions.",
                            Domain::Trading,
                        );
                    }
                });
            });
        });
}

fn domain(index: usize) -> Domain {
    match index {
        1 => Domain::Coding,
        2 => Domain::General,
        _ => Domain::Trading,
    }
}

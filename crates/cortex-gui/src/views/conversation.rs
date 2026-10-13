//! The conversation: what was asked, what the agent said and did.

use std::time::Instant;

use cortex_core::{ApprovalRequest, CheckOutcome, Image, Outcome, Requirement, Usage};
use cortex_providers::pricing;
use eframe::egui::{self, Align, CornerRadius, Layout, RichText, ScrollArea, Sense, Stroke, Ui};

use super::icons::{self, Icon};
use super::prose::{self, code_block};
use super::tool_card;
use crate::attachment;
use crate::engine::Decision;
use crate::format;
use crate::session::{Activity, Doing, Item, Session, Tone, Writing};
use crate::theme::{self, palette};

/// Narrowest margin kept on either side of the column.
const MIN_MARGIN: f32 = 20.0;

const MAX_CHECK_OUTPUT_HEIGHT: f32 = 220.0;

/// How much of the column what the person said may take, so that it
/// reads as theirs: set to the right, with room left of it.
const USER_SHARE: f32 = 0.78;

/// Largest a sent picture is drawn.
const PICTURE_SIZE: egui::Vec2 = egui::vec2(220.0, 160.0);

/// Room kept right of what the agent says for its copy button.
const GUTTER: f32 = 28.0;

/// Most of the reasoning shown while it is still arriving, in characters:
/// the newest of it.
const LIVE_THINKING_CHARS: usize = 420;

/// Draws `session`. `approval` is a question waiting for an answer; the
/// return value is the answer, once given. `show_thinking` is whether the
/// model's reasoning is drawn.
pub fn show(
    ui: &mut Ui,
    session: &mut Session,
    approval: Option<&ApprovalRequest>,
    show_thinking: bool,
) -> Option<Decision> {
    if session.items.is_empty() {
        empty_state(ui);

        return None;
    }

    let mut decision = None;

    ScrollArea::vertical()
        .auto_shrink(false)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            let width = ui.available_width();
            let column = (width - 2.0 * MIN_MARGIN).clamp(40.0, theme::COLUMN_WIDTH);

            ui.horizontal(|ui| {
                ui.add_space((width - column) / 2.0);

                ui.vertical(|ui| {
                    ui.set_width(column);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add_space(12.0);

                    let model = session.model.clone().unwrap_or_default();
                    let running = session.running;

                    let items = &mut session.items;
                    let mut index = 0;

                    while index < items.len() {
                        let end = activity_end(items, index);

                        let briefed = briefings_end(items, index);

                        if briefed > index {
                            ui.push_id(index, |ui| briefings(ui, &mut items[index..briefed]));

                            index = briefed;
                        } else if end > index {
                            // Only the run of calls at the very end can
                            // still be growing.
                            let live = running && end == items.len();

                            ui.push_id(index, |ui| {
                                tool_card::show_group(
                                    ui,
                                    &mut items[index..end],
                                    live,
                                    show_thinking,
                                );
                            });

                            index = end;
                        } else {
                            ui.push_id(index, |ui| {
                                show_item(ui, &mut items[index], &model, show_thinking);
                            });

                            index += 1;
                        }
                    }

                    if let Some(writing) = &session.writing {
                        writing_message(ui, writing, show_thinking);
                    }

                    if let Some(doing) = session.doing.as_ref().filter(|_| running) {
                        doing_line(ui, doing, &model);
                    }

                    if let Some(request) = approval {
                        decision = approval_card(ui, request);
                    }

                    ui.add_space(18.0);
                });
            });
        });

    decision
}

/// Where the run of tool calls starting at `start` ends: the calls and
/// the model steps between them that said nothing. `start` itself when no
/// call begins there.
fn activity_end(items: &[Item], start: usize) -> usize {
    let mut end = start;

    for (index, item) in items.iter().enumerate().skip(start) {
        match item {
            Item::Tool(_) => end = index + 1,
            Item::Assistant { text, .. } if text.is_empty() => {}

            // Not drawn, so it does not split a run of calls in two.
            Item::Notice {
                tone: Tone::Housekeeping,
                ..
            } => {}
            _ => break,
        }
    }

    end
}

/// Where the run of briefings starting at `start` ends.
fn briefings_end(items: &[Item], start: usize) -> usize {
    items[start..]
        .iter()
        .position(|item| !matches!(item, Item::Briefing { .. }))
        .map_or(items.len(), |length| start + length)
}

/// The context put in front of an instruction, as one quiet line naming
/// where each piece came from. A name unfolds into what it brought.
fn briefings(ui: &mut Ui, items: &mut [Item]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        ui.label(
            RichText::new("Context")
                .font(theme::small())
                .color(palette::text_muted()),
        );

        for item in items.iter_mut() {
            let Item::Briefing { source, open, .. } = item else {
                continue;
            };

            let color = if *open {
                palette::text()
            } else {
                palette::text_secondary()
            };

            let name = RichText::new(source.replace('_', " "))
                .font(theme::small())
                .color(color);

            let response = ui
                .add(egui::Label::new(name).sense(Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("What the engine showed the model before your message");

            if response.clicked() {
                *open = !*open;
            }
        }
    });

    for item in items.iter() {
        if let Item::Briefing {
            text, open: true, ..
        } = item
        {
            code_block(ui, text, MAX_CHECK_OUTPUT_HEIGHT);
        }
    }
}

fn empty_state(ui: &mut Ui) {
    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        ui.add_space(ui.available_height() * 0.28);

        ui.label(RichText::new("A workspace for your next idea").font(theme::strong(22.0)));

        ui.add_space(2.0);

        ui.label(
            RichText::new("Ask a question, explore your code, or give an agent a task.")
                .color(palette::text_muted()),
        );
    });
}

fn show_item(ui: &mut Ui, item: &mut Item, model: &str, show_thinking: bool) {
    match item {
        Item::User { text, images } => user_message(ui, text, images),

        // Drawn in runs, by `briefings`.
        Item::Briefing { .. } => {}

        // A step that only called tools has nothing to say; its calls
        // are drawn as a table.
        Item::Assistant { text, thinking, .. } => {
            if !text.is_empty() {
                if show_thinking && !thinking.is_empty() {
                    thought(ui, thinking);
                }

                agent_message(ui, text);
            }
        }

        // Calls are drawn in runs, by `tool_card::show_group`.
        Item::Tool(_) => {}

        Item::Checks {
            passed,
            checks,
            open,
        } => checks_card(ui, *passed, checks, open),

        Item::Review {
            requirements,
            sent_back,
            open,
        } => review_card(ui, requirements, *sent_back, open),

        Item::Notice { text, tone } => {
            let color = match tone {
                Tone::Muted => palette::text_muted(),
                Tone::Bad => palette::bad(),
                Tone::Housekeeping => return,
            };

            ui.label(
                RichText::new(text.as_str())
                    .font(theme::small())
                    .color(color),
            );
        }

        Item::RunSummary {
            outcome,
            verified,
            steps,
            usage,
            duration_ms,
        } => run_summary(ui, *outcome, *verified, *steps, usage, *duration_ms, model),
    }
}

/// What the person said: a bubble set to the right, as wide as its text
/// and no wider than its share of the column, under any pictures sent
/// with it.
fn user_message(ui: &mut Ui, text: &str, images: &[Image]) {
    ui.add_space(10.0);

    if !images.is_empty() {
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;

            // Laid out from the right, so the last is placed first.
            for image in images.iter().rev() {
                picture(ui, image);
            }
        });
    }

    let padding = egui::vec2(12.0, 7.0);
    let widest = (ui.available_width() * USER_SHARE - 2.0 * padding.x).max(40.0);

    let galley = ui.painter().layout(
        text.to_string(),
        egui::FontId::proportional(14.0),
        palette::text(),
        widest,
    );

    let size = galley.size() + 2.0 * padding;

    let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), size.y), Sense::hover());

    let bubble = egui::Rect::from_min_size(egui::pos2(row.right() - size.x, row.top()), size);

    ui.painter()
        .rect_filled(bubble, CornerRadius::same(12), palette::raised());

    // A label rather than painted text, so that it can be selected.
    let inside = egui::UiBuilder::new()
        .max_rect(bubble.shrink2(padding))
        .layout(Layout::top_down(Align::Min));

    ui.scope_builder(inside, |ui| {
        ui.add(
            egui::Label::new(RichText::new(text).size(14.0).color(palette::text()))
                .selectable(true)
                .wrap(),
        );
    });

    if ui.rect_contains_pointer(row) {
        let place = egui::Rect::from_center_size(
            egui::pos2(bubble.left() - 16.0, bubble.top() + 15.0),
            egui::vec2(22.0, 22.0),
        );

        copy_button(ui, place, text);
    }

    ui.add_space(4.0);
}

/// A picture that was sent, scaled to fit and kept in its proportions.
fn picture(ui: &mut Ui, image: &Image) {
    let Some(texture) = attachment::texture(ui.ctx(), image) else {
        ui.label(
            RichText::new("A picture that cannot be shown")
                .font(theme::small())
                .color(palette::text_muted()),
        );

        return;
    };

    let natural = texture.size_vec2();
    let scale = (PICTURE_SIZE.x / natural.x)
        .min(PICTURE_SIZE.y / natural.y)
        .min(1.0);

    let (rect, _) = ui.allocate_exact_size(natural * scale, Sense::hover());

    attachment::paint_cover(ui, rect, &texture, 10);

    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(10),
        Stroke::new(1.0, palette::border()),
        egui::StrokeKind::Inside,
    );
}

/// What the agent said: plain text on the left, with no frame around it.
/// A copy button shows in the gutter while the pointer is over it.
fn agent_message(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);

    let written = ui
        .scope(|ui| {
            ui.set_max_width(ui.available_width() - GUTTER);

            prose::show(ui, text, egui::FontId::proportional(14.0));
        })
        .response
        .rect;

    let row = written.with_max_x(written.left() + ui.available_width());

    if ui.rect_contains_pointer(row) {
        let place = egui::Rect::from_min_size(
            egui::pos2(row.right() - 22.0, row.top()),
            egui::vec2(22.0, 22.0),
        );

        copy_button(ui, place, text);
    }

    ui.add_space(4.0);
}

fn copy_button(ui: &Ui, place: egui::Rect, text: &str) {
    let id = ui.id().with("copy");

    if icons::button_at(ui, place, id, Icon::Copy, "Copy message", false).clicked() {
        ui.ctx().copy_text(text.to_string());
    }
}

/// The response the model is writing right now. Until its text begins,
/// the newest of its reasoning is shown as it arrives; after that the
/// reasoning folds to a line.
fn writing_message(ui: &mut Ui, writing: &Writing, show_thinking: bool) {
    if show_thinking && !writing.thinking.is_empty() {
        if writing.text.is_empty() {
            ui.label(
                RichText::new("Thinking…")
                    .size(13.0)
                    .color(palette::text_secondary()),
            );

            thinking_text(ui, &newest(&writing.thinking, LIVE_THINKING_CHARS));
        } else {
            thought(ui, &writing.thinking);
        }
    }

    if !writing.text.is_empty() {
        ui.add_space(4.0);

        ui.scope(|ui| {
            ui.set_max_width(ui.available_width() - GUTTER);

            prose::show(ui, &writing.text, egui::FontId::proportional(14.0));
        });
    }
}

/// One quiet line saying what the run is waiting on and for how long, so
/// that a slow model or a rate limit does not look like nothing at all.
fn doing_line(ui: &mut Ui, doing: &Doing, model: &str) {
    let waited = doing.since.elapsed().as_secs();

    let text = match &doing.activity {
        Activity::Preparing => "Looking up memory and relevant code".to_string(),
        Activity::Waiting => format!("Waiting for {model}"),

        Activity::Retrying { reason, until } => {
            let left = until.saturating_duration_since(Instant::now()).as_secs();

            format!("{reason}. Asking again in {left}s")
        }

        Activity::Calling(tool) => format!("Writing a call to {}", tool.replace('_', " ")),
        Activity::Attempting => {
            "An attempt is under way; its steps are shown if it is kept".to_string()
        }
        Activity::Finishing => "Checking the work".to_string(),
    };

    // The first second or two is not worth counting.
    let text = match waited {
        0..=1 => format!("{text}…"),
        _ => format!("{text}… {}", format::duration(waited * 1000)),
    };

    // Never so faint that the line cannot be read.
    let pulse = 0.8 + 0.2 * (ui.input(|input| input.time) * 3.0).sin() as f32;

    ui.label(
        RichText::new(text)
            .size(13.0)
            .color(palette::text_secondary().gamma_multiply(pulse)),
    );
}

/// The end of `text`, from a word boundary, with a mark that more came
/// before it.
fn newest(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();

    if count <= max_chars {
        return text.to_string();
    }

    let tail: String = text.chars().skip(count - max_chars).collect();

    let from_word = tail
        .find(' ')
        .map_or(tail.as_str(), |space| &tail[space + 1..]);

    format!("… {from_word}")
}

/// A line that unfolds into the model's reasoning for a step.
fn thought(ui: &mut Ui, thinking: &str) {
    let id = ui.id().with("thought");

    let open: bool = ui.data(|data| data.get_temp(id)).unwrap_or(false);

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), Sense::click());

    let color = if response.hovered() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    let painter = ui.painter_at(rect);

    let label = painter.text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        "Thought",
        egui::FontId::proportional(13.0),
        color,
    );

    tool_card::chevron(
        &painter,
        egui::pos2(label.right() + 11.0, rect.center().y),
        open,
        color,
    );

    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        ui.data_mut(|data| data.insert_temp(id, !open));
    }

    if open {
        thinking_text(ui, thinking);
    }
}

/// Reasoning, set apart from what the model says by a rule down its left
/// side and a quieter voice.
pub fn thinking_text(ui: &mut Ui, thinking: &str) {
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 0,
            top: 0,
            bottom: 2,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            ui.label(
                RichText::new(thinking.trim())
                    .font(theme::small())
                    .color(palette::text_muted()),
            );
        });

    let rect = inner.response.rect;

    ui.painter().vline(
        rect.left() + 2.0,
        rect.y_range(),
        Stroke::new(1.5, palette::border()),
    );
}

fn checks_card(ui: &mut Ui, passed: bool, checks: &[CheckOutcome], open: &mut bool) {
    let (title, color) = if passed {
        ("The project's checks pass", palette::good())
    } else {
        ("The project's checks fail", palette::bad())
    };

    foldable(ui, title, color, open, |ui| {
        for check in checks {
            let mark = if check.passed { "passed" } else { "failed" };

            ui.label(
                RichText::new(format!(
                    "{} · {mark} · {}",
                    check.command,
                    format::duration(check.duration_ms)
                ))
                .font(theme::mono())
                .color(palette::text_secondary()),
            );

            if !check.output.is_empty() {
                code_block(ui, &check.output, MAX_CHECK_OUTPUT_HEIGHT);
            }
        }
    });
}

/// What the review made of the finished work: each thing the request
/// asks for, and whether the changes do it.
fn review_card(ui: &mut Ui, requirements: &[Requirement], sent_back: bool, open: &mut bool) {
    let met = requirements
        .iter()
        .filter(|requirement| requirement.met)
        .count();

    let title = if sent_back {
        format!(
            "Review: {met} of {} requirements met, sent back to the model",
            requirements.len()
        )
    } else {
        format!("Review: all {} requirements met", requirements.len())
    };

    let color = if sent_back {
        palette::bad()
    } else {
        palette::good()
    };

    foldable(ui, &title, color, open, |ui| {
        for requirement in requirements {
            let (mark, color) = if requirement.met {
                ("met", palette::text_secondary())
            } else {
                ("not met", palette::bad())
            };

            ui.label(RichText::new(format!("{mark} · {}", requirement.text)).color(color));

            if !requirement.why.is_empty() {
                ui.label(
                    RichText::new(requirement.why.as_str())
                        .font(theme::small())
                        .color(palette::text_muted()),
                );
            }
        }
    });
}

/// A one-line row with a title that unfolds when clicked.
fn foldable(
    ui: &mut Ui,
    title: &str,
    color: egui::Color32,
    open: &mut bool,
    contents: impl FnOnce(&mut Ui),
) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), Sense::click());
    let painter = ui.painter_at(rect);
    let color = if response.hovered() {
        palette::text()
    } else {
        color
    };
    tool_card::chevron(
        &painter,
        rect.left_center() + egui::vec2(5.0, 0.0),
        *open,
        color,
    );
    painter.text(
        rect.left_center() + egui::vec2(18.0, 0.0),
        egui::Align2::LEFT_CENTER,
        title,
        theme::small(),
        color,
    );
    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        *open = !*open;
    }
    if *open {
        ui.indent("fold contents", contents);
    }
}

fn run_summary(
    ui: &mut Ui,
    outcome: Outcome,
    verified: Option<bool>,
    steps: u32,
    usage: &Usage,
    duration_ms: u64,
    model: &str,
) {
    let (ending, color) = match (outcome, verified) {
        (Outcome::Completed, Some(true)) => ("Finished and verified", palette::good()),
        (Outcome::Completed, Some(false)) => ("Finished, checks still failing", palette::warning()),
        (Outcome::Completed, None) => ("Finished", palette::good()),
        (Outcome::StepLimit, _) => ("Stopped at the step limit", palette::warning()),
        (Outcome::Truncated, _) => ("Stopped: the response was cut off", palette::warning()),
        (Outcome::Refused, _) => ("Stopped: the model declined", palette::bad()),
    };

    let mut facts = vec![
        format!("{steps} steps"),
        format!("{} in", format::count(usage.total_input())),
        format!("{} out", format::count(usage.output_tokens)),
        format::duration(duration_ms),
    ];

    if let Some(cost) = pricing::cost_usd(model, usage) {
        facts.push(format!("about ${cost:.4}"));
    }

    ui.add_space(2.0);

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(ending).font(theme::small()).color(color));

        ui.label(
            RichText::new(facts.join(" · "))
                .font(theme::small())
                .color(palette::text_muted()),
        );
    });

    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 9.0), Sense::hover());

    ui.painter().hline(
        line.x_range(),
        line.center().y,
        Stroke::new(1.0, palette::border()),
    );
}

pub fn approval_card(ui: &mut Ui, request: &ApprovalRequest) -> Option<Decision> {
    let mut decision = None;

    theme::card()
        .stroke(Stroke::new(1.0, palette::accent().gamma_multiply(0.7)))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            ui.label(format!("Allow {}?", request.tool));

            code_block(ui, &request.summary, 120.0);

            ui.horizontal(|ui| {
                let allow = egui::Button::new(RichText::new("Allow").color(palette::sunken()))
                    .fill(palette::accent());

                if ui.add(allow).clicked() {
                    decision = Some(Decision::Allow);
                }

                if ui.button("Always allow").clicked() {
                    decision = Some(Decision::AlwaysAllow);
                }

                if ui.button("Deny").clicked() {
                    decision = Some(Decision::Deny);
                }
            });
        });

    decision
}

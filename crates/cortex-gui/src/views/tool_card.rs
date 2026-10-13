//! Tool calls: a run of them as one line that unfolds into a table, each
//! row unfolding into what was asked and what came back.

use eframe::egui::{self, Align2, CornerRadius, Pos2, RichText, Sense, Stroke, Ui, vec2};
use serde_json::Value;

use super::prose::code_block;
use crate::format;
use crate::session::{Item, ToolItem};
use crate::theme::{self, palette};

/// Input fields worth showing on the folded card, in order of preference.
const SUMMARY_FIELDS: [&str; 5] = ["command", "path", "pattern", "query", "symbol"];

/// Longest output shown in the unfolded card, in characters.
const MAX_OUTPUT_CHARS: usize = 6_000;

const MAX_OUTPUT_HEIGHT: f32 = 260.0;

/// Height of the line that folds a run of activity, and of each row in
/// its table.
const HEADER_HEIGHT: f32 = 24.0;
const ROW_HEIGHT: f32 = 28.0;

/// Width given to a character when cutting text to fit a row.
const CHAR_WIDTH: f32 = 6.9;

const TABLE_RADIUS: u8 = 8;

/// Draws a run of consecutive tool calls as one line that unfolds into a
/// table, a row per call; a row unfolds into the call's input and output.
///
/// `items` are the calls and the wordless model steps between them.
/// `live` is whether the run is still adding to them: the table is open
/// while it is and folds away afterwards, unless it was toggled by hand.
///
/// With `show_thinking`, a step's reasoning is a row of its own above the
/// calls it led to.
pub fn show_group(ui: &mut Ui, items: &mut [Item], live: bool, show_thinking: bool) {
    let id = ui.id().with("activity");

    let chosen: Option<bool> = ui.data(|data| data.get_temp(id));
    let open = chosen.unwrap_or(live);

    if group_header(ui, items, live, open).clicked() {
        ui.data_mut(|data| data.insert_temp(id, !open));
    }

    if !open {
        return;
    }

    egui::Frame::new()
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(TABLE_RADIUS))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;

            let mut first = true;

            for (index, item) in items.iter_mut().enumerate() {
                let thought = match item {
                    Item::Tool(_) => None,

                    Item::Assistant { thinking, .. } if show_thinking && !thinking.is_empty() => {
                        Some(thinking.clone())
                    }

                    _ => continue,
                };

                if !std::mem::take(&mut first) {
                    let (line, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());

                    ui.painter().hline(
                        line.x_range(),
                        line.center().y,
                        Stroke::new(1.0, palette::border()),
                    );
                }

                ui.push_id(index, |ui| match (item, thought) {
                    (Item::Tool(tool), _) => row(ui, tool),
                    (_, Some(thinking)) => thought_row(ui, &thinking),
                    _ => {}
                });
            }
        });
}

/// The line above the table: what the calls amount to, how long they
/// took, and an arrow that says whether the table is open.
fn group_header(ui: &mut Ui, items: &[Item], live: bool, open: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), HEADER_HEIGHT), Sense::click());

    let color = if response.hovered() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    let (title, failed, milliseconds) = digest(items, live);

    let painter = ui.painter_at(rect);
    let font = egui::FontId::proportional(13.0);

    let room = ((rect.width() - 150.0) / CHAR_WIDTH) as usize;

    let mut right = painter
        .text(
            rect.left_center(),
            Align2::LEFT_CENTER,
            format::one_line(&title, room),
            font.clone(),
            color,
        )
        .right();

    if failed > 0 {
        right = painter
            .text(
                Pos2::new(right + 8.0, rect.center().y),
                Align2::LEFT_CENTER,
                format!("{failed} failed"),
                theme::small(),
                palette::bad(),
            )
            .right();
    }

    if milliseconds > 0 {
        right = painter
            .text(
                Pos2::new(right + 8.0, rect.center().y),
                Align2::LEFT_CENTER,
                format::duration(milliseconds),
                theme::small(),
                palette::text_muted(),
            )
            .right();
    }

    chevron(
        &painter,
        Pos2::new(right + 12.0, rect.center().y),
        open,
        color,
    );

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One call: what it did to what, how it went, and an arrow to unfold it.
fn row(ui: &mut Ui, tool: &mut ToolItem) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());

    let painter = ui.painter_at(rect);

    if response.hovered() {
        painter.rect_filled(
            rect,
            CornerRadius::same(TABLE_RADIUS - 1),
            palette::surface(),
        );
    }

    let font = egui::FontId::proportional(13.0);

    let verb = painter.text(
        Pos2::new(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        verb(&tool.call.name, tool.result.is_none()),
        font.clone(),
        palette::text_muted(),
    );

    let (status, status_color) = status(tool);

    let status = painter.text(
        Pos2::new(rect.right() - 12.0, rect.center().y),
        Align2::RIGHT_CENTER,
        status,
        theme::small(),
        status_color,
    );

    let room = ((status.left() - verb.right() - 40.0) / CHAR_WIDTH).max(8.0) as usize;

    let target = painter.text(
        Pos2::new(verb.right() + 7.0, rect.center().y),
        Align2::LEFT_CENTER,
        format::one_line(&summary(&tool.call.input), room),
        font,
        palette::text(),
    );

    chevron(
        &painter,
        Pos2::new(target.right() + 11.0, rect.center().y),
        tool.open,
        palette::text_muted(),
    );

    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        tool.open = !tool.open;
    }

    if tool.open {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 2,
                bottom: 10,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 5.0;

                details(ui, tool);
            });
    }
}

/// A step's reasoning as a row of the table: its opening words, and the
/// whole of it when unfolded.
fn thought_row(ui: &mut Ui, thinking: &str) {
    let id = ui.id().with("thought");

    let open: bool = ui.data(|data| data.get_temp(id)).unwrap_or(false);

    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());

    let painter = ui.painter_at(rect);

    if response.hovered() {
        painter.rect_filled(
            rect,
            CornerRadius::same(TABLE_RADIUS - 1),
            palette::surface(),
        );
    }

    let font = egui::FontId::proportional(13.0);

    let verb = painter.text(
        Pos2::new(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Thought",
        font.clone(),
        palette::text_muted(),
    );

    let room = ((rect.right() - verb.right() - 60.0) / CHAR_WIDTH).max(8.0) as usize;

    let opening = painter.text(
        Pos2::new(verb.right() + 7.0, rect.center().y),
        Align2::LEFT_CENTER,
        format::one_line(thinking, room),
        font,
        palette::text_secondary(),
    );

    chevron(
        &painter,
        Pos2::new(opening.right() + 11.0, rect.center().y),
        open,
        palette::text_muted(),
    );

    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        ui.data_mut(|data| data.insert_temp(id, !open));
    }

    if open {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 2,
                bottom: 10,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                super::conversation::thinking_text(ui, thinking);
            });
    }
}

/// A small arrow: pointing down when open, right when folded.
pub fn chevron(painter: &egui::Painter, centre: Pos2, open: bool, color: egui::Color32) {
    let stroke = Stroke::new(1.2, color);

    let (a, b, c) = if open {
        (vec2(-3.5, -1.8), vec2(0.0, 1.8), vec2(3.5, -1.8))
    } else {
        (vec2(-1.8, -3.5), vec2(1.8, 0.0), vec2(-1.8, 3.5))
    };

    painter.line_segment([centre + a, centre + b], stroke);
    painter.line_segment([centre + b, centre + c], stroke);
}

/// What a tool does, as the first word of its row.
fn verb(tool: &str, running: bool) -> &'static str {
    let (done, doing) = match tool {
        "read_file" | "read_symbol" | "outline" => ("Read", "Reading"),
        "edit_file" => ("Edited", "Editing"),
        "write_file" => ("Wrote", "Writing"),
        "shell" => ("Ran", "Running"),
        "list_files" => ("Listed", "Listing"),
        "remember" => ("Remembered", "Remembering"),
        "recall" => ("Recalled", "Recalling"),
        "find_references" | "file_dependencies" => ("Traced", "Tracing"),
        _ => ("Searched", "Searching"),
    };

    if running { doing } else { done }
}

/// A run of calls in a few words, how many of them failed, and how long
/// the run took, model steps included.
fn digest(items: &[Item], live: bool) -> (String, usize, u64) {
    let tools: Vec<&ToolItem> = items
        .iter()
        .filter_map(|item| match item {
            Item::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();

    let milliseconds: u64 = items
        .iter()
        .map(|item| match item {
            Item::Tool(tool) => tool.duration_ms,
            Item::Assistant { duration_ms, .. } => *duration_ms,
            _ => 0,
        })
        .sum();

    let failed = tools
        .iter()
        .filter(|tool| tool.result.as_ref().is_some_and(|result| result.is_error))
        .count();

    // While the run goes on, the line says what it is doing now.
    if live && let Some(latest) = tools.last() {
        let title = format!(
            "{} {}",
            verb(&latest.call.name, true),
            summary(&latest.call.input)
        );

        return (title, failed, milliseconds);
    }

    let count = |names: &[&str]| {
        tools
            .iter()
            .filter(|tool| names.contains(&tool.call.name.as_str()))
            .count()
    };

    let read = count(&["read_file", "read_symbol", "outline"]);
    let changed = count(&["edit_file", "write_file"]);
    let ran = count(&["shell"]);
    let looked = tools.len() - read - changed - ran;

    let plural = |number: usize, one: &str, many: &str| match number {
        1 => format!("1 {one}"),
        number => format!("{number} {many}"),
    };

    let mut parts = Vec::new();

    if read > 0 {
        parts.push(format!("read {}", plural(read, "file", "files")));
    }

    if looked > 0 {
        parts.push(format!("searched {}", plural(looked, "time", "times")));
    }

    if changed > 0 {
        parts.push(format!("made {}", plural(changed, "edit", "edits")));
    }

    if ran > 0 {
        parts.push(format!("ran {}", plural(ran, "command", "commands")));
    }

    let mut title = parts.join(", ");

    if let Some(first) = title.get_mut(..1) {
        first.make_ascii_uppercase();
    }

    (title, failed, milliseconds)
}

fn details(ui: &mut Ui, tool: &ToolItem) {
    ui.add_space(2.0);

    section_label(ui, "Input");

    let input = serde_json::to_string_pretty(&tool.call.input)
        .unwrap_or_else(|_| tool.call.input.to_string());

    code_block(ui, &input, MAX_OUTPUT_HEIGHT);

    let Some(result) = &tool.result else {
        return;
    };

    section_label(ui, if result.is_error { "Error" } else { "Output" });

    let output = if result.output.is_empty() {
        "(no output)".to_string()
    } else {
        clip(&result.output)
    };

    code_block(ui, &output, MAX_OUTPUT_HEIGHT);
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::text_muted()),
    );
}

fn status(tool: &ToolItem) -> (String, egui::Color32) {
    match &tool.result {
        None => ("running…".to_string(), palette::warning()),

        Some(result) if result.is_error => ("failed".to_string(), palette::bad()),

        Some(_) => (format::duration(tool.duration_ms), palette::text_muted()),
    }
}

/// The one thing about the input most worth seeing at a glance.
fn summary(input: &Value) -> String {
    SUMMARY_FIELDS
        .iter()
        .find_map(|field| input.get(field)?.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| input.to_string())
}

fn clip(output: &str) -> String {
    if output.chars().count() <= MAX_OUTPUT_CHARS {
        return output.to_string();
    }

    let shown: String = output.chars().take(MAX_OUTPUT_CHARS).collect();
    let hidden = output.chars().count() - MAX_OUTPUT_CHARS;

    format!("{shown}\n\n… {hidden} more characters")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool(name: &str, input: Value, error: Option<bool>) -> Item {
        Item::Tool(ToolItem {
            call: cortex_core::ToolCall {
                id: "call".to_string(),
                name: name.to_string(),
                input,
            },
            result: error.map(|is_error| cortex_core::ToolResult {
                call_id: "call".to_string(),
                output: String::new(),
                is_error,
                images: Vec::new(),
            }),
            duration_ms: 1_000,
            open: false,
        })
    }

    #[test]
    fn a_finished_run_of_calls_is_summed_up() {
        let items = [
            tool("read_file", json!({ "path": "a.rs" }), Some(false)),
            tool("read_file", json!({ "path": "b.rs" }), Some(false)),
            Item::Assistant {
                step: 2,
                text: String::new(),
                thinking: String::new(),
                duration_ms: 500,
                speed: None,
            },
            tool("search", json!({ "pattern": "x" }), Some(false)),
            tool("edit_file", json!({ "path": "a.rs" }), Some(true)),
            tool("shell", json!({ "command": "cargo test" }), Some(false)),
        ];

        let (title, failed, milliseconds) = digest(&items, false);

        assert_eq!(
            title,
            "Read 2 files, searched 1 time, made 1 edit, ran 1 command"
        );
        assert_eq!(failed, 1);
        assert_eq!(milliseconds, 5_500);
    }

    #[test]
    fn a_run_still_going_says_what_it_is_doing() {
        let items = [
            tool("read_file", json!({ "path": "a.rs" }), Some(false)),
            tool("shell", json!({ "command": "cargo test" }), None),
        ];

        assert_eq!(digest(&items, true).0, "Running cargo test");
    }

    #[test]
    fn the_summary_prefers_the_telling_field() {
        assert_eq!(summary(&json!({ "path": "a.py", "offset": 3 })), "a.py");
        assert_eq!(summary(&json!({ "command": "ls", "path": "." })), "ls");
        assert_eq!(summary(&json!({ "depth": 2 })), r#"{"depth":2}"#);
    }
}

//! Showing the assistant's words: paragraphs, headings and code.
//!
//! This understands just enough Markdown for an agent's reports: fenced
//! code blocks, `#` headings, `inline code` and `**strong**` words.

use eframe::egui::text::LayoutJob;
use eframe::egui::{self, CornerRadius, FontId, RichText, TextFormat, Ui};

use crate::theme::{self, palette};

#[derive(Debug, PartialEq)]
enum Block {
    Paragraph(String),
    Heading(String),
    Code(String),
}

pub fn show(ui: &mut Ui, text: &str, font: FontId) {
    for block in parse(text) {
        match block {
            Block::Paragraph(text) => {
                ui.add(egui::Label::new(inline(&text, &font)).selectable(true));
            }

            Block::Heading(text) => {
                ui.add_space(2.0);

                ui.label(
                    RichText::new(text)
                        .font(theme::strong(font.size + 1.0))
                        .color(palette::TEXT),
                );
            }

            Block::Code(code) => code_block(ui, &code, f32::INFINITY),
        }
    }
}

/// Monospaced text on a sunken background, scrolling past `max_height`.
pub fn code_block(ui: &mut Ui, code: &str, max_height: f32) {
    egui::Frame::new()
        .fill(palette::SUNKEN)
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Code / output")
                        .font(theme::small())
                        .color(palette::TEXT_MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(code.to_string());
                    }
                });
            });
            egui::ScrollArea::vertical()
                .id_salt(ui.next_auto_id())
                .max_height(max_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(code)
                                .font(theme::mono())
                                .color(palette::TEXT_SECONDARY),
                        )
                        .selectable(true),
                    );
                });
        });
}

fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut code: Option<Vec<&str>> = None;

    let end_paragraph = |paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join("\n")));
            paragraph.clear();
        }
    };

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            match code.take() {
                Some(lines) => blocks.push(Block::Code(lines.join("\n"))),

                None => {
                    end_paragraph(&mut paragraph, &mut blocks);
                    code = Some(Vec::new());
                }
            }

            continue;
        }

        if let Some(lines) = code.as_mut() {
            lines.push(line);
        } else if line.trim().is_empty() {
            end_paragraph(&mut paragraph, &mut blocks);
        } else if let Some(heading) = heading(line) {
            end_paragraph(&mut paragraph, &mut blocks);
            // A heading is already set heavy, so its own emphasis is dropped.
            blocks.push(Block::Heading(heading.replace("**", "")));
        } else {
            paragraph.push(line);
        }
    }

    end_paragraph(&mut paragraph, &mut blocks);

    // A fence that was never closed still holds code worth showing.
    if let Some(lines) = code {
        blocks.push(Block::Code(lines.join("\n")));
    }

    blocks
}

fn heading(line: &str) -> Option<&str> {
    let title = line.trim_start_matches('#');

    let hashes = line.len() - title.len();

    ((1..=6).contains(&hashes) && title.starts_with(' ')).then(|| title.trim())
}

/// A paragraph with its `inline code` set in the monospaced font and its
/// `**strong**` words set heavy.
fn inline(text: &str, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();

    let line_height = Some(font.size * 1.45);

    let plain = TextFormat {
        font_id: font.clone(),
        color: palette::TEXT,
        line_height,
        ..TextFormat::default()
    };

    // Monospaced letters look larger than a serif's at the same size, so
    // code set inside a sentence is taken down a little to sit level.
    let code = TextFormat {
        font_id: FontId::monospace(font.size * 0.84),
        color: palette::CODE,
        background: palette::SURFACE,
        line_height,
        ..TextFormat::default()
    };

    let strong = TextFormat {
        font_id: theme::strong(font.size),
        ..plain.clone()
    };

    for (piece, style) in spans(text) {
        let format = match style {
            Style::Plain => &plain,
            Style::Strong => &strong,
            Style::Code => &code,
        };

        job.append(piece, 0.0, format.clone());
    }

    job
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Style {
    Plain,
    Strong,
    Code,
}

/// The pieces of a paragraph and how each is set, with the marks that
/// said so left out.
fn spans(text: &str) -> Vec<(&str, Style)> {
    let mut spans = Vec::new();

    // A pair of asterisks may open before a piece of code and close after
    // it, so this is carried from one piece to the next. One that is never
    // closed, as while the words are still arriving, runs to the end.
    let mut strong = false;

    // Backticks come in pairs, so every second piece is code.
    for (index, piece) in text.split('`').enumerate() {
        if index % 2 == 1 {
            spans.push((piece, Style::Code));
            continue;
        }

        for (index, part) in piece.split("**").enumerate() {
            if index > 0 {
                strong = !strong;
            }

            if !part.is_empty() {
                spans.push((part, if strong { Style::Strong } else { Style::Plain }));
            }
        }
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_split_into_blocks() {
        let blocks =
            parse("Fixed it.\nTwo lines.\n\n## What changed\n\n```python\nx = 1\n```\nDone.");

        assert_eq!(
            blocks,
            [
                Block::Paragraph("Fixed it.\nTwo lines.".to_string()),
                Block::Heading("What changed".to_string()),
                Block::Code("x = 1".to_string()),
                Block::Paragraph("Done.".to_string()),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_keeps_its_code() {
        assert_eq!(parse("```\nx = 1"), [Block::Code("x = 1".to_string())]);
    }

    #[test]
    fn strong_words_lose_their_asterisks() {
        assert_eq!(
            spans("A **bold** word and `x ** y`."),
            [
                ("A ", Style::Plain),
                ("bold", Style::Strong),
                (" word and ", Style::Plain),
                ("x ** y", Style::Code),
                (".", Style::Plain),
            ]
        );
    }

    #[test]
    fn strong_words_may_hold_code() {
        assert_eq!(
            spans("**Run `make` now** then"),
            [
                ("Run ", Style::Strong),
                ("make", Style::Code),
                (" now", Style::Strong),
                (" then", Style::Plain),
            ]
        );
    }

    #[test]
    fn a_heading_drops_its_asterisks() {
        assert_eq!(
            parse("## **Summary**"),
            [Block::Heading("Summary".to_string())]
        );
    }

    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        assert_eq!(heading("#include <stdio.h>"), None);
        assert_eq!(heading("# Title"), Some("Title"));
    }
}

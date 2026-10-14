//! Showing the assistant's words: paragraphs, headings, lists, tables
//! and code.
//!
//! This understands the Markdown an agent's reports are written in:
//! fenced code blocks, `#` headings, lists, tables, quotes, rules,
//! `inline code`, `**strong**` and `*emphasised*` words, and links.
//!
//! Nothing here is worked out while drawing. A message is first
//! [prepared](prepare): split into blocks, each laid out to the width it
//! will have, through [`typeset`], which keeps the result. What comes
//! back knows its own height, and [painting](paint) it only places what
//! was laid out. A message that has not changed is therefore never read
//! or laid out a second time, and one that is off screen need not be
//! touched at all.
//!
//! A response that is still being written is handled by [`Stream`]: the
//! blocks that are complete are laid out once and kept, and only the
//! last, still growing one is laid out again as it grows.

use std::ops::Range;
use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    self, Align, CornerRadius, FontId, Galley, Layout, Pos2, Rect, Sense, Stroke, TextFormat, Ui,
    UiBuilder, vec2,
};

use super::icons::{self, Icon};
use super::typeset;
use crate::theme::{self, palette, radius, size, space};

/// Room between two blocks of a message.
const BLOCK_GAP: f32 = space::M;

/// Room above a heading that is not the first thing said.
const HEADING_GAP: f32 = space::L;

/// Room between two entries of a list.
const ENTRY_GAP: f32 = space::XS;

/// How far each level of a list is set in.
const LIST_INDENT: f32 = 18.0;

/// Room between an entry's mark and its text.
const MARKER_GAP: f32 = 7.0;

const CODE_PADDING: egui::Vec2 = vec2(12.0, 8.0);

/// The line above a block of code that names its language and holds its
/// copy button.
const CODE_HEADER: f32 = 22.0;

const CODE_HEADER_GAP: f32 = space::XS;

/// Code longer than this is shown in one colour: telling its words
/// apart would take longer than it is worth.
const MAX_TINTED_BYTES: usize = 60_000;

const CELL_PADDING: egui::Vec2 = vec2(10.0, 6.0);

/// A sentence with the letters of ordinary English, for finding how
/// wide a line of so many characters is.
const SAMPLE: &str = "the quick brown fox jumps over the lazy dog and runs ";

#[derive(Debug, Clone, PartialEq)]
enum Block {
    Paragraph(String),

    Heading {
        level: u8,
        text: String,
    },

    Code {
        language: String,
        code: String,
    },

    List(Vec<Entry>),
    Quote(String),

    /// Rows of cells; the first row is the header.
    Table(Vec<Vec<String>>),
    Rule,
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    /// How deeply it is nested, from 0.
    depth: u8,

    /// A bullet, or the entry's own number.
    marker: String,
    text: String,
}

/// Part of a paragraph that leads somewhere when clicked.
#[derive(Debug, Clone, PartialEq)]
struct Link {
    /// Which of the paragraph's characters, as they are drawn.
    chars: Range<usize>,
    url: String,
}

/// One block, laid out.
enum Laid {
    Text {
        galley: Arc<Galley>,
        links: Vec<Link>,
        indent: f32,

        /// A rule down the left, as beside a quotation.
        bar: bool,
    },

    Entry {
        marker: Arc<Galley>,
        galley: Arc<Galley>,
        links: Vec<Link>,
        indent: f32,
    },

    Code {
        galley: Arc<Galley>,
        label: String,
    },

    Table(Table),
    Rule,
}

struct Table {
    /// Every cell, row by row.
    cells: Vec<Vec<Arc<Galley>>>,

    /// The width of each column and the height of each row, padding
    /// included.
    columns: Vec<f32>,
    rows: Vec<f32>,
}

impl Table {
    fn size(&self) -> egui::Vec2 {
        vec2(self.columns.iter().sum(), self.rows.iter().sum())
    }
}

struct Placed {
    /// Room left above it.
    gap: f32,
    height: f32,
    laid: Laid,
}

/// A message ready to draw: its blocks laid out, and how tall they are
/// together.
#[derive(Default)]
pub struct Prepared {
    blocks: Vec<Placed>,
    width: f32,
    height: f32,
}

impl Prepared {
    pub fn height(&self) -> f32 {
        self.height
    }
}

/// How a block is laid out: into the kept store, or just for now.
struct Setting<'a> {
    context: &'a egui::Context,
    width: f32,

    /// False for text that will have changed by the next frame.
    keep: bool,
}

impl Setting<'_> {
    fn galley(&self, content: u64, width: f32, job: LayoutJob) -> Arc<Galley> {
        if self.keep {
            typeset::layout_at(self.context, content, width, || job)
        } else {
            typeset::direct(self.context, job)
        }
    }
}

/// The widest a line of prose should run: about [`theme::MEASURE_CHARS`]
/// characters of the body font.
pub fn measure(context: &egui::Context) -> f32 {
    let sample = typeset::layout(context, typeset::hash(("measure", SAMPLE)), || {
        LayoutJob::simple_singleline(SAMPLE.to_string(), theme::body(), palette::text())
    });

    sample.size().x / SAMPLE.chars().count() as f32 * theme::MEASURE_CHARS
}

/// Splits `text` into blocks and lays each out no wider than `width`.
pub fn prepare(context: &egui::Context, text: &str, width: f32) -> Prepared {
    let setting = Setting {
        context,
        width,
        keep: true,
    };

    let mut prepared = Prepared {
        width,
        ..Prepared::default()
    };

    let mut before = None;

    for block in parse(text) {
        place(&setting, before.as_ref(), &block, &mut prepared.blocks);

        before = Some(block);
    }

    prepared.height = total(&prepared.blocks);

    prepared
}

fn total(blocks: &[Placed]) -> f32 {
    blocks.iter().map(|placed| placed.gap + placed.height).sum()
}

/// Roughly how tall `text` will be at `width`, from its length alone.
/// Used for messages that have never been on screen; the true height
/// replaces it the first time they are laid out.
pub fn estimate_height(text: &str, width: f32) -> f32 {
    let row = size::BODY * theme::LINE_HEIGHT;
    let per_row = (width / (size::BODY * 0.48)).max(8.0);

    text.lines()
        .map(|line| match line.trim().len() {
            0 => BLOCK_GAP - row * 0.1,
            length => (length as f32 / per_row).ceil() * row,
        })
        .sum::<f32>()
        .max(row)
}

/// Draws a prepared message at the cursor. It takes exactly
/// [`Prepared::height`] of room.
pub fn paint(ui: &mut Ui, prepared: &Prepared) {
    paint_blocks(ui, &prepared.blocks, prepared.width, 0);
}

/// `first` numbers the blocks, so that each keeps the same identity from
/// frame to frame whichever of the others are drawn.
fn paint_blocks(ui: &mut Ui, blocks: &[Placed], width: f32, first: usize) {
    // The room between blocks is theirs to say, so that what is drawn is
    // exactly as tall as what was prepared.
    let spacing = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);

    for (index, placed) in blocks.iter().enumerate() {
        let top = ui.cursor().min + vec2(0.0, placed.gap);
        let rect = Rect::from_min_size(top, vec2(width, placed.height));

        ui.allocate_rect(
            Rect::from_min_max(ui.cursor().min, rect.max),
            Sense::hover(),
        );

        // What is scrolled out of sight keeps its room and costs nothing.
        if !ui.is_rect_visible(rect) {
            continue;
        }

        let id = ui.id().with(("block", first + index));

        match &placed.laid {
            Laid::Text {
                galley,
                links,
                indent,
                bar,
            } => {
                if *bar {
                    ui.painter().vline(
                        rect.left() + 2.0,
                        rect.y_range(),
                        Stroke::new(2.0, palette::border()),
                    );
                }

                label(ui, id, rect.min + vec2(*indent, 0.0), galley, links);
            }

            Laid::Entry {
                marker,
                galley,
                links,
                indent,
            } => {
                ui.painter().galley(
                    rect.min + vec2(*indent, 0.0),
                    marker.clone(),
                    palette::text_muted(),
                );

                let left = indent + marker.size().x + MARKER_GAP;

                label(ui, id, rect.min + vec2(left, 0.0), galley, links);
            }

            Laid::Code { galley, label } => paint_code(ui, id, rect, galley, label),
            Laid::Table(table) => paint_table(ui, id, rect, table),

            Laid::Rule => {
                ui.painter().hline(
                    rect.x_range(),
                    rect.center().y,
                    Stroke::new(1.0, palette::border()),
                );
            }
        }
    }

    ui.spacing_mut().item_spacing.y = spacing;
}

/// Text that can be selected, at a place already chosen, with its links
/// answering the pointer.
fn label(ui: &mut Ui, id: egui::Id, at: Pos2, galley: &Arc<Galley>, links: &[Link]) {
    let rect = Rect::from_min_size(at, galley.size());

    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );

    let response = child.add(egui::Label::new(galley.clone()).selectable(true));

    if links.is_empty() {
        return;
    }

    let Some(pointer) = response.hover_pos() else {
        return;
    };

    let under = galley.cursor_from_pos(pointer - response.rect.min).index.0;

    let Some(link) = links.iter().find(|link| link.chars.contains(&under)) else {
        return;
    };

    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);

    if response.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&link.url));
    }

    response.on_hover_text(&link.url);
}

/// Monospaced text on a sunken ground, under a line that names it and
/// holds a button to copy it. Lines are not wrapped: what is too wide
/// scrolls sideways. `max_height` is the tallest the text is shown
/// before it scrolls down as well.
pub fn code_block(ui: &mut Ui, label: &str, code: &str, max_height: f32) {
    let galley = typeset::layout(ui.ctx(), typeset::hash(("code", label, code)), || {
        code_job(code, label)
    });

    let height = code_height(&galley).min(max_height + code_height_around());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());

    if ui.is_rect_visible(rect) {
        let id = ui.id().with(("code", ui.next_auto_id()));

        paint_code(ui, id, rect, &galley, label);
    }
}

/// How tall a block of code is with all of its lines showing.
fn code_height(galley: &Galley) -> f32 {
    galley.size().y + code_height_around()
}

/// What a block of code takes besides its lines.
fn code_height_around() -> f32 {
    2.0 * CODE_PADDING.y + CODE_HEADER + CODE_HEADER_GAP
}

fn paint_code(ui: &mut Ui, id: egui::Id, rect: Rect, galley: &Arc<Galley>, label: &str) {
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius::M), palette::sunken());

    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(radius::M),
        Stroke::new(1.0, palette::border().gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );

    let header = Rect::from_min_size(
        rect.min + vec2(0.0, CODE_PADDING.y),
        vec2(rect.width(), CODE_HEADER),
    );

    ui.painter().text(
        header.left_center() + vec2(CODE_PADDING.x, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        theme::small(),
        palette::text_muted(),
    );

    let copy = icons::slot_from_right(header, 0);

    if icons::button_at(ui, copy, id.with("copy"), Icon::Copy, "Copy", false).clicked() {
        ui.ctx().copy_text(galley.job.text.clone());
    }

    let body = Rect::from_min_max(
        Pos2::new(
            rect.left() + CODE_PADDING.x,
            header.bottom() + CODE_HEADER_GAP,
        ),
        rect.max - CODE_PADDING,
    );

    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(body)
            .layout(Layout::top_down(Align::Min)),
    );

    child.set_clip_rect(body.intersect(ui.clip_rect()));

    // It scrolls down only when it has been cut short.
    let cut = galley.size().y > body.height() + 0.5;

    egui::ScrollArea::new([true, cut])
        .id_salt(id.with("scroll"))
        .auto_shrink(false)
        .max_width(body.width())
        .max_height(body.height())
        .show(&mut child, |ui| {
            ui.add(egui::Label::new(galley.clone()).selectable(true));
        });
}

fn paint_table(ui: &mut Ui, id: egui::Id, rect: Rect, table: &Table) {
    let size = table.size();

    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );

    child.set_clip_rect(rect.intersect(ui.clip_rect()));

    egui::ScrollArea::horizontal()
        .id_salt(id.with("scroll"))
        .auto_shrink(false)
        .max_width(rect.width())
        .max_height(rect.height())
        .show(&mut child, |ui| {
            let (area, _) = ui.allocate_exact_size(size, Sense::hover());
            let painter = ui.painter_at(area.intersect(ui.clip_rect()));

            let line = Stroke::new(1.0, palette::border());

            let header = Rect::from_min_size(area.min, vec2(size.x, table.rows[0]));

            painter.rect_filled(
                header,
                CornerRadius {
                    nw: radius::M,
                    ne: radius::M,
                    sw: 0,
                    se: 0,
                },
                palette::surface(),
            );

            let mut top = area.top();

            for (row, cells) in table.cells.iter().enumerate() {
                if row > 0 {
                    painter.hline(area.x_range(), top, line);
                }

                let mut left = area.left();

                for (column, cell) in cells.iter().enumerate() {
                    painter.galley(
                        Pos2::new(left, top) + CELL_PADDING,
                        cell.clone(),
                        palette::text(),
                    );

                    left += table.columns[column];
                }

                top += table.rows[row];
            }

            painter.rect_stroke(
                area,
                CornerRadius::same(radius::M),
                line,
                egui::StrokeKind::Inside,
            );
        });
}

/// Lays `block` out and adds it to `placed`. `before` is the block
/// above it, which decides how much room is left between the two.
fn place(setting: &Setting<'_>, before: Option<&Block>, block: &Block, placed: &mut Vec<Placed>) {
    let gap = match (before, block) {
        (None, _) => 0.0,
        (Some(Block::List(_)), Block::List(_)) => ENTRY_GAP,
        (Some(Block::Heading { .. }), _) => space::S,
        (Some(_), Block::Heading { .. }) => HEADING_GAP,
        (Some(_), _) => BLOCK_GAP,
    };

    let width = setting.width;

    let mut put = |gap: f32, height: f32, laid: Laid| {
        placed.push(Placed { gap, height, laid });
    };

    match block {
        Block::Paragraph(text) => {
            let (galley, links) = paragraph(setting, text, &body_format(), width);

            put(
                gap,
                galley.size().y,
                Laid::Text {
                    galley,
                    links,
                    indent: 0.0,
                    bar: false,
                },
            );
        }

        Block::Heading { level, text } => {
            let size = match level {
                1 => size::TITLE + 2.0,
                2 => size::TITLE,
                3 => size::BODY + 1.0,
                _ => size::BODY,
            };

            let format = TextFormat {
                font_id: theme::strong(size),
                color: palette::text(),
                line_height: Some(size * 1.3),
                ..TextFormat::default()
            };

            let (galley, links) = paragraph(setting, text, &format, width);

            put(
                gap,
                galley.size().y,
                Laid::Text {
                    galley,
                    links,
                    indent: 0.0,
                    bar: false,
                },
            );
        }

        Block::Quote(text) => {
            let format = TextFormat {
                color: palette::text_secondary(),
                ..body_format()
            };

            let indent = space::L;
            let (galley, links) = paragraph(setting, text, &format, width - indent);

            put(
                gap,
                galley.size().y,
                Laid::Text {
                    galley,
                    links,
                    indent,
                    bar: true,
                },
            );
        }

        Block::List(entries) => {
            for (index, entry) in entries.iter().enumerate() {
                let marker = setting.galley(
                    typeset::hash(("marker", &entry.marker)),
                    f32::INFINITY,
                    LayoutJob::single_section(
                        entry.marker.clone(),
                        TextFormat {
                            color: palette::text_muted(),
                            ..body_format()
                        },
                    ),
                );

                let indent = f32::from(entry.depth) * LIST_INDENT;
                let room = (width - indent - marker.size().x - MARKER_GAP).max(40.0);

                let (galley, links) = paragraph(setting, &entry.text, &body_format(), room);

                put(
                    if index == 0 { gap } else { ENTRY_GAP },
                    galley.size().y.max(marker.size().y),
                    Laid::Entry {
                        marker,
                        galley,
                        links,
                        indent,
                    },
                );
            }
        }

        Block::Code { language, code } => {
            let galley = setting.galley(
                typeset::hash(("code", language, code)),
                f32::INFINITY,
                code_job(code, language),
            );

            put(
                gap,
                code_height(&galley),
                Laid::Code {
                    galley,
                    label: language.clone(),
                },
            );
        }

        Block::Table(rows) => {
            let table = table(setting, rows);

            put(gap, table.size().y, Laid::Table(table));
        }

        Block::Rule => put(gap, 1.0, Laid::Rule),
    }
}

fn table(setting: &Setting<'_>, rows: &[Vec<String>]) -> Table {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);

    // A cell wraps before it takes most of the column to itself.
    let widest = (setting.width * 0.6).max(120.0);

    let mut table = Table {
        cells: Vec::with_capacity(rows.len()),
        columns: vec![0.0; columns],
        rows: Vec::with_capacity(rows.len()),
    };

    for (row, cells) in rows.iter().enumerate() {
        let format = if row == 0 {
            TextFormat {
                font_id: theme::strong(size::UI),
                ..body_format()
            }
        } else {
            TextFormat {
                font_id: theme::ui(),
                ..body_format()
            }
        };

        let mut height: f32 = 0.0;
        let mut laid = Vec::with_capacity(columns);

        for column in 0..columns {
            let text = cells.get(column).map(String::as_str).unwrap_or_default();
            let (galley, _) = paragraph(setting, text, &format, widest);

            table.columns[column] =
                table.columns[column].max(galley.size().x + 2.0 * CELL_PADDING.x);
            height = height.max(galley.size().y + 2.0 * CELL_PADDING.y);

            laid.push(galley);
        }

        table.cells.push(laid);
        table.rows.push(height);
    }

    table
}

fn body_format() -> TextFormat {
    TextFormat {
        font_id: theme::body(),
        color: palette::text(),
        line_height: Some(size::BODY * theme::LINE_HEIGHT),
        ..TextFormat::default()
    }
}

/// A paragraph laid out to `width`, with where its links are.
fn paragraph(
    setting: &Setting<'_>,
    text: &str,
    format: &TextFormat,
    width: f32,
) -> (Arc<Galley>, Vec<Link>) {
    let (mut job, links) = inline(text, format);

    job.wrap = TextWrapping::wrap_at_width(width);

    let content = typeset::hash((
        "text",
        text,
        format.font_id.size.to_bits(),
        &format.font_id.family,
        format.color,
    ));

    (setting.galley(content, width, job), links)
}

/// A paragraph with its `inline code` set in the monospaced font, its
/// `**strong**` words set heavy, its `*emphasised*` ones slanted and its
/// links marked.
fn inline(text: &str, plain: &TextFormat) -> (LayoutJob, Vec<Link>) {
    let mut job = LayoutJob::default();
    let mut links = Vec::new();
    let mut chars = 0;

    let size = plain.font_id.size;

    // Monospaced letters look larger than a serif's at the same size, so
    // code set inside a sentence is taken down a little to sit level.
    let code = TextFormat {
        font_id: FontId::monospace(size * 0.88),
        color: palette::code(),
        background: palette::surface(),
        ..plain.clone()
    };

    let strong = TextFormat {
        font_id: theme::strong(size),
        ..plain.clone()
    };

    let emphasis = TextFormat {
        italics: true,
        ..plain.clone()
    };

    for (piece, style) in spans(text) {
        let format = match style {
            Style::Plain => plain,
            Style::Strong => &strong,
            Style::Emphasis => &emphasis,

            Style::Code => {
                job.append(piece, 0.0, code.clone());
                chars += piece.chars().count();

                continue;
            }
        };

        for (shown, url) in linked(piece) {
            let count = shown.chars().count();

            match url {
                Some(url) => {
                    let marked = TextFormat {
                        color: palette::accent(),
                        underline: Stroke::new(1.0, palette::accent().gamma_multiply(0.5)),
                        ..format.clone()
                    };

                    job.append(shown, 0.0, marked);

                    links.push(Link {
                        chars: chars..chars + count,
                        url: url.to_string(),
                    });
                }

                None => job.append(shown, 0.0, format.clone()),
            }

            chars += count;
        }
    }

    (job, links)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Style {
    Plain,
    Strong,
    Emphasis,
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

            if part.is_empty() {
                continue;
            }

            if strong {
                spans.push((part, Style::Strong));
            } else {
                emphasised(part, &mut spans);
            }
        }
    }

    spans
}

/// Splits `part` at its `*emphasised*` words. An asterisk only opens
/// when a word follows it at once and only closes when one comes right
/// before it, so `2 * 3 * 4` is left as it is.
fn emphasised<'a>(part: &'a str, spans: &mut Vec<(&'a str, Style)>) {
    let mut rest = part;

    loop {
        let found = rest.char_indices().find_map(|(open, character)| {
            if character != '*' {
                return None;
            }

            let inside = &rest[open + 1..];

            if inside.starts_with(|next: char| next.is_whitespace() || next == '*') {
                return None;
            }

            let before = rest[..open].chars().next_back();

            if before.is_some_and(|before| before.is_alphanumeric()) {
                return None;
            }

            let close = inside.char_indices().find_map(|(close, character)| {
                let word_before = inside[..close]
                    .chars()
                    .next_back()
                    .is_some_and(|before| !before.is_whitespace());

                (character == '*' && word_before).then_some(close)
            })?;

            Some((open, open + 1 + close))
        });

        let Some((open, close)) = found else {
            if !rest.is_empty() {
                spans.push((rest, Style::Plain));
            }

            return;
        };

        if open > 0 {
            spans.push((&rest[..open], Style::Plain));
        }

        spans.push((&rest[open + 1..close], Style::Emphasis));

        rest = &rest[close + 1..];
    }
}

/// The pieces of `text` as they are shown, each with the address it
/// leads to if it is a link: `[words](address)`, or an address written
/// out. Only addresses a browser or a mail program opens are links.
fn linked(text: &str) -> Vec<(&str, Option<&str>)> {
    let mut pieces = Vec::new();
    let mut rest = text;

    while !rest.is_empty() {
        let marked = marked_link(rest);
        let bare = bare_link(rest);

        // Whichever comes first.
        let next = match (marked, bare) {
            (Some(marked), Some(bare)) if bare.0.start < marked.0.start => Some(bare),
            (Some(marked), _) => Some(marked),
            (None, bare) => bare,
        };

        let Some((whole, shown, url)) = next else {
            pieces.push((rest, None));

            break;
        };

        if whole.start > 0 {
            pieces.push((&rest[..whole.start], None));
        }

        pieces.push((&rest[shown], Some(&rest[url])));

        rest = &rest[whole.end..];
    }

    pieces
}

type Found = (Range<usize>, Range<usize>, Range<usize>);

/// The first `[words](address)` in `text`: all of it, its words, and
/// its address.
fn marked_link(text: &str) -> Option<Found> {
    let mut from = 0;

    while let Some(open) = text[from..].find('[').map(|at| from + at) {
        from = open + 1;

        let middle = text[open..].find("](").map(|at| open + at)?;
        let close = text[middle..].find(')').map(|at| middle + at)?;

        let shown = open + 1..middle;
        let url = middle + 2..close;

        if !text[shown.clone()].contains('[') && opens(&text[url.clone()]) && !shown.is_empty() {
            return Some((open..close + 1, shown, url));
        }
    }

    None
}

/// The first address written out in `text`.
fn bare_link(text: &str) -> Option<Found> {
    let start = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| text.find(scheme))
        .min()?;

    let length = text[start..]
        .find(|character: char| character.is_whitespace() || "<>\"'`".contains(character))
        .unwrap_or(text.len() - start);

    // A full stop or a bracket after an address belongs to the sentence.
    let url =
        text[start..start + length].trim_end_matches(['.', ',', ';', ':', ')', ']', '!', '?']);

    let whole = start..start + url.len();

    opens(url).then(|| (whole.clone(), whole.clone(), whole))
}

/// Whether `url` is an address that may be handed to the system to open.
fn opens(url: &str) -> bool {
    let rest = ["https://", "http://", "mailto:"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme));

    rest.is_some_and(|rest| !rest.is_empty() && !url.contains(char::is_whitespace))
}

fn parse(text: &str) -> Vec<Block> {
    parse_at(text).into_iter().map(|(_, block)| block).collect()
}

/// What is being gathered, line by line, into the next block.
enum Open<'a> {
    Nothing,
    Paragraph(Vec<&'a str>),
    Code(&'a str, Vec<&'a str>),
    List(Vec<Entry>),
    Quote(Vec<&'a str>),
    Table(Vec<&'a str>),
}

impl Open<'_> {
    /// Ends the block being gathered, if there is one.
    fn close(&mut self, start: usize, blocks: &mut Vec<(usize, Block)>) {
        let block = match std::mem::replace(self, Open::Nothing) {
            Open::Nothing => return,
            Open::Paragraph(lines) => Block::Paragraph(lines.join("\n")),

            Open::Code(language, lines) => Block::Code {
                language: language.to_string(),
                code: lines.join("\n"),
            },

            Open::List(entries) => Block::List(entries),
            Open::Quote(lines) => Block::Quote(lines.join("\n")),

            // Rows with no line of dashes under the first are not a
            // table, just lines with bars in them.
            Open::Table(lines) => match lines.get(1).map(|line| cells(line)) {
                Some(rule) if is_table_rule(&rule) => {
                    let rows = std::iter::once(lines[0])
                        .chain(lines[2..].iter().copied())
                        .map(|line| cells(line).into_iter().map(str::to_string).collect())
                        .collect();

                    Block::Table(rows)
                }

                _ => Block::Paragraph(lines.join("\n")),
            },
        };

        blocks.push((start, block));
    }
}

/// The blocks of `text`, each with the place in `text` where it begins.
/// Reading on from the beginning of any block gives the same blocks from
/// there, which is what lets a growing text be read from its last block
/// instead of from its start.
fn parse_at(text: &str) -> Vec<(usize, Block)> {
    let mut blocks = Vec::new();
    let mut open = Open::Nothing;

    // Where the block being gathered began, and where this line does.
    let mut start = 0;
    let mut offset = 0;

    for raw in text.split_inclusive('\n') {
        let at = offset;
        let line = raw.trim_end_matches(['\n', '\r']);

        offset += raw.len();

        if let Open::Code(_, lines) = &mut open {
            if line.trim_start().starts_with("```") {
                open.close(start, &mut blocks);
            } else {
                lines.push(line);
            }

            continue;
        }

        if let Some(language) = line.trim_start().strip_prefix("```") {
            open.close(start, &mut blocks);

            start = at;
            open = Open::Code(language.trim(), Vec::new());

            continue;
        }

        if line.trim().is_empty() {
            open.close(start, &mut blocks);

            continue;
        }

        if let Some((level, title)) = heading(line) {
            open.close(start, &mut blocks);

            // A heading is already set heavy, so its own emphasis is dropped.
            blocks.push((
                at,
                Block::Heading {
                    level,
                    text: title.replace("**", ""),
                },
            ));

            continue;
        }

        if is_rule(line) {
            open.close(start, &mut blocks);
            blocks.push((at, Block::Rule));

            continue;
        }

        if let Some(entry) = list_entry(line) {
            match &mut open {
                Open::List(entries) => entries.push(entry),

                _ => {
                    open.close(start, &mut blocks);

                    start = at;
                    open = Open::List(vec![entry]);
                }
            }

            continue;
        }

        // A line set in under an entry carries the entry on.
        if let Open::List(entries) = &mut open
            && line.starts_with([' ', '\t'])
            && let Some(last) = entries.last_mut()
        {
            last.text.push(' ');
            last.text.push_str(line.trim());

            continue;
        }

        if let Some(quoted) = line.trim_start().strip_prefix('>') {
            let quoted = quoted.strip_prefix(' ').unwrap_or(quoted);

            match &mut open {
                Open::Quote(lines) => lines.push(quoted),

                _ => {
                    open.close(start, &mut blocks);

                    start = at;
                    open = Open::Quote(vec![quoted]);
                }
            }

            continue;
        }

        if line.trim_start().starts_with('|') {
            match &mut open {
                Open::Table(lines) => lines.push(line),

                _ => {
                    open.close(start, &mut blocks);

                    start = at;
                    open = Open::Table(vec![line]);
                }
            }

            continue;
        }

        match &mut open {
            Open::Paragraph(lines) => lines.push(line),

            _ => {
                open.close(start, &mut blocks);

                start = at;
                open = Open::Paragraph(vec![line]);
            }
        }
    }

    // A fence that was never closed still holds code worth showing.
    open.close(start, &mut blocks);

    blocks
}

/// A heading's level and its words.
fn heading(line: &str) -> Option<(u8, &str)> {
    let title = line.trim_start_matches('#');

    let hashes = line.len() - title.len();

    ((1..=6).contains(&hashes) && title.starts_with(' ')).then(|| (hashes as u8, title.trim()))
}

/// A line of nothing but three or more dashes, asterisks or underscores.
fn is_rule(line: &str) -> bool {
    let line = line.trim();

    let Some(first) = line.chars().next() else {
        return false;
    };

    line.len() >= 3 && "-*_".contains(first) && line.chars().all(|character| character == first)
}

/// A line that begins an entry of a list: `- `, `* `, `+ `, `1. ` or
/// `1) `, set in by two spaces for each level it is nested.
fn list_entry(line: &str) -> Option<Entry> {
    let rest = line.trim_start();

    let indent: usize = line[..line.len() - rest.len()]
        .chars()
        .map(|character| if character == '\t' { 4 } else { 1 })
        .sum();

    let depth = (indent / 2).min(4) as u8;

    for bullet in ["- ", "* ", "+ "] {
        if let Some(text) = rest.strip_prefix(bullet) {
            return Some(Entry {
                depth,
                marker: "•".to_string(),
                text: text.trim().to_string(),
            });
        }
    }

    let digits = rest.chars().take_while(char::is_ascii_digit).count();

    if !(1..=9).contains(&digits) {
        return None;
    }

    let text = rest[digits..]
        .strip_prefix(". ")
        .or_else(|| rest[digits..].strip_prefix(") "))?;

    Some(Entry {
        depth,
        marker: format!("{}.", &rest[..digits]),
        text: text.trim().to_string(),
    })
}

/// The cells of one row of a table.
fn cells(line: &str) -> Vec<&str> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);

    line.split('|').map(str::trim).collect()
}

/// Whether these are the cells of the line of dashes under a header.
fn is_table_rule(cells: &[&str]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let dashes = cell.trim_matches(':');

            !dashes.is_empty() && dashes.chars().all(|character| character == '-')
        })
}

/// What a word of code is, for its colour.
#[derive(Clone, Copy, PartialEq)]
enum Token {
    Plain,
    Keyword,
    Text,
    Number,
    Comment,
}

/// Words that mean something to enough languages to be worth marking.
const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "def",
    "default",
    "defer",
    "do",
    "elif",
    "else",
    "enum",
    "except",
    "export",
    "extends",
    "false",
    "False",
    "finally",
    "fn",
    "for",
    "from",
    "func",
    "function",
    "go",
    "if",
    "impl",
    "import",
    "in",
    "interface",
    "let",
    "loop",
    "match",
    "mod",
    "mut",
    "new",
    "nil",
    "None",
    "not",
    "null",
    "package",
    "pass",
    "pub",
    "raise",
    "return",
    "select",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "switch",
    "this",
    "throw",
    "trait",
    "true",
    "True",
    "try",
    "type",
    "typeof",
    "use",
    "var",
    "void",
    "where",
    "while",
    "with",
    "yield",
    "and",
    "or",
];

/// How a comment that runs to the end of its line begins in `language`,
/// or `None` for a language that is shown in one colour.
fn line_comment(language: &str) -> Option<&'static str> {
    match language.to_ascii_lowercase().as_str() {
        "rust" | "rs" | "c" | "cpp" | "c++" | "h" | "java" | "javascript" | "js" | "jsx"
        | "typescript" | "ts" | "tsx" | "go" | "swift" | "kotlin" | "kt" | "scala" | "dart"
        | "css" | "json" | "jsonc" | "zig" | "csharp" | "cs" => Some("//"),

        "python" | "py" | "sh" | "bash" | "zsh" | "shell" | "yaml" | "yml" | "toml" | "ruby"
        | "rb" | "make" | "makefile" | "dockerfile" | "r" | "perl" | "elixir" => Some("#"),

        "sql" | "lua" | "haskell" | "hs" => Some("--"),
        _ => None,
    }
}

/// Code as one unwrapped job. With a language it knows, comments,
/// strings, numbers and common keywords each get a colour; this is done
/// once, when the code is laid out, and kept with the layout.
fn code_job(code: &str, language: &str) -> LayoutJob {
    let format = |color| TextFormat {
        font_id: theme::mono(),
        color,
        line_height: Some(size::SMALL * 1.5),
        ..TextFormat::default()
    };

    let mut job = LayoutJob {
        wrap: TextWrapping::no_max_width(),
        ..LayoutJob::default()
    };

    let comment = line_comment(language).filter(|_| code.len() <= MAX_TINTED_BYTES);

    let Some(comment) = comment else {
        job.append(code, 0.0, format(palette::text_secondary()));

        return job;
    };

    // A single quote in Rust is more often a lifetime than a string.
    let quotes: &[char] = match language.to_ascii_lowercase().as_str() {
        "rust" | "rs" => &['"'],
        _ => &['"', '\''],
    };

    for (range, token) in tokens(code, comment, quotes) {
        let color = match token {
            Token::Plain => palette::text(),
            Token::Keyword => palette::model(),
            Token::Text => palette::good(),
            Token::Number => palette::warning(),
            Token::Comment => palette::text_muted(),
        };

        job.append(&code[range], 0.0, format(color));
    }

    job
}

/// `code` cut into runs of one kind each. Every byte is in exactly one.
fn tokens(code: &str, comment: &str, quotes: &[char]) -> Vec<(Range<usize>, Token)> {
    let mut tokens: Vec<(Range<usize>, Token)> = Vec::new();

    let mut push = |range: Range<usize>, token: Token| {
        if range.is_empty() {
            return;
        }

        match tokens.last_mut() {
            Some((last, kind)) if *kind == token && last.end == range.start => {
                last.end = range.end;
            }

            _ => tokens.push((range, token)),
        }
    };

    let mut at = 0;

    while at < code.len() {
        let rest = &code[at..];
        let first = rest.chars().next().unwrap_or(' ');

        let (length, token) = if rest.starts_with(comment) {
            (rest.find('\n').unwrap_or(rest.len()), Token::Comment)
        } else if quotes.contains(&first) {
            // To the matching quote on the same line, past any that are
            // escaped; a quote left open ends with its line.
            let mut escaped = false;
            let mut length = rest.len();

            for (index, character) in rest.char_indices().skip(1) {
                if character == '\n' {
                    length = index;

                    break;
                }

                if character == first && !escaped {
                    length = index + character.len_utf8();

                    break;
                }

                escaped = character == '\\' && !escaped;
            }

            (length, Token::Text)
        } else if first.is_alphabetic() || first == '_' {
            let length = rest
                .find(|character: char| !character.is_alphanumeric() && character != '_')
                .unwrap_or(rest.len());

            let token = if KEYWORDS.contains(&&rest[..length]) {
                Token::Keyword
            } else {
                Token::Plain
            };

            (length, token)
        } else if first.is_ascii_digit() {
            let length = rest
                .find(|character: char| {
                    !character.is_alphanumeric() && character != '.' && character != '_'
                })
                .unwrap_or(rest.len());

            (length, Token::Number)
        } else {
            (first.len_utf8(), Token::Plain)
        };

        let length = length.max(first.len_utf8());

        push(at..at + length, token);

        at += length;
    }

    tokens
}

/// A response that is still being written, laid out as it grows.
///
/// The text only ever gets longer, so every block but the last is
/// complete: those are laid out once and kept here, and each frame only
/// what follows them is read and laid out again. Their heights never
/// change, so nothing above the growing end moves.
#[derive(Default)]
pub struct Stream {
    /// The complete blocks, and how tall they are together.
    done: Vec<Placed>,
    done_height: f32,

    /// The last complete block as it was read, for the room left under
    /// it.
    last_done: Option<Block>,

    /// How much of the text the complete blocks cover.
    consumed: usize,

    /// A hash of the text just before `consumed`, to notice a text that
    /// is not the one this was following.
    guard: u64,

    /// The block still growing.
    tail: Vec<Placed>,
    tail_height: f32,

    width: f32,
    theme: usize,
    generation: u64,

    /// How many blocks the last update read and laid out.
    worked: usize,
}

impl Stream {
    /// Takes in `text` as it is now. Only what follows the last complete
    /// block is read.
    pub fn update(&mut self, context: &egui::Context, text: &str, width: f32) {
        let generation = typeset::generation(context);

        let same_setting =
            self.width == width && self.theme == palette::index() && self.generation == generation;

        let follows = text.len() >= self.consumed
            && text.is_char_boundary(self.consumed)
            && guard(text, self.consumed) == self.guard;

        if !same_setting || !follows {
            *self = Self {
                width,
                theme: palette::index(),
                generation,
                guard: guard(text, 0),
                ..Self::default()
            };
        }

        let parsed = parse_at(&text[self.consumed..]);

        self.worked = parsed.len();

        let Some(((last_start, last), complete)) = parsed.split_last() else {
            self.tail.clear();
            self.tail_height = 0.0;

            return;
        };

        let kept = Setting {
            context,
            width,
            keep: true,
        };

        for (_, block) in complete {
            place(&kept, self.last_done.as_ref(), block, &mut self.done);

            self.last_done = Some(block.clone());
        }

        if !complete.is_empty() {
            self.done_height = total(&self.done);
            self.consumed += last_start;
            self.guard = guard(text, self.consumed);
        }

        let passing = Setting {
            context,
            width,
            keep: false,
        };

        self.tail.clear();

        place(&passing, self.last_done.as_ref(), last, &mut self.tail);

        self.tail_height = total(&self.tail);
    }

    #[cfg(test)]
    pub fn height(&self) -> f32 {
        self.done_height + self.tail_height
    }

    /// How many blocks the last update had to read and lay out: the one
    /// still growing, and any that were completed since the update
    /// before.
    #[cfg(test)]
    pub fn worked(&self) -> usize {
        self.worked
    }

    /// How many blocks are complete and kept.
    #[cfg(test)]
    pub fn kept(&self) -> usize {
        self.done.len()
    }

    pub fn paint(&self, ui: &mut Ui) {
        paint_blocks(ui, &self.done, self.width, 0);
        paint_blocks(ui, &self.tail, self.width, self.done.len());
    }
}

/// A hash of the little of `text` that comes just before `end`.
fn guard(text: &str, end: usize) -> u64 {
    const LOOKED_AT: usize = 48;

    let end = end.min(text.len());

    typeset::hash((end, &text.as_bytes()[end.saturating_sub(LOOKED_AT)..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> egui::Context {
        let context = egui::Context::default();

        theme::apply(&context);

        // Fonts exist only once a frame has begun.
        let mut output = context.run_ui(egui::RawInput::default(), |_| {});

        output.textures_delta.clear();

        context
    }

    fn paragraph(text: &str) -> Block {
        Block::Paragraph(text.to_string())
    }

    #[test]
    fn text_is_split_into_blocks() {
        let blocks =
            parse("Fixed it.\nTwo lines.\n\n## What changed\n\n```python\nx = 1\n```\nDone.");

        assert_eq!(
            blocks,
            [
                paragraph("Fixed it.\nTwo lines."),
                Block::Heading {
                    level: 2,
                    text: "What changed".to_string()
                },
                Block::Code {
                    language: "python".to_string(),
                    code: "x = 1".to_string()
                },
                paragraph("Done."),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_keeps_its_code() {
        assert_eq!(
            parse("```\nx = 1"),
            [Block::Code {
                language: String::new(),
                code: "x = 1".to_string()
            }]
        );
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
    fn emphasis_needs_a_word_on_the_inside_of_each_asterisk() {
        assert_eq!(
            spans("It is *very* fast."),
            [
                ("It is ", Style::Plain),
                ("very", Style::Emphasis),
                (" fast.", Style::Plain),
            ]
        );

        assert_eq!(spans("2 * 3 * 4"), [("2 * 3 * 4", Style::Plain)]);
        assert_eq!(spans("a*b*c"), [("a*b*c", Style::Plain)]);
        assert_eq!(spans("an open *one"), [("an open *one", Style::Plain)]);
    }

    #[test]
    fn a_heading_drops_its_asterisks() {
        assert_eq!(
            parse("## **Summary**"),
            [Block::Heading {
                level: 2,
                text: "Summary".to_string()
            }]
        );
    }

    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        assert_eq!(heading("#include <stdio.h>"), None);
        assert_eq!(heading("# Title"), Some((1, "Title")));
        assert_eq!(heading("### Deeper"), Some((3, "Deeper")));
    }

    #[test]
    fn lists_keep_their_marks_and_their_nesting() {
        let blocks = parse("- one\n  - inside\n- two\n  carried on\n\n1. first\n2) second");

        let entry = |depth, marker: &str, text: &str| Entry {
            depth,
            marker: marker.to_string(),
            text: text.to_string(),
        };

        assert_eq!(
            blocks,
            [
                Block::List(vec![
                    entry(0, "•", "one"),
                    entry(1, "•", "inside"),
                    entry(0, "•", "two carried on"),
                ]),
                Block::List(vec![entry(0, "1.", "first"), entry(0, "2.", "second")]),
            ]
        );
    }

    #[test]
    fn a_table_needs_its_line_of_dashes() {
        let table = parse("| Name | Size |\n|:-----|-----:|\n| a.rs | 12 |\n| b.rs |");

        assert_eq!(
            table,
            [Block::Table(vec![
                vec!["Name".to_string(), "Size".to_string()],
                vec!["a.rs".to_string(), "12".to_string()],
                vec!["b.rs".to_string()],
            ])]
        );

        assert_eq!(parse("| not | a table |"), [paragraph("| not | a table |")]);
    }

    #[test]
    fn quotes_and_rules_are_told_apart_from_text() {
        assert_eq!(
            parse("> said\n> twice\n\n---\nafter"),
            [
                Block::Quote("said\ntwice".to_string()),
                Block::Rule,
                paragraph("after"),
            ]
        );

        assert!(!is_rule("--"));
        assert!(!is_rule("--- x"));
    }

    #[test]
    fn links_are_found_marked_or_written_out() {
        assert_eq!(
            linked("See [the docs](https://example.test/a) or https://example.test/b."),
            [
                ("See ", None),
                ("the docs", Some("https://example.test/a")),
                (" or ", None),
                ("https://example.test/b", Some("https://example.test/b")),
                (".", None),
            ]
        );
    }

    #[test]
    fn only_addresses_a_browser_opens_are_links() {
        assert_eq!(
            linked("[run](file:///bin/sh) and [x](javascript:alert(1))"),
            [("[run](file:///bin/sh) and [x](javascript:alert(1))", None)]
        );

        assert_eq!(linked("an array[0](1)"), [("an array[0](1)", None)]);
        assert!(!opens("https://"));
    }

    #[test]
    fn a_link_knows_which_drawn_characters_it_covers() {
        let (job, links) = inline("Go to [here](https://example.test) now", &body_format());

        assert_eq!(job.text, "Go to here now");
        assert_eq!(
            links,
            [Link {
                chars: 6..10,
                url: "https://example.test".to_string()
            }]
        );
    }

    #[test]
    fn every_byte_of_code_is_in_one_token() {
        let code = "fn main() {\n    let s = \"a \\\" b\"; // done\n    let n = 42_u8;\n}\n'x";

        let tokens = tokens(code, "//", &['"']);

        let mut end = 0;

        for (range, _) in &tokens {
            assert_eq!(range.start, end);

            end = range.end;
        }

        assert_eq!(end, code.len());

        let of = |token: Token| -> Vec<&str> {
            tokens
                .iter()
                .filter(|(_, kind)| *kind == token)
                .map(|(range, _)| &code[range.clone()])
                .collect()
        };

        assert_eq!(of(Token::Keyword), ["fn", "let", "let"]);
        assert_eq!(of(Token::Text), ["\"a \\\" b\""]);
        assert_eq!(of(Token::Comment), ["// done"]);
        assert_eq!(of(Token::Number), ["42_u8"]);
    }

    #[test]
    fn code_of_an_unknown_language_is_one_colour() {
        assert_eq!(code_job("let x = 1;", "").sections.len(), 1);
        assert!(code_job("let x = 1;", "rust").sections.len() > 1);
        assert_eq!(code_job("let x = 1;", "rust").text, "let x = 1;");
    }

    #[test]
    fn reading_on_from_any_block_gives_the_same_blocks() {
        let text = "Intro line.\n\n# Title\nBody one\nbody two.\n\n- a\n- b\n\n```rust\nfn x() {}\n\n```\n\
                    | h | i |\n|---|---|\n| 1 | 2 |\n\n> quoted\n\nlast";

        let whole = parse_at(text);

        assert!(whole.len() >= 7);

        for (index, (start, _)) in whole.iter().enumerate() {
            let rest: Vec<Block> = parse(&text[*start..]);
            let expected: Vec<Block> = whole[index..]
                .iter()
                .map(|(_, block)| block.clone())
                .collect();

            assert_eq!(rest, expected, "from block {index}");
        }
    }

    #[test]
    fn a_prepared_message_draws_as_tall_as_it_said() {
        let context = context();

        let text = "# Title\n\nA paragraph that is long enough to need more than one line when \
                    it is drawn in a narrow column, with `code` and **weight**.\n\n\
                    - one\n- two\n  - nested\n\n```rust\nfn main() {}\n// done\n```\n\n\
                    | a | b |\n|---|---|\n| 1 | 2 |\n\n> quoted\n\n---\n\nThe end.";

        let mut drawn = 0.0;
        let mut said = 0.0;

        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;

                let prepared = prepare(ui.ctx(), text, 320.0);
                let top = ui.cursor().top();

                paint(ui, &prepared);

                drawn = ui.cursor().top() - top;
                said = prepared.height();
            });
        });

        output.textures_delta.clear();

        assert!(said > 200.0, "{said}");
        assert!((drawn - said).abs() < 0.01, "drew {drawn}, said {said}");
    }

    #[test]
    fn a_message_is_laid_out_once_however_often_it_is_prepared() {
        let context = context();
        let text = "One paragraph.\n\nAnother, with `code`.\n\n- an entry";

        let first = prepare(&context, text, 400.0);
        let laid_out = typeset::counts(&context).laid_out;

        assert!(laid_out >= 3);

        for _ in 0..20 {
            let again = prepare(&context, text, 400.0);

            assert_eq!(again.height(), first.height());
        }

        assert_eq!(typeset::counts(&context).laid_out, laid_out);

        // Another width is another layout.
        prepare(&context, text, 300.0);

        assert!(typeset::counts(&context).laid_out > laid_out);
    }

    #[test]
    fn a_growing_response_lays_out_only_its_last_block() {
        let context = context();

        let paragraphs: Vec<String> = (0..40)
            .map(|number| format!("Paragraph {number} says a few words about the work."))
            .collect();

        let whole = paragraphs.join("\n\n");

        let mut stream = Stream::default();
        let mut most = 0;
        let mut heights = Vec::new();

        // The text arrives a few characters at a time.
        let mut end = 0;

        while end < whole.len() {
            end = (end + 7).min(whole.len());

            while !whole.is_char_boundary(end) {
                end += 1;
            }

            stream.update(&context, &whole[..end], 400.0);

            most = most.max(stream.worked());
            heights.push(stream.height());
        }

        // Never more than the block being written and the one it has just
        // left behind, however long the response has grown.
        assert!(most <= 2, "{most} blocks read in one update");
        assert_eq!(stream.kept(), 39);

        // It only ever grows: nothing that was drawn moves back up.
        assert!(heights.windows(2).all(|pair| pair[1] >= pair[0] - 0.01));

        // And it ends exactly as tall as the finished message.
        let finished = prepare(&context, &whole, 400.0);

        assert!((stream.height() - finished.height()).abs() < 0.01);
    }

    #[test]
    fn the_complete_blocks_of_a_response_are_laid_out_once() {
        let context = context();

        let mut stream = Stream::default();
        let mut text = String::from("First paragraph.\n\nSecond paragraph.\n\nThird");

        stream.update(&context, &text, 400.0);

        let laid_out = typeset::counts(&context).laid_out;

        for word in [" one", " two", " three", " four"] {
            text.push_str(word);

            stream.update(&context, &text, 400.0);
        }

        // The growing block is not kept, so the store did no more work.
        assert_eq!(typeset::counts(&context).laid_out, laid_out);
        assert_eq!(stream.kept(), 2);
    }

    #[test]
    fn a_response_that_starts_over_is_read_from_its_start() {
        let context = context();
        let mut stream = Stream::default();

        stream.update(&context, "One.\n\nTwo.\n\nThree", 400.0);

        assert_eq!(stream.kept(), 2);

        // The request was retried: other words, and already longer.
        stream.update(
            &context,
            "A different answer altogether, in one block",
            400.0,
        );

        assert_eq!(stream.kept(), 0);

        let fresh = prepare(
            &context,
            "A different answer altogether, in one block",
            400.0,
        );

        assert!((stream.height() - fresh.height()).abs() < 0.01);
    }

    #[test]
    fn an_estimate_is_within_reach_of_the_real_height() {
        let context = context();

        let text = "A paragraph of ordinary length that wraps onto a second line or so when \
                    it is drawn at this width, and then says a little more.\n\n"
            .repeat(8);

        let real = prepare(&context, &text, 500.0).height();
        let guess = estimate_height(&text, 500.0);

        assert!(
            guess > real * 0.5 && guess < real * 2.0,
            "{guess} for {real}"
        );
    }

    #[test]
    fn a_line_of_prose_is_about_eighty_characters() {
        let context = context();
        let width = measure(&context);

        let line = "word ".repeat(40);
        let (job, _) = inline(&line, &body_format());

        let mut job = job;
        job.wrap = TextWrapping::wrap_at_width(width);

        let galley = typeset::direct(&context, job);
        let longest = galley
            .rows
            .iter()
            .map(|row| row.glyphs.len())
            .max()
            .unwrap();

        assert!((66..=90).contains(&longest), "{longest} characters");
    }
}

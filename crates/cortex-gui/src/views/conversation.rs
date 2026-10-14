//! The conversation: what was asked, what the agent said and did.
//!
//! A conversation can run to thousands of messages, so it is not drawn
//! from its first to its last on every frame. Each message's text is
//! laid out ahead of drawing and kept (see [`prose`] and [`typeset`]),
//! which means its height is known without drawing it. With every
//! height known, only the messages that fall in or near the window are
//! drawn; the rest are passed over as empty room of the right size.
//! The work a frame takes then depends on how much is on screen, not on
//! how long the conversation is.
//!
//! A message that has never been on screen has its height guessed from
//! its length until it is first laid out. When the true height turns
//! out different and the message lies above what is being read, the
//! scroll position is moved by the difference, so that what is on
//! screen stays where it is.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use cortex_core::{ApprovalRequest, CheckOutcome, Image, Outcome, Requirement, Usage};
use cortex_providers::pricing;
use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    self, Align, CornerRadius, Galley, Layout, RichText, ScrollArea, Sense, Stroke, TextFormat, Ui,
    UiBuilder,
};

use super::icons::{self, Icon};
use super::prose::{self, code_block};
use super::{tool_card, typeset};
use crate::attachment;
use crate::engine::Decision;
use crate::format;
use crate::session::{Activity, Doing, Item, Session, Tone, Writing};
use crate::theme::{self, palette, radius, size, space};

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

/// Room above the first message and under the last.
const TOP_PADDING: f32 = space::M;
const BOTTOM_PADDING: f32 = 18.0;

/// Room under every message, tool run and notice.
const UNIT_GAP: f32 = space::XS;

/// Room above what the person said, which sets a new exchange apart from
/// the one before, and the padding inside its bubble.
const USER_GAP: f32 = 10.0;
const BUBBLE_PADDING: egui::Vec2 = egui::vec2(12.0, 7.0);

/// Room above and below what the agent said.
const AGENT_PADDING: f32 = space::XS;

/// Height of a line that unfolds into something.
const FOLD_HEIGHT: f32 = 20.0;

/// How far past the window messages are still drawn, so that scrolling
/// a little never shows a gap.
const DRAWN_MARGIN: f32 = 300.0;

/// How far past the window messages are laid out ahead of being drawn,
/// so that their true heights are known before they come into view.
const MEASURED_MARGIN: f32 = 700.0;

/// The trailing units looked at again on every frame: a tool's result
/// arrives in a call that is already listed.
const UNSETTLED: usize = 4;

/// Side of the round button that goes to the latest message.
const JUMP_SIZE: f32 = 30.0;

/// What a stretch of the conversation is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    /// Items that are not drawn: a step that only called tools, the
    /// engine's housekeeping.
    Nothing,
    User,
    Assistant,
    Notice,

    /// A run of context put in front of an instruction.
    Briefings,

    /// A run of tool calls, with the wordless steps between them.
    Tools,
    Checks,
    Review,
    Summary,
}

/// Text laid out for a unit, kept while the unit is near the window.
enum Ready {
    Prose(prose::Prepared),
    Bubble(Arc<Galley>),
    Line(Arc<Galley>),
}

/// One thing drawn in the column: a message, or a run of items that are
/// drawn together.
struct Unit {
    /// The items it covers.
    start: usize,
    end: usize,
    kind: Kind,

    /// A cheap mark of what the items held when the unit was last looked
    /// at. When it changes, what was worked out for the unit is void.
    print: u64,

    /// How tall it was when last laid out or drawn at the width in use.
    height: Option<f32>,

    /// How tall it is taken to be until then.
    guess: f32,

    ready: Option<Ready>,
}

impl Unit {
    fn height(&self) -> f32 {
        self.height.unwrap_or(self.guess)
    }
}

/// What the laid-out heights depend on besides the text.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Setting {
    column: f32,
    theme: usize,
    scale: f32,
    show_thinking: bool,

    /// Changes when every kept layout was dropped.
    generation: u64,
}

/// The widths the column's parts are set to.
#[derive(Debug, Clone, Copy)]
struct Widths {
    column: f32,

    /// The agent's prose: a comfortable line, and room for the gutter.
    prose: f32,

    /// The text inside the person's bubble, at most.
    bubble: f32,
}

/// What one frame did, for tests and for measuring.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Work {
    /// Units there are.
    pub units: usize,

    /// Units drawn.
    pub drawn: usize,

    /// Units whose text was read and laid out, or found laid out.
    pub prepared: usize,
}

/// How a conversation is being shown: where each message sits, how far
/// it is scrolled, and the text laid out for what is on screen.
pub struct ChatView {
    units: Vec<Unit>,

    /// How many of the session's items the units cover.
    covered: usize,

    /// A mark of the session's first item, to notice another session.
    head: u64,

    setting: Option<Setting>,

    /// How far down the conversation was scrolled, and how tall the
    /// window on it was, after the last frame.
    offset: f32,
    viewport: f32,

    /// The newest message is in view, and the view follows it.
    at_bottom: bool,

    /// Go to the newest message.
    jump: bool,

    /// Something arrived while the view was scrolled up.
    unseen: bool,

    /// How much the messages above the window grew since the last
    /// frame; owed to the scroll position.
    shift: f32,

    /// Counts the sessions shown, so that each starts scrolled to its
    /// end instead of where the one before was left.
    epoch: u64,

    /// Nothing of this session has been drawn yet.
    fresh: bool,

    /// How tall everything under the last unit was: the response being
    /// written, the question waiting, the padding.
    tail: f32,

    stream: prose::Stream,
    work: Work,
}

impl Default for ChatView {
    fn default() -> Self {
        Self {
            units: Vec::new(),
            covered: 0,
            head: 0,
            setting: None,
            offset: 0.0,
            viewport: 0.0,
            at_bottom: true,
            jump: false,
            unseen: false,
            shift: 0.0,
            epoch: 0,
            fresh: true,
            tail: 0.0,
            stream: prose::Stream::default(),
            work: Work::default(),
        }
    }
}

impl ChatView {
    /// Forgets the session that was shown. The next one opens at its end.
    pub fn reset(&mut self) {
        *self = Self {
            epoch: self.epoch + 1,
            ..Self::default()
        };
    }

    /// What the last frame did.
    #[cfg(test)]
    pub fn work(&self) -> Work {
        self.work
    }

    /// Whether the view is following the newest message.
    #[cfg(test)]
    pub fn at_bottom(&self) -> bool {
        self.at_bottom
    }

    /// Brings the units in step with `items`. Items are only ever added
    /// at the end, so only the last units can be affected: those are
    /// grouped again, and everything before them is left as it is.
    ///
    /// Returns the first unit that is new or was grouped again.
    fn sync(&mut self, items: &[Item]) -> usize {
        let head = head_print(items);

        if head != self.head || items.len() < self.covered {
            self.reset();

            self.head = head;
        }

        if items.len() == self.covered {
            return self.units.len();
        }

        if !self.at_bottom {
            self.unseen = true;
        }

        // A run of calls may grow, and items that are not drawn may yet
        // be taken into the run that follows them.
        let mut spare = Vec::new();

        while let Some(unit) = self.units.pop() {
            let settled = unit.kind != Kind::Nothing;

            spare.push(unit);

            if settled {
                break;
            }
        }

        let mut index = spare.last().map_or(self.covered, |unit| unit.start);
        let first = self.units.len();

        while index < items.len() {
            let (end, kind) = next_unit(items, index);

            let kept = spare
                .iter()
                .position(|unit| unit.start == index && unit.end == end && unit.kind == kind);

            let unit = match kept {
                Some(position) => spare.swap_remove(position),

                None => Unit {
                    start: index,
                    end,
                    kind,
                    print: 0,
                    height: None,
                    guess: 0.0,
                    ready: None,
                },
            };

            self.units.push(unit);

            index = end;
        }

        self.covered = items.len();

        first
    }

    /// Looks at unit `index` again: if its items are not what they were,
    /// what was worked out for it is dropped and its height guessed anew.
    fn refresh(
        &mut self,
        index: usize,
        items: &[Item],
        live: bool,
        widths: &Widths,
        thinking: bool,
    ) {
        let last = index + 1 == self.units.len();
        let unit = &mut self.units[index];

        let live = live && last;
        let print = unit_print(&items[unit.start..unit.end], unit.kind, live);

        if print == unit.print {
            return;
        }

        unit.print = print;
        unit.height = None;
        unit.ready = None;
        unit.guess = guess(
            &items[unit.start..unit.end],
            unit.kind,
            live,
            widths,
            thinking,
        );
    }

    /// Where each unit begins, from the top of the first; the last entry
    /// is where the last one ends.
    fn tops(&self) -> Vec<f32> {
        let mut tops = Vec::with_capacity(self.units.len() + 1);
        let mut top = 0.0;

        for unit in &self.units {
            tops.push(top);

            top += unit.height();
        }

        tops.push(top);

        tops
    }

    /// Lays out the text of unit `index` if it is not laid out already,
    /// which also tells its true height. Returns how much taller that
    /// made it.
    fn prepare(
        &mut self,
        context: &egui::Context,
        index: usize,
        items: &[Item],
        widths: &Widths,
        thinking: bool,
    ) -> f32 {
        let unit = &mut self.units[index];

        if unit.ready.is_some() {
            return 0.0;
        }

        let before = unit.height();

        match (unit.kind, &items[unit.start]) {
            (
                Kind::Assistant,
                Item::Assistant {
                    text,
                    thinking: thought,
                    ..
                },
            ) => {
                let prepared = prose::prepare(context, text, widths.prose);

                let mut height = 2.0 * AGENT_PADDING + prepared.height() + UNIT_GAP;

                if thinking && !thought.is_empty() {
                    height += FOLD_HEIGHT + space::XS;
                }

                // A height that was measured in drawing is truer: the
                // reasoning may be unfolded.
                unit.height.get_or_insert(height);
                unit.ready = Some(Ready::Prose(prepared));
            }

            (Kind::User, Item::User { text, images }) => {
                let galley = bubble(context, text, widths.bubble);

                // Pictures are sized when they are drawn.
                if images.is_empty() {
                    let height = USER_GAP + galley.size().y + 2.0 * BUBBLE_PADDING.y + UNIT_GAP;

                    unit.height.get_or_insert(height);
                }

                unit.ready = Some(Ready::Bubble(galley));
            }

            (Kind::Notice, Item::Notice { text, tone }) => {
                let galley = notice(context, text, *tone, widths.column);

                unit.height.get_or_insert(galley.size().y + UNIT_GAP);
                unit.ready = Some(Ready::Line(galley));
            }

            _ => return 0.0,
        }

        self.work.prepared += 1;

        self.units[index].height() - before
    }

    /// The first unit that reaches below `y`.
    fn unit_at(tops: &[f32], y: f32) -> usize {
        let units = tops.len().saturating_sub(1);

        // `tops[index + 1]` is where unit `index` ends.
        tops[1..]
            .partition_point(|bottom| *bottom <= y)
            .min(units.saturating_sub(1))
    }

    /// Takes over a new width, theme or scale: every height is void.
    /// The message at the top of the window is found again afterwards,
    /// so that the view stays on what was being read.
    fn adopt(
        &mut self,
        setting: Setting,
        items: &[Item],
        live: bool,
        widths: &Widths,
    ) -> Option<f32> {
        if self.setting == Some(setting) {
            return None;
        }

        let known = self.setting.is_some() && !self.units.is_empty() && !self.at_bottom;

        // The unit at the top of the window, and how far into it the
        // window begins, as a share of its height.
        let anchor = known.then(|| {
            let tops = self.tops();
            let index = Self::unit_at(&tops, self.offset - TOP_PADDING);
            let height = self.units[index].height().max(1.0);

            (
                index,
                ((self.offset - TOP_PADDING - tops[index]) / height).clamp(0.0, 1.0),
            )
        });

        self.setting = Some(setting);

        for index in 0..self.units.len() {
            self.units[index].print = 0;

            self.refresh(index, items, live, widths, setting.show_thinking);
        }

        anchor.map(|(index, share)| {
            let tops = self.tops();

            TOP_PADDING + tops[index] + share * self.units[index].height()
        })
    }
}

/// Draws `session`. `approval` is a question waiting for an answer; the
/// return value is the answer, once given. `show_thinking` is whether the
/// model's reasoning is drawn.
pub fn show(
    ui: &mut Ui,
    view: &mut ChatView,
    session: &mut Session,
    approval: Option<&ApprovalRequest>,
    show_thinking: bool,
) -> Option<Decision> {
    if session.items.is_empty() {
        if view.covered > 0 {
            view.reset();
        }

        view.work = Work::default();

        empty_state(ui);

        return None;
    }

    let context = ui.ctx().clone();
    let running = session.running;

    let width = ui.available_width();
    let column = (width - 2.0 * MIN_MARGIN).clamp(40.0, theme::COLUMN_WIDTH);

    let widths = Widths {
        column,
        prose: (column - GUTTER).min(prose::measure(&context)).max(40.0),
        bubble: (column * USER_SHARE - 2.0 * BUBBLE_PADDING.x).max(40.0),
    };

    let setting = Setting {
        column,
        theme: palette::index(),
        scale: context.pixels_per_point(),
        show_thinking,
        generation: typeset::generation(&context),
    };

    view.work = Work::default();

    let changed = view.sync(&session.items);

    let mut forced = view.adopt(setting, &session.items, running, &widths);

    // The new units, and the last few as well: those may have changed
    // under the same count of items.
    let unsettled = view.units.len().saturating_sub(UNSETTLED);

    for index in changed.min(unsettled)..view.units.len() {
        view.refresh(index, &session.items, running, &widths, show_thinking);
    }

    if view.viewport <= 0.0 {
        view.viewport = ui.available_height();
    }

    let follows = view.at_bottom || view.fresh;

    // Where the window is expected to be, so that what it will show is
    // laid out, and its true height known, before anything is placed.
    let shift = measure_ahead(
        view,
        &context,
        &session.items,
        running,
        &widths,
        show_thinking,
        follows,
    );

    view.shift += shift;

    let total = view.tops().last().copied().unwrap_or(0.0);

    if view.fresh {
        let end = TOP_PADDING + total + view.tail.max(BOTTOM_PADDING) - view.viewport;

        forced = Some(end.max(0.0));
    } else if !follows && view.shift.abs() > 0.25 {
        forced = Some(forced.unwrap_or(view.offset) + view.shift);
    }

    view.shift = 0.0;

    let mut area = ScrollArea::vertical()
        .id_salt(("conversation", view.epoch))
        .auto_shrink(false)
        .stick_to_bottom(true);

    if let Some(offset) = forced {
        area = area.vertical_scroll_offset(offset.max(0.0));
    }

    // Scrolling by hand always wins over being taken to the end.
    if ui.input(|input| input.smooth_scroll_delta.y != 0.0) {
        view.jump = false;
    }

    let mut decision = None;

    let output = area.show_viewport(ui, |ui, viewport| {
        ui.horizontal(|ui| {
            ui.add_space((width - column) / 2.0);

            ui.vertical(|ui| {
                ui.set_width(column);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(TOP_PADDING);

                let model = session.model.as_deref().unwrap_or_default();

                draw_units(
                    ui,
                    view,
                    &mut session.items,
                    viewport,
                    running,
                    model,
                    &widths,
                    show_thinking,
                );

                let tail = ui.cursor().top();

                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = space::XS;

                    if let Some(writing) = &session.writing {
                        writing_message(ui, &mut view.stream, writing, show_thinking, widths.prose);
                    }

                    if let Some(doing) = session.doing.as_ref().filter(|_| running) {
                        doing_line(ui, doing, model);
                    }

                    if let Some(request) = approval {
                        decision = approval_card(ui, request);
                    }
                });

                ui.add_space(BOTTOM_PADDING);

                view.tail = ui.cursor().top() - tail;

                if view.jump {
                    ui.scroll_to_cursor(Some(Align::BOTTOM));
                }
            });
        });
    });

    let furthest = (output.content_size.y - output.inner_rect.height()).max(0.0);

    view.offset = output.state.offset.y;
    view.viewport = output.inner_rect.height();
    view.at_bottom = view.offset >= furthest - 1.0;
    view.fresh = false;

    if view.at_bottom {
        view.jump = false;
        view.unseen = false;
    } else if session.writing.is_some() {
        view.unseen = true;
    }

    forget_far(view);

    if !view.at_bottom && jump_button(ui, output.inner_rect, view.unseen) {
        view.jump = true;

        ui.ctx().request_repaint();
    }

    decision
}

/// Lays out the text of the units the window is about to show, and
/// returns how much the units that begin above the window grew by it.
fn measure_ahead(
    view: &mut ChatView,
    context: &egui::Context,
    items: &[Item],
    live: bool,
    widths: &Widths,
    thinking: bool,
    follows: bool,
) -> f32 {
    if view.units.is_empty() {
        return 0.0;
    }

    // Following the end, the window is the last stretch of the column,
    // whatever the units above it turn out to measure.
    if follows {
        let wanted = view.viewport + MEASURED_MARGIN;
        let mut reached = view.tail;

        for index in (0..view.units.len()).rev() {
            if reached > wanted {
                break;
            }

            view.refresh(index, items, live, widths, thinking);
            view.prepare(context, index, items, widths, thinking);

            reached += view.units[index].height();
        }

        return 0.0;
    }

    let top = view.offset + view.shift - TOP_PADDING;
    let tops = view.tops();

    let first = ChatView::unit_at(&tops, top - MEASURED_MARGIN);
    let end = top + view.viewport + MEASURED_MARGIN;

    let mut at = tops[first];
    let mut shift = 0.0;

    for index in first..view.units.len() {
        if at > end {
            break;
        }

        view.refresh(index, items, live, widths, thinking);

        let grew = view.prepare(context, index, items, widths, thinking);

        if at < top {
            shift += grew;
        }

        at += view.units[index].height();
    }

    shift
}

/// Draws the units that fall in or near `viewport`, and leaves empty
/// room of the right height for the rest.
#[allow(clippy::too_many_arguments)]
fn draw_units(
    ui: &mut Ui,
    view: &mut ChatView,
    items: &mut [Item],
    viewport: egui::Rect,
    running: bool,
    model: &str,
    widths: &Widths,
    show_thinking: bool,
) {
    let tops = view.tops();

    view.work.units = view.units.len();

    let Some(total) = tops.last().copied().filter(|_| !view.units.is_empty()) else {
        return;
    };

    // The window, measured from the top of the first unit.
    let window_top = viewport.min.y - TOP_PADDING;
    let window_bottom = viewport.max.y - TOP_PADDING;

    let first = ChatView::unit_at(&tops, window_top - DRAWN_MARGIN);

    ui.add_space(tops[first]);

    let mut index = first;
    let mut at = tops[first];

    while index < view.units.len() && at <= window_bottom + DRAWN_MARGIN {
        view.refresh(index, items, running, widths, show_thinking);

        let context = ui.ctx().clone();

        view.prepare(&context, index, items, widths, show_thinking);

        let last = index + 1 == view.units.len();
        let unit = &view.units[index];
        let assumed = unit.height();

        let before = ui.cursor().top();

        if unit.kind != Kind::Nothing {
            ui.push_id(unit.start, |ui| {
                ui.spacing_mut().item_spacing.y = space::XS;

                draw_unit(
                    ui,
                    unit,
                    &mut items[unit.start..unit.end],
                    running && last,
                    model,
                    widths,
                    show_thinking,
                );
            });

            ui.add_space(gap_under(unit.kind));
        }

        let drawn = ui.cursor().top() - before;

        view.units[index].height = Some(drawn);

        // What begins above the window and turns out a different height
        // would push what is being read: the scroll position makes up
        // for it on the next frame.
        if (drawn - assumed).abs() > 0.25 && at < window_top {
            view.shift += drawn - assumed;
        }

        view.work.drawn += 1;

        at += assumed;
        index += 1;
    }

    ui.add_space((total - tops[index.min(tops.len() - 1)]).max(0.0));
}

/// The room left under a unit of this kind. It is added by whoever
/// draws the unit, after it, so that it counts towards the unit's height.
fn gap_under(kind: Kind) -> f32 {
    match kind {
        Kind::Nothing => 0.0,
        Kind::Assistant => AGENT_PADDING + UNIT_GAP,
        _ => UNIT_GAP,
    }
}

fn draw_unit(
    ui: &mut Ui,
    unit: &Unit,
    items: &mut [Item],
    live: bool,
    model: &str,
    widths: &Widths,
    show_thinking: bool,
) {
    match (unit.kind, &unit.ready) {
        (Kind::Nothing, _) => {}

        (Kind::Briefings, _) => briefings(ui, items),
        (Kind::Tools, _) => tool_card::show_group(ui, items, live, show_thinking),

        (Kind::User, Some(Ready::Bubble(galley))) => {
            if let Item::User { text, images } = &items[0] {
                user_message(ui, text, images, galley);
            }
        }

        (Kind::Assistant, Some(Ready::Prose(prepared))) => {
            if let Item::Assistant {
                step,
                text,
                thinking,
                duration_ms,
                speed,
            } = &items[0]
            {
                ui.spacing_mut().item_spacing.y = 0.0;

                if show_thinking && !thinking.is_empty() {
                    thought(ui, thinking);

                    ui.add_space(space::XS);
                }

                let meta = meta_line(*step, *duration_ms, *speed);

                agent_message(ui, text, prepared, &meta, widths);
            }
        }

        (Kind::Notice, Some(Ready::Line(galley))) => {
            ui.add(egui::Label::new(galley.clone()).selectable(true));
        }

        _ => show_item(ui, &mut items[0], model),
    }
}

/// Drops the laid-out text of units that are far from the window. Their
/// heights stay; the text is found again in the kept layouts if they are
/// scrolled back to.
fn forget_far(view: &mut ChatView) {
    let near = 3.0 * MEASURED_MARGIN + view.viewport;
    let mut top = TOP_PADDING;

    for unit in &mut view.units {
        let height = unit.height();

        if unit.ready.is_some() && (top + height < view.offset - near || top > view.offset + near) {
            unit.ready = None;
        }

        top += height;
    }
}

/// A round button over the foot of the conversation that goes to the
/// newest message. `unseen` marks it when something has arrived since
/// the view was scrolled away. Returns whether it was clicked.
fn jump_button(ui: &mut Ui, window: egui::Rect, unseen: bool) -> bool {
    let centre = egui::pos2(
        window.center().x,
        window.bottom() - JUMP_SIZE / 2.0 - space::M,
    );
    let rect = egui::Rect::from_center_size(centre, egui::vec2(JUMP_SIZE, JUMP_SIZE));

    let response = ui
        .interact(rect, ui.id().with("jump to latest"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    let fill = if response.hovered() {
        palette::raised()
    } else {
        palette::surface()
    };

    let painter = ui.painter();

    painter.circle_filled(centre, JUMP_SIZE / 2.0, fill);
    painter.circle_stroke(centre, JUMP_SIZE / 2.0, Stroke::new(1.0, palette::border()));

    if response.has_focus() {
        painter.circle_stroke(centre, JUMP_SIZE / 2.0 + 1.5, theme::focus_ring());
    }

    let stroke = Stroke::new(1.5, palette::text());
    let tip = centre + egui::vec2(0.0, 5.0);

    painter.line_segment([centre - egui::vec2(0.0, 5.5), tip], stroke);
    painter.line_segment([tip, tip + egui::vec2(-4.5, -4.5)], stroke);
    painter.line_segment([tip, tip + egui::vec2(4.5, -4.5)], stroke);

    if unseen {
        painter.circle_filled(
            centre + egui::vec2(JUMP_SIZE / 2.0 - 4.0, -JUMP_SIZE / 2.0 + 4.0),
            4.0,
            palette::accent(),
        );
    }

    let tooltip = if unseen {
        "Go to the latest: there is more below"
    } else {
        "Go to the latest"
    };

    response.on_hover_text(tooltip).clicked()
}

/// Which unit begins at `start`, and where it ends.
fn next_unit(items: &[Item], start: usize) -> (usize, Kind) {
    let briefed = briefings_end(items, start);

    if briefed > start {
        return (briefed, Kind::Briefings);
    }

    let end = activity_end(items, start);

    if end > start {
        return (end, Kind::Tools);
    }

    let kind = match &items[start] {
        Item::User { .. } => Kind::User,

        // A step that only called tools has nothing to say; its calls
        // are drawn as a table.
        Item::Assistant { text, .. } if text.is_empty() => Kind::Nothing,
        Item::Assistant { .. } => Kind::Assistant,

        Item::Notice {
            tone: Tone::Housekeeping,
            ..
        } => Kind::Nothing,

        Item::Notice { .. } => Kind::Notice,
        Item::Checks { .. } => Kind::Checks,
        Item::Review { .. } => Kind::Review,
        Item::RunSummary { .. } => Kind::Summary,

        // Both are drawn in runs, which were looked for above.
        Item::Briefing { .. } | Item::Tool(_) => Kind::Nothing,
    };

    (start + 1, kind)
}

/// A mark of a session's first item. Another session begins otherwise.
fn head_print(items: &[Item]) -> u64 {
    const LOOKED_AT: usize = 96;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    match items.first() {
        None => 0_u8.hash(&mut hasher),

        Some(Item::User { text, .. }) => {
            let bytes = text.as_bytes();

            (1_u8, text.len(), &bytes[..bytes.len().min(LOOKED_AT)]).hash(&mut hasher);
        }

        Some(Item::Briefing { source, text, .. }) => {
            (2_u8, source, text.len()).hash(&mut hasher);
        }

        Some(Item::Notice { text, .. }) => (3_u8, text).hash(&mut hasher),
        Some(_) => 4_u8.hash(&mut hasher),
    }

    hasher.finish()
}

/// A mark of what a unit's items hold, from their sizes and switches
/// alone: no text is read, so it is cheap enough to take every frame.
fn unit_print(items: &[Item], kind: Kind, live: bool) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    (kind, live, items.len()).hash(&mut hasher);

    for item in items {
        match item {
            Item::User { text, images } => (0_u8, text.len(), images.len()).hash(&mut hasher),

            Item::Briefing { text, open, .. } => (1_u8, text.len(), open).hash(&mut hasher),

            Item::Assistant { text, thinking, .. } => {
                (2_u8, text.len(), thinking.len()).hash(&mut hasher);
            }

            Item::Tool(tool) => {
                let result = tool
                    .result
                    .as_ref()
                    .map(|result| (result.is_error, result.output.len()));

                (3_u8, tool.open, result, tool.duration_ms).hash(&mut hasher);
            }

            Item::Checks {
                passed,
                checks,
                open,
            } => (4_u8, passed, checks.len(), open).hash(&mut hasher),

            Item::Review {
                requirements,
                sent_back,
                open,
            } => (5_u8, requirements.len(), sent_back, open).hash(&mut hasher),

            Item::Notice { text, tone } => {
                (6_u8, text.len(), *tone == Tone::Bad).hash(&mut hasher);
            }

            Item::RunSummary {
                steps, duration_ms, ..
            } => (7_u8, steps, duration_ms).hash(&mut hasher),
        }
    }

    // Never the mark of a unit that has not been looked at.
    hasher.finish() | 1
}

/// Roughly how tall a unit is, without laying anything out.
fn guess(items: &[Item], kind: Kind, live: bool, widths: &Widths, thinking: bool) -> f32 {
    let row = size::BODY * theme::LINE_HEIGHT;

    let height = match (kind, &items[0]) {
        (Kind::Nothing, _) => return 0.0,

        (Kind::User, Item::User { text, images }) => {
            let pictures = if images.is_empty() {
                0.0
            } else {
                PICTURE_SIZE.y + space::XS
            };

            USER_GAP
                + pictures
                + prose::estimate_height(text, widths.bubble)
                + 2.0 * BUBBLE_PADDING.y
        }

        (
            Kind::Assistant,
            Item::Assistant {
                text,
                thinking: thought,
                ..
            },
        ) => {
            let fold = if thinking && !thought.is_empty() {
                FOLD_HEIGHT + space::XS
            } else {
                0.0
            };

            2.0 * AGENT_PADDING + fold + prose::estimate_height(text, widths.prose)
        }

        (Kind::Notice, Item::Notice { text, .. }) => {
            prose::estimate_height(text, widths.column) * (size::SMALL / size::BODY)
        }

        (Kind::Tools, _) => {
            // Folded to its one line unless it is still running.
            let rows = if live { items.len() as f32 } else { 0.0 };

            tool_card::HEADER_HEIGHT + rows * (tool_card::ROW_HEIGHT + 1.0)
        }

        (Kind::Checks, Item::Checks { checks, open, .. }) => {
            let unfolded = if *open {
                checks.len() as f32 * (row + 60.0)
            } else {
                0.0
            };

            22.0 + unfolded
        }

        (
            Kind::Review,
            Item::Review {
                requirements, open, ..
            },
        ) => {
            let unfolded = if *open {
                requirements.len() as f32 * 2.0 * row
            } else {
                0.0
            };

            22.0 + unfolded
        }

        (Kind::Summary, _) => 2.0 + row + 9.0 + 2.0 * space::XS,
        _ => FOLD_HEIGHT,
    };

    height + UNIT_GAP
}

/// What the person said, laid out as narrow as it can be without taking
/// another line: the bubble is then no wider than its longest line, and
/// its lines are of even length rather than one long and one short.
fn bubble(context: &egui::Context, text: &str, widest: f32) -> Arc<Galley> {
    let job = |width: f32| {
        let mut job = LayoutJob::single_section(
            text.to_string(),
            TextFormat {
                font_id: theme::body(),
                color: palette::text(),
                line_height: Some(size::BODY * theme::LINE_HEIGHT),
                ..TextFormat::default()
            },
        );

        job.wrap = TextWrapping::wrap_at_width(width);

        job
    };

    let lay_out = |width: f32| typeset::direct(context, job(width));

    // Kept under the widest it may be: the search below is run once.
    typeset::layout_at(context, typeset::hash(("bubble", text)), widest, || {
        let loose = lay_out(widest);
        let rows = loose.rows.len();

        if rows <= 1 {
            return job(widest);
        }

        // The narrowest width that still fits the text in as many lines.
        let mut narrow = loose.size().x / 2.0;
        let mut wide = widest;

        for _ in 0..7 {
            let middle = (narrow + wide) / 2.0;

            if lay_out(middle).rows.len() <= rows {
                wide = middle;
            } else {
                narrow = middle;
            }
        }

        job(wide.ceil())
    })
}

/// A notice: one quiet line, or a few.
fn notice(context: &egui::Context, text: &str, tone: Tone, width: f32) -> Arc<Galley> {
    let color = match tone {
        Tone::Bad => palette::bad(),
        Tone::Muted | Tone::Housekeeping => palette::text_muted(),
    };

    typeset::layout_at(
        context,
        typeset::hash(("notice", text, tone == Tone::Bad)),
        width,
        || LayoutJob::simple(text.to_string(), theme::small(), color, width),
    )
}

/// A step's number, how long the model took over it and how fast it
/// wrote, as far as they are known.
fn meta_line(step: u32, duration_ms: u64, speed: Option<f32>) -> String {
    let mut parts = vec![format!("step {step}")];

    if duration_ms > 0 {
        parts.push(format::duration(duration_ms));
    }

    if let Some(speed) = speed {
        parts.push(format!("{speed:.0} tok/s"));
    }

    parts.join(" · ")
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
            source,
            text,
            open: true,
        } = item
        {
            code_block(ui, &source.replace('_', " "), text, MAX_CHECK_OUTPUT_HEIGHT);
        }
    }
}

fn empty_state(ui: &mut Ui) {
    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        ui.add_space(ui.available_height() * 0.28);

        ui.label(
            RichText::new("A workspace for your next idea")
                .font(theme::strong(size::DISPLAY - 2.0)),
        );

        ui.add_space(space::XXS);

        ui.label(
            RichText::new("Ask a question, explore your code, or give an agent a task.")
                .color(palette::text_muted()),
        );

        ui.add_space(space::L);

        ui.label(
            RichText::new(
                "Return sends  ·  Shift+Return starts a new line  ·  Up brings back your last \
                 message  ·  drop pictures to attach them",
            )
            .font(theme::small())
            .color(palette::text_muted()),
        );
    });
}

/// The items that are drawn one at a time and hold no long text.
fn show_item(ui: &mut Ui, item: &mut Item, model: &str) {
    match item {
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

        Item::RunSummary {
            outcome,
            verified,
            steps,
            usage,
            duration_ms,
        } => run_summary(ui, *outcome, *verified, *steps, usage, *duration_ms, model),

        // Drawn from text laid out ahead, or in runs, or not at all.
        Item::User { .. }
        | Item::Assistant { .. }
        | Item::Notice { .. }
        | Item::Briefing { .. }
        | Item::Tool(_) => {}
    }
}

/// What the person said: a bubble set to the right, as wide as its text
/// and no wider than its share of the column, under any pictures sent
/// with it.
fn user_message(ui: &mut Ui, text: &str, images: &[Image], galley: &Arc<Galley>) {
    ui.add_space(USER_GAP);

    if !images.is_empty() {
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;

            // Laid out from the right, so the last is placed first.
            for image in images.iter().rev() {
                picture(ui, image);
            }
        });
    }

    let size = galley.size() + 2.0 * BUBBLE_PADDING;

    let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), size.y), Sense::hover());

    if !ui.is_rect_visible(row) {
        return;
    }

    let bubble = egui::Rect::from_min_size(egui::pos2(row.right() - size.x, row.top()), size);

    ui.painter()
        .rect_filled(bubble, CornerRadius::same(radius::XL), palette::raised());

    // A label rather than painted text, so that it can be selected.
    let mut inside = ui.new_child(
        UiBuilder::new()
            .id_salt("bubble")
            .max_rect(bubble.shrink2(BUBBLE_PADDING))
            .layout(Layout::top_down(Align::Min)),
    );

    inside.add(egui::Label::new(galley.clone()).selectable(true));

    if ui.rect_contains_pointer(row) {
        let place = egui::Rect::from_center_size(
            egui::pos2(bubble.left() - 16.0, bubble.top() + 15.0),
            egui::vec2(22.0, 22.0),
        );

        copy_button(ui, place, text, "Copy message");
    }
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

    attachment::paint_cover(ui, rect, &texture, radius::L);

    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(radius::L),
        Stroke::new(1.0, palette::border()),
        egui::StrokeKind::Inside,
    );
}

/// What the agent said: plain text on the left, with no frame around it.
/// While the pointer is over it a copy button shows in the gutter, and
/// beside that, where there is room, a quiet line about the step.
fn agent_message(ui: &mut Ui, text: &str, prepared: &prose::Prepared, meta: &str, widths: &Widths) {
    ui.add_space(AGENT_PADDING);

    let top = ui.cursor().min;

    prose::paint(ui, prepared);

    let row = egui::Rect::from_min_max(top, egui::pos2(top.x + widths.column, ui.cursor().top()));

    if !ui.rect_contains_pointer(row) {
        return;
    }

    // Kept in sight on a message taller than the window.
    let visible = row.intersect(ui.clip_rect());

    let place = egui::Rect::from_min_size(
        egui::pos2(row.right() - 22.0, visible.top().min(row.bottom() - 22.0)),
        egui::vec2(22.0, 22.0),
    );

    copy_button(ui, place, text, &format!("Copy message\n{meta}"));

    // Only where the prose leaves the right of the column free.
    let free = widths.column - GUTTER - widths.prose;

    let galley =
        ui.painter()
            .layout_no_wrap(meta.to_string(), theme::small(), palette::text_muted());

    if galley.size().x + space::M <= free {
        ui.painter().galley(
            egui::pos2(
                place.left() - space::S - galley.size().x,
                place.center().y - galley.size().y / 2.0,
            ),
            galley,
            palette::text_muted(),
        );
    }
}

fn copy_button(ui: &Ui, place: egui::Rect, text: &str, tooltip: &str) {
    let id = ui.id().with("copy");

    if icons::button_at(ui, place, id, Icon::Copy, tooltip, false).clicked() {
        ui.ctx().copy_text(text.to_string());
    }
}

/// The response the model is writing right now. Until its text begins,
/// the newest of its reasoning is shown as it arrives; after that the
/// reasoning folds to a line.
fn writing_message(
    ui: &mut Ui,
    stream: &mut prose::Stream,
    writing: &Writing,
    show_thinking: bool,
    width: f32,
) {
    if show_thinking && !writing.thinking.is_empty() {
        if writing.text.is_empty() {
            ui.label(
                RichText::new("Thinking…")
                    .size(size::UI)
                    .color(palette::text_secondary()),
            );

            thinking_text(ui, &newest(&writing.thinking, LIVE_THINKING_CHARS));
        } else {
            thought(ui, &writing.thinking);
        }
    }

    if !writing.text.is_empty() {
        ui.add_space(AGENT_PADDING);

        stream.update(ui.ctx(), &writing.text, width);
        stream.paint(ui);
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
            .size(size::UI)
            .color(palette::text_secondary().gamma_multiply(pulse)),
    );
}

/// The end of `text`, from a word boundary, with a mark that more came
/// before it. Only the end is looked at, however long the text has grown.
fn newest(text: &str, max_chars: usize) -> std::borrow::Cow<'_, str> {
    // No character takes more than four bytes.
    let mut start = text.len().saturating_sub(max_chars * 4);

    while !text.is_char_boundary(start) {
        start += 1;
    }

    let tail = &text[start..];
    let count = tail.chars().count();

    if start == 0 && count <= max_chars {
        return text.into();
    }

    let skipped = count.saturating_sub(max_chars);

    let tail = tail
        .char_indices()
        .nth(skipped)
        .map_or(tail, |(at, _)| &tail[at..]);

    let from_word = tail.find(' ').map_or(tail, |space| &tail[space + 1..]);

    format!("… {from_word}").into()
}

/// A line that unfolds into the model's reasoning for a step.
fn thought(ui: &mut Ui, thinking: &str) {
    let id = ui.id().with("thought");

    let open: bool = ui.data(|data| data.get_temp(id)).unwrap_or(false);

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), FOLD_HEIGHT),
        Sense::click(),
    );

    let color = if response.hovered() || response.has_focus() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    let painter = ui.painter_at(rect);

    let label = painter.text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        "Thought",
        theme::ui(),
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
                code_block(ui, "output", &check.output, MAX_CHECK_OUTPUT_HEIGHT);
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
    let color = if response.hovered() || response.has_focus() {
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

    ui.add_space(space::XXS);

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

            ui.label(
                RichText::new(format!("Allow {}?", request.tool)).font(theme::strong(size::BODY)),
            );

            code_block(ui, "request", &request.summary, 120.0);

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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    const WINDOW: egui::Vec2 = egui::vec2(1000.0, 800.0);

    struct Harness {
        context: egui::Context,
        view: ChatView,
        session: Session,
    }

    impl Harness {
        fn new(exchanges: usize) -> Self {
            let context = egui::Context::default();

            theme::apply(&context);

            let mut session = Session::default();

            for number in 0..exchanges {
                add_exchange(&mut session, number);
            }

            Self {
                context,
                view: ChatView::default(),
                session,
            }
        }

        /// Draws one frame and says how long it took.
        fn frame(&mut self, events: Vec<egui::Event>) -> Duration {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, WINDOW)),
                events,
                ..Default::default()
            };

            let started = Instant::now();

            let mut output = self.context.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    show(ui, &mut self.view, &mut self.session, None, true);
                });
            });

            let took = started.elapsed();

            output.textures_delta.clear();

            took
        }

        fn frames(&mut self, count: usize) {
            for _ in 0..count {
                self.frame(Vec::new());
            }
        }

        /// Scrolls up by `points`, with the pointer over the conversation.
        fn scroll_up(&mut self, points: f32) {
            let over = egui::pos2(WINDOW.x / 2.0, WINDOW.y / 2.0);

            self.frame(vec![
                egui::Event::PointerMoved(over),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, points),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);

            // Smooth scrolling spreads it over a few frames.
            self.frames(20);
        }
    }

    fn add_exchange(session: &mut Session, number: usize) {
        session.items.push(Item::User {
            text: format!("Question number {number}: what does this part of the code do?"),
            images: Vec::new(),
        });

        session.items.push(Item::Assistant {
            step: 1,
            text: format!(
                "Answer {number}. It reads the settings, checks them, and hands them on.\n\n\
                 - first it opens the file\n- then it parses it\n\n\
                 ```rust\nfn answer() -> u32 {{ {number} }}\n```\n\nThat is all it does."
            ),
            thinking: String::new(),
            duration_ms: 1_200,
            speed: Some(40.0),
        });
    }

    #[test]
    fn a_long_conversation_costs_a_frame_what_a_short_one_does() {
        let mut short = Harness::new(25);
        let mut long = Harness::new(2_500);

        // Settling: the end is laid out once.
        short.frames(5);
        long.frames(5);

        let short_work = short.view.work();

        eprintln!("work for 50 items: {short_work:?}");
        let long_work = long.view.work();

        eprintln!("work for 5,000 items: {long_work:?}");

        assert_eq!(long_work.units, 5_000);

        // Only what is on screen and near it is drawn, however long the
        // conversation is.
        assert!(long_work.drawn <= 30, "{long_work:?}");
        assert!(
            long_work.drawn <= short_work.drawn + 4,
            "{short_work:?} {long_work:?}"
        );

        // And once laid out, nothing is laid out again.
        assert_eq!(long_work.prepared, 0, "{long_work:?}");

        let time = |harness: &mut Harness| {
            let frames = 30;
            let total: Duration = (0..frames).map(|_| harness.frame(Vec::new())).sum();

            total / frames
        };

        let short_time = time(&mut short);
        let long_time = time(&mut long);

        eprintln!("a frame of 50 items: {short_time:?}; of 5,000 items: {long_time:?}");

        // Generous, for a loaded machine: it is a few times, not a hundred.
        assert!(
            long_time < short_time * 8 + Duration::from_millis(2),
            "{short_time:?} {long_time:?}"
        );
    }

    #[test]
    fn a_conversation_opens_at_its_end() {
        let mut harness = Harness::new(400);

        harness.frames(3);

        assert!(harness.view.at_bottom());

        let last = harness.view.units.len() - 1;

        assert!(
            harness.view.units[last].height.is_some(),
            "the end was drawn"
        );
        assert!(harness.view.units[0].height.is_none(), "the start was not");
    }

    #[test]
    fn new_messages_are_followed_at_the_end_and_never_yank_a_reader_back() {
        let mut harness = Harness::new(300);

        harness.frames(3);

        // At the end, the view follows what arrives.
        add_exchange(&mut harness.session, 300);
        harness.frames(3);

        assert!(harness.view.at_bottom());

        // Scrolled up, it stays exactly where the reader is.
        harness.scroll_up(2_000.0);

        assert!(!harness.view.at_bottom());

        let reading = harness.view.offset;

        for number in 301..306 {
            add_exchange(&mut harness.session, number);
            harness.frames(2);
        }

        assert!(
            (harness.view.offset - reading).abs() < 1.0,
            "{} then {}",
            reading,
            harness.view.offset
        );
        assert!(harness.view.unseen);

        // Scrolling up into messages never drawn before does not make
        // what is on screen jump either: their heights are measured
        // before they come into view.
        let before = harness.view.offset;

        harness.scroll_up(600.0);

        let moved = before - harness.view.offset;

        assert!(
            (moved - 600.0).abs() < 40.0,
            "moved {moved} for a scroll of 600"
        );

        // The button takes the reader to the latest.
        harness.view.jump = true;
        harness.frames(40);

        assert!(harness.view.at_bottom());
        assert!(!harness.view.unseen);
    }

    #[test]
    fn another_session_starts_afresh() {
        let mut harness = Harness::new(50);

        harness.frames(2);

        let mut other = Harness::new(0).session;

        other.items.push(Item::User {
            text: "A different first question".to_string(),
            images: Vec::new(),
        });

        harness.session = other;
        harness.frames(2);

        assert_eq!(harness.view.units.len(), 1);
        assert!(harness.view.at_bottom());
    }

    #[test]
    fn units_follow_the_items_as_they_arrive() {
        let tool = |id: &str| {
            Item::Tool(crate::session::ToolItem {
                call: cortex_core::ToolCall {
                    id: id.to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({ "path": "a.rs" }),
                },
                result: None,
                duration_ms: 0,
                open: false,
            })
        };

        let empty_step = Item::Assistant {
            step: 1,
            text: String::new(),
            thinking: String::new(),
            duration_ms: 0,
            speed: None,
        };

        let mut view = ChatView::default();
        let mut items = vec![Item::User {
            text: "Go".to_string(),
            images: Vec::new(),
        }];

        view.sync(&items);

        // A step that said nothing, then the calls it made: one run.
        items.push(empty_step.clone());
        view.sync(&items);

        items.push(tool("a"));
        view.sync(&items);

        items.push(empty_step);
        items.push(tool("b"));
        view.sync(&items);

        let kinds: Vec<(usize, usize, Kind)> = view
            .units
            .iter()
            .map(|unit| (unit.start, unit.end, unit.kind))
            .collect();

        assert_eq!(kinds, [(0, 1, Kind::User), (1, 5, Kind::Tools)]);

        // As if grouped all at once.
        let mut whole = ChatView::default();

        whole.sync(&items);

        let all_at_once: Vec<(usize, usize, Kind)> = whole
            .units
            .iter()
            .map(|unit| (unit.start, unit.end, unit.kind))
            .collect();

        assert_eq!(kinds, all_at_once);
    }

    #[test]
    fn the_newest_of_a_long_thought_is_found_from_its_end() {
        let long = format!("{} the very end", "word ".repeat(10_000));

        let tail = newest(&long, 40);

        assert!(tail.starts_with("… "));
        assert!(tail.ends_with("the very end"));
        assert!(tail.chars().count() <= 44);

        assert_eq!(newest("short", 40), "short");
    }

    #[test]
    fn a_short_message_is_a_narrow_bubble_and_a_long_one_is_balanced() {
        let context = egui::Context::default();

        theme::apply(&context);

        let mut output = context.run_ui(egui::RawInput::default(), |_| {});

        output.textures_delta.clear();

        let short = bubble(&context, "Thanks!", 500.0);

        assert!(short.size().x < 100.0);

        let text = "Please look at the settings loader and tell me why it forgets the theme \
                    the person picked whenever the app starts again after an update";

        let loose = typeset::direct(&context, {
            let mut job =
                LayoutJob::simple(text.to_string(), theme::body(), palette::text(), 500.0);

            job.wrap = TextWrapping::wrap_at_width(500.0);

            job
        });

        let balanced = bubble(&context, text, 500.0);

        // As many lines as it would have taken anyway, but no wider than
        // needed for them.
        assert_eq!(balanced.rows.len(), loose.rows.len());
        assert!(
            balanced.size().x < loose.size().x - 20.0,
            "{} {}",
            balanced.size().x,
            loose.size().x
        );
    }
}

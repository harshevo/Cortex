//! The memory space: the brain, and what is written on top of it.

use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Ui, UiBuilder, vec2,
};

use crate::brain::{self, Scene, anatomy};
use crate::format;
use crate::memory::graph::{Graph, Lasting, Node, Thought, Tie};
use crate::memory::reader::Placement;
use crate::memory::{Region, Remembered};
use crate::theme::{self, palette};

const MARGIN: f32 = 18.0;
const CARD_WIDTH: f32 = 320.0;

/// Longest frame the clock will step over, in seconds. A stall (the
/// window was hidden, say) should not make the animation jump.
const LONGEST_STEP: f32 = 0.05;

/// Zoom per point of scrolling.
const SCROLL_ZOOM: f32 = 0.0022;

const LABEL_FILL: Color32 = Color32::from_rgba_premultiplied(8, 8, 14, 205);
const CARD_FILL: Color32 = Color32::from_rgba_premultiplied(18, 18, 24, 238);

/// The memories and how they are being shown.
#[derive(Default)]
pub struct MemoryView {
    /// How many episodes the engine holds.
    episodes: usize,
    pub activity: crate::brain::activity::ActivityView,

    /// Episodes and facts, arranged for drawing.
    lasting: Lasting,

    /// A newer `lasting` being arranged on another thread.
    arranging: Option<Receiver<Lasting>>,

    /// What the agent has in mind in the current run.
    thoughts: Vec<Thought>,

    /// What decided where each episode sits.
    placement: Placement,

    /// The whole of the intuition's network, of which only the best
    /// connected part is drawn: neurons, synapses, runs learned from.
    intuition: (usize, usize, u32),

    /// Why the engine's memory could not be read.
    pub problem: Option<String>,

    scene: Scene,

    /// Memory has been shown once, so anything new is born on screen
    /// rather than simply being there.
    shown: bool,

    /// Where the card was drawn last frame, so the brain underneath
    /// ignores the pointer there.
    card: Option<Rect>,

    /// Select the newest episode as soon as it has been placed.
    select_newest: bool,
}

impl MemoryView {
    pub fn episode_count(&self) -> usize {
        self.episodes
    }

    /// Takes in what the engine remembers now.
    ///
    /// Arranging it compares every episode with every other, which takes
    /// long enough, once there are many, to make the window stutter. So
    /// that is done on another thread and picked up when it is ready.
    pub fn load(&mut self, remembered: Remembered) {
        self.episodes = remembered.total_episodes.max(remembered.episodes.len());
        let checked = remembered
            .episodes
            .iter()
            .filter(|e| e.verified.is_some())
            .count();
        let passed = remembered
            .episodes
            .iter()
            .filter(|e| e.verified == Some(true))
            .count();
        let first = remembered
            .episodes
            .iter()
            .filter(|e| e.verified == Some(true) && e.rejections == 0)
            .count();
        let mut steps: Vec<_> = remembered.episodes.iter().map(|e| e.steps).collect();
        steps.sort_unstable();
        self.activity.performance = if checked == 0 {
            format!("{} stored tasks · no checked outcomes yet", self.episodes)
        } else {
            format!(
                "Checks passed {passed}/{checked} · first attempt {first}/{checked} · median {} steps (observed tasks, not a controlled accuracy comparison)",
                steps.get(steps.len() / 2).unwrap_or(&0)
            )
        };
        self.placement = remembered.placement.clone();

        let intuition = &remembered.intuition;

        self.intuition = (
            intuition.total_neurons,
            intuition.total_synapses,
            intuition.runs,
        );
        self.problem = None;

        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            // The window may have moved on to newer memory by now.
            let _ = sender.send(Lasting::arrange(&remembered));
        });

        self.arranging = Some(receiver);
    }

    pub fn set_thoughts(&mut self, thoughts: Vec<Thought>) {
        if self.thoughts != thoughts {
            self.thoughts = thoughts;

            self.rebuild_graph();
        }
    }

    pub fn select_newest(&mut self) {
        self.select_newest = true;
    }

    fn receive_arrangement(&mut self) {
        let Some(receiver) = &self.arranging else {
            return;
        };

        let Ok(lasting) = receiver.try_recv() else {
            return;
        };

        self.lasting = lasting;
        self.arranging = None;

        self.rebuild_graph();

        self.shown = true;

        if std::mem::take(&mut self.select_newest) {
            self.scene
                .select(self.lasting.episode_count().checked_sub(1));
        }
    }

    pub fn observe_brain(&mut self, source: &str, step: u32, detail: &serde_json::Value, now: f64) {
        self.activity.observe(source, step, detail, now);
        self.intuition = (
            self.activity.intuition.total_neurons,
            self.activity.intuition.total_synapses,
            self.activity.intuition.runs,
        );
        self.rebuild_graph();
    }

    /// Takes in the engine's answer to a question the inspector asked.
    pub fn inspected(&mut self, states: Vec<serde_json::Value>) {
        if self.activity.inspected(states) {
            self.rebuild_graph();
        }
    }

    fn rebuild_graph(&mut self) {
        let mut graph = Graph::assemble(&self.lasting, &self.thoughts);
        graph.add_activity(&self.activity);

        self.scene
            .set_graph(graph, &self.lasting.episode_points, self.shown);
    }
}

/// Draws the memory space. `thinking` is whether a run is in progress.
pub fn show(ui: &mut Ui, view: &mut MemoryView, thinking: bool) {
    view.receive_arrangement();
    if view.activity.advance(ui.input(|i| i.time)) {
        view.rebuild_graph();
    }

    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());

    steer(ui, view, rect, &response);

    let seconds = ui.input(|input| input.stable_dt).min(LONGEST_STEP);

    view.scene.advance(seconds, thinking);

    let frame = view.scene.frame(rect, ui.pixels_per_point());

    ui.painter_at(rect).add(brain::paint(rect, frame));

    region_names(ui, view, rect);
    node_names(ui, view, rect);
    heading(ui, view, rect);

    view.card = view
        .scene
        .selected()
        .map(|index| card(ui, view, index, rect));

    let before = view.activity.frame();
    egui::Window::new("Brain inspector")
        .id(egui::Id::new("brain-inspector"))
        .default_width(360.0)
        .default_height(620.0)
        .max_height((rect.height() - 100.0).max(250.0))
        .default_pos(rect.right_top() + vec2(-390.0, 80.0))
        .vscroll(true)
        .show(ui.ctx(), |ui| {
            view.activity.show(ui);
        });

    if before != view.activity.frame() {
        view.rebuild_graph();
    }

    // The brain is always moving.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(if thinking {
            33
        } else {
            100
        }));
}

/// Applies the pointer to the scene: drag to orbit, scroll to zoom, hover
/// to name a memory, click to select one.
fn steer(ui: &Ui, view: &mut MemoryView, rect: Rect, response: &egui::Response) {
    let over_card = |position: Pos2| view.card.is_some_and(|card| card.contains(position));

    let pointer = response
        .hover_pos()
        .filter(|position| !over_card(*position));

    if response.dragged() {
        let delta = response.drag_delta();

        view.scene.orbit(delta.x, delta.y);
    }

    if pointer.is_some() {
        let (scroll, pinch) = ui.input(|input| (input.smooth_scroll_delta.y, input.zoom_delta()));

        view.scene.zoom((-scroll * SCROLL_ZOOM).exp() / pinch);
    }

    let under_pointer = pointer.and_then(|position| view.scene.pick(position, rect));

    view.scene.set_hovered(under_pointer);

    if under_pointer.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if response.clicked() && pointer.is_some() {
        view.scene.select(under_pointer);
    }
}

/// Writes each region's name, and how much it holds, beside it.
fn region_names(ui: &Ui, view: &MemoryView, rect: Rect) {
    // With a memory selected, the names of its connections matter more.
    if view.scene.selected().is_some() {
        return;
    }

    let painter = ui.painter_at(rect);
    let graph = view.scene.graph();

    for region in Region::ALL {
        let Some(position) = view.scene.project(anatomy::label_anchor(region), rect) else {
            continue;
        };

        painter.text(
            position,
            Align2::CENTER_CENTER,
            region.label(),
            egui::FontId::proportional(13.0),
            palette::region(region),
        );

        painter.text(
            position + vec2(0.0, 16.0),
            Align2::CENTER_CENTER,
            holds(region, graph.count(region)),
            theme::small(),
            palette::TEXT_MUTED,
        );
    }
}

fn holds(region: Region, count: usize) -> String {
    let (one, many) = match region {
        Region::ShortTerm => ("thing in mind", "things in mind"),
        Region::Episodic => ("episode", "episodes"),
        Region::LongTerm => ("fact", "facts"),
        Region::Neural | Region::Intuition => ("neuron", "neurons"),
    };

    match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    }
}

/// Names the hovered memory and, when one is selected, it and the
/// memories it is connected to.
fn node_names(ui: &Ui, view: &MemoryView, rect: Rect) {
    let scene = &view.scene;
    let graph = scene.graph();

    let mut named: Vec<usize> = Vec::new();

    if let Some(selected) = scene.selected() {
        named.push(selected);
        named.extend(
            graph
                .connections(selected)
                .into_iter()
                .map(|(other, _)| other),
        );
    }

    if let Some(hovered) = scene.hovered().filter(|hovered| !named.contains(hovered)) {
        named.push(hovered);
    }

    let painter = ui.painter_at(rect);

    for index in named {
        let Some(position) = scene.screen_position(index, rect) else {
            continue;
        };

        let emphasised = scene.selected() == Some(index) || scene.hovered() == Some(index);

        let color = if emphasised {
            palette::TEXT
        } else {
            palette::TEXT_SECONDARY
        };

        let galley = painter.layout_no_wrap(
            format::one_line(&graph.nodes[index].title, 46),
            theme::small(),
            color,
        );

        let anchor = position + vec2(12.0, -galley.size().y / 2.0);
        let background = Rect::from_min_size(anchor, galley.size()).expand2(vec2(6.0, 3.0));

        painter.rect_filled(background, CornerRadius::same(5), LABEL_FILL);
        painter.galley(anchor, galley, color);
    }
}

fn heading(ui: &Ui, view: &MemoryView, rect: Rect) {
    let painter = ui.painter_at(rect);
    let corner = rect.left_top() + vec2(MARGIN, MARGIN);

    let graph = view.scene.graph();

    painter.text(
        corner,
        Align2::LEFT_TOP,
        format!(
            "{} memory/concept nodes · {} fly neurons · {} links",
            graph.nodes.len() - graph.count(Region::Neural),
            graph.count(Region::Neural),
            graph.links.len()
        ),
        egui::FontId::proportional(14.0),
        palette::TEXT,
    );

    let placement = match &view.placement {
        Placement::Model(model) => format!("Episodes placed by meaning with {model}"),

        Placement::SharedWords => {
            "Episodes placed by the words their tasks share (no embedding model)".to_string()
        }
    };

    painter.text(
        corner + vec2(0.0, 21.0),
        Align2::LEFT_TOP,
        placement,
        theme::small(),
        palette::TEXT_MUTED,
    );

    let (neurons, synapses, runs) = view.intuition;

    let intuition = if runs == 0 {
        "Intuition has no completed training runs yet".to_string()
    } else {
        format!(
            "Intuition: {} neurons and {} synapses learned from {} runs",
            format::count(neurons as u64),
            format::count(synapses as u64),
            runs
        )
    };

    painter.text(
        corner + vec2(0.0, 38.0),
        Align2::LEFT_TOP,
        intuition,
        theme::small(),
        palette::TEXT_MUTED,
    );

    if let Some(problem) = &view.problem {
        painter.text(
            corner + vec2(0.0, 57.0),
            Align2::LEFT_TOP,
            format::one_line(problem, 90),
            theme::small(),
            palette::BAD,
        );
    }

    painter.text(
        rect.left_bottom() + vec2(MARGIN, -MARGIN),
        Align2::LEFT_BOTTOM,
        "Bright points are memories and sampled neurons. Faint dots, glow and green tracts are scenery.",
        theme::small(),
        palette::TEXT_MUTED,
    );

    painter.text(
        rect.right_bottom() - vec2(MARGIN, MARGIN),
        Align2::RIGHT_BOTTOM,
        "Drag to orbit · scroll to zoom · click a memory",
        theme::small(),
        palette::TEXT_MUTED,
    );
}

/// The card for the selected memory. Returns where it was drawn.
fn card(ui: &mut Ui, view: &mut MemoryView, index: usize, rect: Rect) -> Rect {
    let area = Rect::from_min_max(
        Pos2::new(rect.right() - MARGIN - CARD_WIDTH, rect.top() + MARGIN),
        Pos2::new(rect.right() - MARGIN, rect.bottom() - MARGIN - 28.0),
    );

    let mut go_to = None;

    let drawn = ui
        .scope_builder(UiBuilder::new().max_rect(area), |ui| {
            egui::Frame::new()
                .fill(CARD_FILL)
                .stroke(Stroke::new(1.0, palette::BORDER))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());

                    let graph = view.scene.graph();

                    about(ui, &graph.nodes[index]);

                    go_to = connections(ui, graph, index);
                })
                .response
                .rect
        })
        .inner;

    if let Some(other) = go_to {
        view.scene.select(Some(other));
    }

    drawn
}

fn about(ui: &mut Ui, node: &Node) {
    ui.label(
        RichText::new(format!("{} memory", node.region.label()))
            .font(theme::small())
            .color(palette::region(node.region)),
    );

    ui.label(RichText::new(&node.title).font(theme::voice(16.0)));

    for fact in node.facts.iter().filter(|fact| !fact.is_empty()) {
        ui.label(
            RichText::new(fact)
                .font(theme::small())
                .color(palette::TEXT_SECONDARY),
        );
    }
}

/// Lists the memories linked to `index`. Returns the one that was clicked.
fn connections(ui: &mut Ui, graph: &Graph, index: usize) -> Option<usize> {
    let connections = graph.connections(index);

    ui.add_space(4.0);
    ui.separator();

    let title = match connections.len() {
        0 => "No connected memories".to_string(),
        1 => "1 connected memory".to_string(),
        count => format!("{count} connected memories"),
    };

    ui.label(
        RichText::new(title)
            .font(theme::small())
            .color(palette::TEXT_MUTED),
    );

    let mut clicked = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;

            for (other, tie) in connections {
                if connection_row(ui, &graph.nodes[other], tie).clicked() {
                    clicked = Some(other);
                }
            }
        });

    clicked
}

fn connection_row(ui: &mut Ui, node: &Node, tie: Tie) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());

    let painter = ui.painter_at(rect);

    if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), palette::RAISED);
    }

    painter.circle_filled(
        Pos2::new(rect.left() + 10.0, rect.center().y),
        3.5,
        palette::region(node.region),
    );

    painter.text(
        Pos2::new(rect.left() + 22.0, rect.center().y),
        Align2::LEFT_CENTER,
        format::one_line(&node.title, 36),
        egui::FontId::proportional(12.5),
        palette::TEXT,
    );

    painter.text(
        Pos2::new(rect.right() - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        tie.label(),
        theme::small(),
        palette::TEXT_MUTED,
    );

    let why = match tie {
        Tie::Synapse(weight) => format!(
            "Directed neural connection; signed weight {weight:+}. Negative inhibits, positive excites."
        ),
        Tie::Similar(similarity) => format!("A similar task (similarity {similarity:.2})."),
        Tie::Consolidated => "A fact and an episode that supports it.".to_string(),
        Tie::InMind => "Held in mind for the current task.".to_string(),

        Tie::Association(strength) => {
            format!("A synapse learned from runs that passed (strength {strength:.2}).")
        }
    };

    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("{}\n\n{why} Click to travel there.", node.title))
}

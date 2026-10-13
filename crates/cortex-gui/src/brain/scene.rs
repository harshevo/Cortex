//! Where every memory is, where it is heading, and what is selected.
//!
//! The scene does not move anything itself. It records where each memory
//! set off from, where it is going and when it left; the shaders work out
//! the rest every frame. The scene only repeats that sum for the handful
//! of memories it needs to find under the pointer.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{Pos2, Rect};
use glam::Vec3;

use super::anatomy;
use super::camera::Camera;
use super::instances::{Fibre, Frame, LONG_AGO, Particle, Snapshot};
use crate::memory::Region;
use crate::memory::graph::{Graph, NodeKey, Tie};
use crate::theme::palette;

/// Must match `TRAVEL_SECONDS` in `shaders/globals.wgsl`.
const TRAVEL_SECONDS: f32 = 2.6;

/// A memory that moves less than this keeps its place, so that small
/// shifts in the layout do not set the whole brain in motion.
const SETTLED: f32 = 0.02;

/// How close, in points, the pointer must be to a memory to pick it.
const PICK_RADIUS: f32 = 14.0;

/// How quickly the short-term region lights up and dims.
const ACTIVITY_RATE: f32 = 3.0;

const OVERVIEW_DISTANCE: f32 = 5.4;
const CLOSE_UP_DISTANCE: f32 = 2.4;

/// A memory's place in the brain, now and soon.
#[derive(Debug, Clone, Copy)]
struct Body {
    from: Vec3,
    to: Vec3,
    departed_at: f32,
    created_at: f32,
}

impl Body {
    fn settled(place: Vec3) -> Self {
        Self {
            from: place,
            to: place,
            departed_at: LONG_AGO,
            created_at: LONG_AGO,
        }
    }

    /// Where the memory is at `time`. The same sum as `journey` in the
    /// shaders.
    fn position(&self, time: f32) -> Vec3 {
        let progress = ((time - self.departed_at) / TRAVEL_SECONDS).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - progress).powi(3);

        self.from.lerp(self.to, eased)
    }

    fn arrives_at(&self) -> f32 {
        self.departed_at + TRAVEL_SECONDS
    }
}

pub struct Scene {
    camera: Camera,
    graph: Graph,
    bodies: Vec<Body>,

    /// When each link appears, in step with `graph.links`.
    link_appears: Vec<f32>,

    selected: Option<usize>,
    hovered: Option<usize>,

    /// Seconds since the scene was created.
    clock: f32,

    /// How lit up short-term memory is, easing toward `thinking`.
    activity: f32,
    thinking: bool,

    /// What the GPU should hold. Rebuilt only when something changes.
    snapshot: Arc<Snapshot>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            camera: Camera::default(),
            graph: Graph::default(),
            bodies: Vec::new(),
            link_appears: Vec::new(),
            selected: None,
            hovered: None,
            clock: 0.0,
            activity: 0.0,
            thinking: false,
            snapshot: Arc::new(Snapshot {
                revision: 0,
                nodes: Vec::new(),
                links: Vec::new(),
            }),
        }
    }
}

impl Scene {
    /// Replaces the memories shown.
    ///
    /// `episode_points` places each episode by the meaning of its task.
    /// With `animate`, memories that are new are born and travel to their
    /// place; without it (the first time memory is loaded) everything is
    /// simply there.
    pub fn set_graph(&mut self, graph: Graph, episode_points: &[Vec3], animate: bool) {
        let before: HashMap<NodeKey, Body> = self
            .graph
            .nodes
            .iter()
            .zip(&self.bodies)
            .map(|(node, body)| (node.key.clone(), *body))
            .collect();

        let appeared: HashMap<(NodeKey, NodeKey), f32> = self
            .graph
            .links
            .iter()
            .zip(&self.link_appears)
            .map(|(link, appears)| (self.link_keys(link.a, link.b), *appears))
            .collect();

        let selected_key = self
            .selected
            .map(|index| self.graph.nodes[index].key.clone());

        let homes = homes(&graph, episode_points);

        self.bodies = graph
            .nodes
            .iter()
            .zip(&homes)
            .map(|(node, home)| self.body_for(node.region, before.get(&node.key), *home, animate))
            .collect();

        self.graph = graph;

        self.link_appears = self
            .graph
            .links
            .iter()
            .map(|link| {
                let known = appeared.get(&self.link_keys(link.a, link.b)).copied();

                // A link waits for both of its ends to arrive.
                let ready = self.bodies[link.a]
                    .arrives_at()
                    .max(self.bodies[link.b].arrives_at());

                known.unwrap_or(LONG_AGO).max(ready)
            })
            .collect();

        self.hovered = None;

        self.selected =
            selected_key.and_then(|key| self.graph.nodes.iter().position(|node| node.key == key));

        self.rebuild_snapshot();
    }

    fn link_keys(&self, a: usize, b: usize) -> (NodeKey, NodeKey) {
        (
            self.graph.nodes[a].key.clone(),
            self.graph.nodes[b].key.clone(),
        )
    }

    fn body_for(&self, region: Region, before: Option<&Body>, home: Vec3, animate: bool) -> Body {
        match before {
            Some(body) if body.to.distance(home) < SETTLED => *body,

            // The layout shifted: travel on from wherever it is now.
            Some(body) => Body {
                from: body.position(self.clock),
                to: home,
                departed_at: self.clock,
                created_at: body.created_at,
            },

            None if !animate => Body::settled(home),

            None => Body {
                from: birthplace(region, home),
                to: home,
                departed_at: self.clock,
                created_at: self.clock,
            },
        }
    }

    /// Moves the clock on. `thinking` is whether a run is in progress.
    pub fn advance(&mut self, seconds: f32, thinking: bool) {
        self.clock += seconds;
        self.thinking = thinking;

        let goal = if thinking { 1.0 } else { 0.0 };

        self.activity += (goal - self.activity) * (1.0 - (-seconds * ACTIVITY_RATE).exp());

        if let Some(index) = self.selected {
            self.camera.follow(self.bodies[index].position(self.clock));
        }

        self.camera.advance(seconds);
    }

    pub fn orbit(&mut self, pixels_x: f32, pixels_y: f32) {
        self.camera.orbit(pixels_x, pixels_y);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.camera.zoom(factor);
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn hovered(&self) -> Option<usize> {
        self.hovered
    }

    pub fn set_hovered(&mut self, index: Option<usize>) {
        if self.hovered != index {
            self.hovered = index;

            self.rebuild_snapshot();
        }
    }

    /// Selects a memory and glides to it, or clears the selection and
    /// pulls back to the whole brain.
    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index.filter(|index| *index < self.bodies.len());

        match self.selected {
            Some(index) => self
                .camera
                .look_at(self.bodies[index].to, CLOSE_UP_DISTANCE),
            None => self.camera.look_at(Vec3::ZERO, OVERVIEW_DISTANCE),
        }

        self.rebuild_snapshot();
    }

    /// Where a memory appears inside `rect`.
    pub fn screen_position(&self, index: usize, rect: Rect) -> Option<Pos2> {
        let position = self.bodies.get(index)?.position(self.clock);

        self.camera.lens(rect).project(position)
    }

    /// Where a point of the brain appears inside `rect`.
    pub fn project(&self, point: Vec3, rect: Rect) -> Option<Pos2> {
        self.camera.lens(rect).project(point)
    }

    /// The memory under the pointer, if any.
    pub fn pick(&self, pointer: Pos2, rect: Rect) -> Option<usize> {
        let lens = self.camera.lens(rect);

        self.bodies
            .iter()
            .enumerate()
            .filter_map(|(index, body)| {
                let position = body.position(self.clock);
                let on_screen = lens.project(position)?;

                let size = radius(self.graph.nodes[index].weight) * lens.scale_at(position);

                let distance = on_screen.distance(pointer);

                (distance <= PICK_RADIUS.max(size * 2.0)).then_some((index, distance))
            })
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(index, _)| index)
    }

    /// Everything the renderer needs to draw the scene into `rect`.
    pub fn frame(&self, rect: Rect, pixels_per_point: f32) -> Frame {
        let focus = if self.selected.is_some() { 1.0 } else { 0.0 };

        Frame {
            globals: self
                .camera
                .globals(rect, pixels_per_point, self.clock, focus, self.activity),
            snapshot: Arc::clone(&self.snapshot),
        }
    }

    fn rebuild_snapshot(&mut self) {
        self.snapshot = Arc::new(Snapshot {
            revision: self.snapshot.revision + 1,
            nodes: self.particles(),
            links: self.fibres(),
        });
    }

    fn particles(&self) -> Vec<Particle> {
        let connected: Vec<usize> = match self.selected {
            Some(index) => self
                .graph
                .connections(index)
                .into_iter()
                .map(|(other, _)| other)
                .collect(),

            None => Vec::new(),
        };

        self.graph
            .nodes
            .iter()
            .zip(&self.bodies)
            .enumerate()
            .map(|(index, (node, body))| {
                let is_selected = self.selected == Some(index);

                let mut brightness = match self.selected {
                    None => 0.8,
                    Some(_) if is_selected => 1.35,
                    Some(_) if connected.contains(&index) => 1.15,
                    Some(_) => 0.2,
                };

                if self.hovered == Some(index) {
                    brightness += 0.45;
                }

                let color = if node.troubled {
                    palette::TROUBLED
                } else {
                    palette::region(node.region)
                };

                let [red, green, blue, _] = color.to_normalized_gamma_f32();

                let emphasis = if is_selected { 1.0 } else { 0.0 };
                let spikes = if is_selected { 1.0 } else { 0.5 };

                Particle {
                    from_radius: body.from.extend(radius(node.weight)).to_array(),
                    to_departure: body.to.extend(body.departed_at).to_array(),
                    color_brightness: [red, green, blue, brightness],
                    life: [body.created_at, twinkle_phase(&node.key), emphasis, spikes],
                }
            })
            .collect()
    }

    fn fibres(&self) -> Vec<Fibre> {
        self.graph
            .links
            .iter()
            .zip(&self.link_appears)
            .map(|(link, appears)| {
                let touches_selection = self
                    .selected
                    .is_some_and(|index| link.other(index).is_some());

                let (intensity, width, flow) = match (self.selected, touches_selection) {
                    (Some(_), true) => (1.0, 1.5, 1.0),
                    (Some(_), false) => (0.03, 1.0, 0.0),
                    (None, _) => (0.2, 1.0, 0.0),
                };

                let [red, green, blue, _] = tie_color(link.tie).to_normalized_gamma_f32();

                Fibre {
                    start_width: self.bodies[link.a].to.extend(width).to_array(),
                    end_flow: self.bodies[link.b].to.extend(flow).to_array(),
                    color_intensity: [red, green, blue, intensity * link.tie.strength()],
                    timing: [*appears, 0.0, 0.0, 0.0],
                }
            })
            .collect()
    }
}

/// The home of every node in `graph`, in order.
fn homes(graph: &Graph, episode_points: &[Vec3]) -> Vec<Vec3> {
    let mut homes: Vec<Vec3> = Vec::with_capacity(graph.nodes.len());
    let mut thoughts = 0;

    for (index, node) in graph.nodes.iter().enumerate() {
        let home = match &node.key {
            // Episodes come first in the graph, in step with their points.
            NodeKey::Episode(_) => {
                anatomy::episode_home(episode_points.get(index).copied().unwrap_or(Vec3::ZERO))
            }

            NodeKey::Fact(name) => {
                // Knowledge leans toward the episodes it was drawn from,
                // all of which are already placed.
                let lean: Vec3 = graph
                    .connections(index)
                    .into_iter()
                    .filter_map(|(other, _)| homes.get(other))
                    .map(|home| *home - anatomy::HIPPOCAMPUS)
                    .sum();

                anatomy::knowledge_home(name, lean)
            }

            NodeKey::Neuron(id) => anatomy::neuron_home(*id),
            NodeKey::Concept(name) => anatomy::concept_home(name),

            NodeKey::Thought(_) => {
                thoughts += 1;

                anatomy::thought_home(thoughts - 1)
            }
        };

        homes.push(home);
    }

    homes
}

/// Where a new memory first appears. An episode forms out of short-term
/// memory; long-term knowledge is drawn out of the episodes.
fn birthplace(region: Region, home: Vec3) -> Vec3 {
    match region {
        Region::Neural => home,
        Region::ShortTerm => home,
        Region::Episodic => anatomy::PREFRONTAL,
        Region::LongTerm => anatomy::HIPPOCAMPUS,

        // The intuition, too, is drawn out of what runs went through.
        Region::Intuition => anatomy::HIPPOCAMPUS,
    }
}

fn radius(weight: f32) -> f32 {
    0.013 + 0.017 * weight
}

fn tie_color(tie: Tie) -> eframe::egui::Color32 {
    match tie {
        Tie::Synapse(weight) => {
            if weight < 0 {
                palette::TROUBLED
            } else {
                palette::region(Region::Neural)
            }
        }
        Tie::Similar(_) => palette::region(Region::Episodic),
        Tie::Consolidated => palette::TRACT,
        Tie::InMind => palette::region(Region::ShortTerm),
        Tie::Association(_) => palette::region(Region::Intuition),
    }
}

/// A fixed phase per memory, so each twinkles to its own rhythm.
fn twinkle_phase(key: &NodeKey) -> f32 {
    let seed = match key {
        NodeKey::Neuron(id) => u64::from(*id),
        NodeKey::Episode(id) => id.unsigned_abs(),
        NodeKey::Thought(text) | NodeKey::Fact(text) | NodeKey::Concept(text) => {
            text.bytes().map(u64::from).sum()
        }
    };

    (seed % 1_000) as f32 * 0.37
}

#[cfg(test)]
mod tests {
    use cortex_core::Outcome;
    use cortex_memory::Episode;
    use cortex_memory::consolidate::derive_facts;
    use eframe::egui::Vec2;

    use super::*;
    use crate::memory::Remembered;
    use crate::memory::graph::{Lasting, Thought};
    use crate::memory::reader::Placement;

    fn episode(id: i64, files: &[&str]) -> Episode {
        Episode {
            id,
            created_ms: id as u64,
            instruction: format!("task {id}"),
            outcome: Some(Outcome::Completed),
            verified: None,
            rejections: 0,
            steps: 5,
            tokens: 0,
            duration_ms: 0,
            files_changed: files.iter().map(|file| file.to_string()).collect(),
            files_read: Vec::new(),
            commands_worked: Vec::new(),
            calculations: Vec::new(),
            dead_ends: Vec::new(),
            checks_run: Vec::new(),
            checks_failed: Vec::new(),
            report: String::new(),
        }
    }

    /// Episodes 1 and 2 are alike and changed the same file, which the
    /// engine promotes to a fact; episode 3 is unrelated.
    fn remembered() -> Remembered {
        let episodes = vec![
            episode(1, &["rsi.py"]),
            episode(2, &["rsi.py"]),
            episode(3, &[]),
        ];

        Remembered {
            total_episodes: episodes.len(),
            facts: derive_facts(&episodes, 10),
            episodes,
            vectors: vec![
                vec![1.0, 0.0, 0.0],
                vec![0.9, 0.1, 0.0],
                vec![0.0, 0.0, 1.0],
            ],
            placement: Placement::SharedWords,
            intuition: Default::default(),
            lessons: Vec::new(),
        }
    }

    fn with_one_more(
        mut remembered: Remembered,
        id: i64,
        vector: [f32; 3],
        first: bool,
    ) -> Remembered {
        let position = if first { 0 } else { remembered.episodes.len() };

        remembered.episodes.insert(position, episode(id, &[]));
        remembered.vectors.insert(position, vector.to_vec());

        remembered
    }

    fn show(scene: &mut Scene, remembered: &Remembered, thoughts: &[Thought], animate: bool) {
        let lasting = Lasting::arrange(remembered);
        let graph = Graph::assemble(&lasting, thoughts);

        scene.set_graph(graph, &lasting.episode_points, animate);
    }

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))
    }

    #[test]
    fn memories_present_at_the_start_are_already_in_place() {
        let mut scene = Scene::default();

        show(&mut scene, &remembered(), &[], false);

        assert_eq!(scene.bodies.len(), 4);
        assert!(scene.bodies.iter().all(|body| body.from == body.to));
        assert!(scene.link_appears.iter().all(|appears| *appears < 0.0));
    }

    #[test]
    fn a_new_episode_forms_in_short_term_memory_and_travels_to_the_hippocampus() {
        let mut scene = Scene::default();

        show(&mut scene, &remembered(), &[], false);

        scene.advance(1.0, false);

        show(
            &mut scene,
            &with_one_more(remembered(), 4, [0.0, 1.0, 0.0], false),
            &[],
            true,
        );

        // Episodes come first, so the new one is the fourth node.
        let newborn = scene.bodies[3];

        assert_eq!(newborn.from, anatomy::PREFRONTAL);
        assert_eq!(newborn.created_at, 1.0);
        assert_eq!(newborn.position(1.0), anatomy::PREFRONTAL);

        let arrived = newborn.position(1.0 + TRAVEL_SECONDS);

        assert!(arrived.distance(newborn.to) < 1e-5);
        assert!(arrived.distance(anatomy::HIPPOCAMPUS) < 0.75);
    }

    #[test]
    fn the_snapshot_changes_only_when_the_picture_does() {
        let mut scene = Scene::default();

        show(&mut scene, &remembered(), &[], false);

        let before = scene.frame(rect(), 2.0).snapshot.revision;

        scene.advance(0.5, true);
        scene.orbit(40.0, 10.0);
        scene.set_hovered(None);

        assert_eq!(scene.frame(rect(), 2.0).snapshot.revision, before);

        scene.select(Some(0));

        assert_eq!(scene.frame(rect(), 2.0).snapshot.revision, before + 1);
    }

    #[test]
    fn selecting_a_memory_lights_its_connections() {
        let mut scene = Scene::default();

        show(&mut scene, &remembered(), &[], false);

        scene.select(Some(0));

        let frame = scene.frame(rect(), 2.0);
        let brightness = |index: usize| frame.snapshot.nodes[index].color_brightness[3];

        // Episode 1 is tied to episode 2 and to the fact they both
        // support, not to episode 3.
        assert!(brightness(1) > brightness(2));
        assert!(brightness(3) > brightness(2));
        assert_eq!(frame.snapshot.nodes[0].life[2], 1.0);
        assert_eq!(frame.globals.camera_position[3], 1.0);
    }

    #[test]
    fn the_selection_survives_new_memories() {
        let mut scene = Scene::default();

        show(&mut scene, &remembered(), &[], false);

        scene.select(Some(2));

        show(
            &mut scene,
            &with_one_more(remembered(), 9, [0.5, 0.5, 0.0], true),
            &[],
            true,
        );

        assert_eq!(scene.selected(), Some(3));
    }

    #[test]
    fn a_memory_can_be_picked_where_it_is_drawn() {
        let mut scene = Scene::default();

        let thought = Thought {
            key: "task".to_string(),
            title: "look".to_string(),
            detail: String::new(),
        };

        show(&mut scene, &remembered(), &[thought], false);

        let last = scene.bodies.len() - 1;
        let on_screen = scene.screen_position(last, rect()).unwrap();

        assert_eq!(scene.pick(on_screen, rect()), Some(last));
        assert_eq!(scene.pick(Pos2::new(-500.0, -500.0), rect()), None);
    }

    #[test]
    fn thinking_lights_up_short_term_memory() {
        let mut scene = Scene::default();

        for _ in 0..120 {
            scene.advance(1.0 / 60.0, true);
        }

        assert!(scene.frame(rect(), 2.0).globals.activity[0] > 0.9);
    }
}

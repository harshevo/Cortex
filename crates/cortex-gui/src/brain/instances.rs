//! The data handed to the shaders. Field order and sizes must match the
//! structs in `shaders/*.wgsl`.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Globals {
    pub view_projection: [[f32; 4]; 4],
    pub camera_right: [f32; 4],
    pub camera_up: [f32; 4],
    pub camera_forward: [f32; 4],
    pub camera_position: [f32; 4],
    pub viewport: [f32; 4],

    /// How lit up short-term memory is, from 0 to 1. The other three are
    /// unused.
    pub activity: [f32; 4],
}

/// One glowing point: a memory, or a dot of scenery.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Particle {
    /// Where it is travelling from, and its core radius.
    pub from_radius: [f32; 4],

    /// Where it is travelling to, and when it set off.
    pub to_departure: [f32; 4],

    pub color_brightness: [f32; 4],

    /// When it was created, twinkle phase, emphasis, kind.
    pub life: [f32; 4],
}

/// The `kind` of a particle that is scenery rather than a memory. A
/// memory's kind is the strength of its spikes, zero or more.
pub const SCENERY: f32 = -1.0;

/// A creation or departure time long enough ago that nothing is animating.
pub const LONG_AGO: f32 = -1.0e6;

/// One ribbon of light between two points.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Fibre {
    /// One end, and the width in pixels.
    pub start_width: [f32; 4],

    /// The other end, and whether light flows along it.
    pub end_flow: [f32; 4],

    pub color_intensity: [f32; 4],

    /// When it appears. The other three are unused.
    pub timing: [f32; 4],
}

/// The memories, as the GPU needs them. Sent only when `revision` differs
/// from what the renderer already holds.
pub struct Snapshot {
    pub revision: u64,
    pub nodes: Vec<Particle>,
    pub links: Vec<Fibre>,
}

/// Everything the renderer needs to draw one frame.
pub struct Frame {
    pub globals: Globals,
    pub snapshot: Arc<Snapshot>,
}

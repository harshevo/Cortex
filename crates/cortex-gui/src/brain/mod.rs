//! The memory space: memory drawn as a brain.
//!
//! Three regions hold the three kinds of memory. Short-term memory sits at
//! the front, where the prefrontal cortex is; episodes sit in the middle,
//! in the hippocampus; long-term knowledge is spread over the cortex.
//!
//! [`Scene`] decides where everything is; [`renderer`] draws it. The
//! simulation runs on the GPU: every particle's motion is a function of
//! time worked out in the shaders, so a frame costs the CPU one small
//! upload, and node data is only sent again when memory changes.
//!
//! The faint dots, the glow and the green tracts are scenery that gives
//! the brain its shape. Only the bright nodes and their links are data.

pub mod activity;
pub mod anatomy;
mod camera;
mod instances;
mod renderer;
mod scene;
mod scenery;

pub use renderer::{install, paint};
pub use scene::Scene;

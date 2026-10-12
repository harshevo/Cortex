// Shared by every brain shader: the camera, the clock, and the helpers
// that turn time into motion.

struct Globals {
    view_projection: mat4x4<f32>,

    // xyz: camera axis in world space. w: see each field.
    camera_right: vec4<f32>,    // w = tan(half the horizontal field of view)
    camera_up: vec4<f32>,       // w = tan(half the vertical field of view)
    camera_forward: vec4<f32>,  // w = seconds since the scene opened
    camera_position: vec4<f32>, // w = focus: 1 while a memory is selected

    // x, y: size of the drawing area in pixels.
    viewport: vec4<f32>,

    // x: how lit up short-term memory is, from 0 to 1.
    activity: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> globals: Globals;

// How long a memory takes to travel to a new place, in seconds. The scene
// uses the same number to know where a memory is when it is clicked.
const TRAVEL_SECONDS: f32 = 2.6;

// Corners of a quad drawn as two triangles, from the vertex index alone.
fn quad_corner(vertex_index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );

    return corners[vertex_index];
}

// From 0 to 1 over the journey that began at `departure`: fast at first,
// settling gently.
fn journey(departure: f32) -> f32 {
    let progress = clamp((globals.camera_forward.w - departure) / TRAVEL_SECONDS, 0.0, 1.0);
    let remaining = 1.0 - progress;

    return 1.0 - remaining * remaining * remaining;
}

// Brings bright additive light back into range without a hard clip.
fn soften(light: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(1.0) - exp(-light);
}

fn hash21(point: vec2<f32>) -> f32 {
    let scrambled = fract(point * vec2<f32>(123.34, 456.21));
    let mixed = scrambled + dot(scrambled, scrambled + 45.32);

    return fract(mixed.x * mixed.y);
}

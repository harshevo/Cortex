// The backdrop: the soft glow that fills the brain. Scenery only.
//
// Each lobe is a cloud whose density falls off like a bell curve. The
// light a view ray gathers passing through such a cloud has an exact
// answer, so one pass over the screen gives a true volume, with depth and
// parallax, and no stepping along the ray.

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) screen: vec2<f32>,
}

// A cloud: where it is, how far it reaches along each axis, its colour,
// and how dense it is. A negative density hollows out what is around it.
struct Cloud {
    centre: vec3<f32>,
    reach: vec3<f32>,
    color: vec3<f32>,
    density: f32,
}

// These follow the constants in `anatomy.rs`.
const CLOUD_COUNT: u32 = 9u;

const CLOUDS = array<Cloud, 9>(
    // The cortex: a wide violet shell...
    Cloud(vec3<f32>(0.0, 0.18, 0.0), vec3<f32>(1.30, 0.80, 0.95), vec3<f32>(0.62, 0.28, 1.00), 1.10),

    // ...hollowed out, so the rim is brighter than the middle.
    Cloud(vec3<f32>(0.0, 0.10, 0.0), vec3<f32>(0.90, 0.52, 0.64), vec3<f32>(0.62, 0.28, 1.00), -0.95),

    // The deep white matter, in blue.
    Cloud(vec3<f32>(0.15, 0.10, 0.0), vec3<f32>(0.85, 0.42, 0.55), vec3<f32>(0.14, 0.36, 1.00), 0.50),

    // The tracts rising from the stem, in green.
    Cloud(vec3<f32>(-0.2, -0.45, 0.0), vec3<f32>(0.80, 0.30, 0.50), vec3<f32>(0.12, 0.95, 0.34), 0.48),

    // The cerebellum.
    Cloud(vec3<f32>(-1.3, -0.85, 0.0), vec3<f32>(0.40, 0.27, 0.52), vec3<f32>(0.36, 0.34, 1.00), 0.55),

    // The temporal lobes, left and right.
    Cloud(vec3<f32>(0.35, -0.62, 0.8), vec3<f32>(0.60, 0.23, 0.27), vec3<f32>(0.80, 0.30, 0.85), 0.40),
    Cloud(vec3<f32>(0.35, -0.62, -0.8), vec3<f32>(0.60, 0.23, 0.27), vec3<f32>(0.80, 0.30, 0.85), 0.40),

    // The hippocampus, where episodes live.
    Cloud(vec3<f32>(-0.1, -0.20, 0.0), vec3<f32>(0.48, 0.24, 0.40), vec3<f32>(1.00, 0.40, 0.22), 0.42),

    // The prefrontal cortex, where short-term memory lives. Its density
    // is raised by `activity` while the agent is thinking.
    Cloud(vec3<f32>(1.35, 0.30, 0.0), vec3<f32>(0.34, 0.34, 0.40), vec3<f32>(0.22, 0.85, 1.00), 0.22),
);

const SHORT_TERM_CLOUD: u32 = 8u;

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // One triangle large enough to cover the whole drawing area.
    let x = f32(i32(vertex_index & 1u) * 4 - 1);
    let y = f32(i32(vertex_index >> 1u) * 4 - 1);

    var output: VertexOutput;

    output.position = vec4<f32>(x, y, 0.0, 1.0);
    output.screen = vec2<f32>(x, y);

    return output;
}

// The light gathered along a whole ray through one cloud.
//
// Squashing space by the cloud's reach turns it into a round bell curve.
// A straight line through a round bell curve gathers an amount that
// depends only on how close the line passes to the centre.
fn gathered(origin: vec3<f32>, ray: vec3<f32>, shape: Cloud) -> f32 {
    let start = (origin - shape.centre) / shape.reach;
    let direction = ray / shape.reach;

    let speed_squared = dot(direction, direction);
    let along = dot(start, direction);

    let miss_squared = dot(start, start) - along * along / speed_squared;

    return shape.density * exp(-miss_squared) / sqrt(speed_squared);
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let ray = normalize(
        globals.camera_forward.xyz
            + globals.camera_right.xyz * input.screen.x * globals.camera_right.w
            + globals.camera_up.xyz * input.screen.y * globals.camera_up.w,
    );

    let origin = globals.camera_position.xyz;
    let time = globals.camera_forward.w;

    var glow = vec3<f32>(0.0);

    for (var index = 0u; index < CLOUD_COUNT; index++) {
        var shape = CLOUDS[index];

        if index == SHORT_TERM_CLOUD {
            // Thinking: the region brightens and pulses.
            let pulse = 0.75 + 0.25 * sin(time * 5.0);

            shape.density += 0.8 * globals.activity.x * pulse;
        }

        glow += shape.color * gathered(origin, ray, shape);
    }

    // A selected memory should stand out, so the glow steps back.
    let presence = mix(1.0, 0.5, globals.camera_position.w);

    // The whole brain breathes, very slowly.
    let breath = 1.0 + 0.05 * sin(time * 0.7);

    let space = vec3<f32>(0.006, 0.006, 0.012);
    let light = space + max(glow, vec3<f32>(0.0)) * presence * breath;

    let vignette = 1.0 - 0.16 * dot(input.screen, input.screen);

    // A little noise hides the banding that smooth dark gradients show.
    let grain = (hash21(input.position.xy) - 0.5) / 255.0;

    return vec4<f32>(soften(light * 1.5) * vignette + grain, 1.0);
}

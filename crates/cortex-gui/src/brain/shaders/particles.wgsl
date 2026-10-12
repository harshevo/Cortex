// Particles: one camera-facing quad each, moved and lit on the GPU.
//
// Memories and the dots of scenery share this shader. A memory travels to
// its place, flashes when it is created, and wears a ring while selected.
// Scenery drifts gently on the spot. All of that motion is worked out here
// from the clock, so the CPU sends each particle once, not every frame.

struct Particle {
    // xyz: where it is travelling from. w: radius of the bright core.
    @location(0) from_radius: vec4<f32>,

    // xyz: where it is travelling to. w: when it set off.
    @location(1) to_departure: vec4<f32>,

    // rgb: colour. a: brightness.
    @location(2) color_brightness: vec4<f32>,

    // x: when it was created.
    // y: twinkle phase.
    // z: emphasis, 1 for the selected memory.
    // w: strength of the spikes; negative marks scenery.
    @location(3) life: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,

    // Position on the quad, measured in core radii.
    @location(0) local: vec2<f32>,

    @location(1) color_brightness: vec4<f32>,

    // x: seconds since creation. y: phase. z: emphasis. w: kind.
    @location(2) life: vec4<f32>,

    // How far the quad reaches, in core radii.
    @location(3) reach: f32,
}

// Smallest core radius, in pixels, a particle is drawn with. Anything
// smaller would flicker as it crossed pixel boundaries.
const MIN_CORE_PIXELS: f32 = 0.9;

// How long a new memory's flash and shockwave last, in seconds.
const BIRTH_SECONDS: f32 = 3.5;

// How far scenery wanders from its place, in world units.
const DRIFT: f32 = 0.014;

fn birth_progress(age: f32) -> f32 {
    return clamp(age / BIRTH_SECONDS, 0.0, 1.0);
}

// A slow wander around the particle's own spot, different for each one.
fn drift(phase: f32) -> vec3<f32> {
    let time = globals.camera_forward.w;

    return vec3<f32>(
        sin(time * 0.31 + phase),
        sin(time * 0.27 + phase * 1.7),
        sin(time * 0.23 + phase * 2.3),
    ) * DRIFT;
}

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32, particle: Particle) -> VertexOutput {
    let corner = quad_corner(vertex_index);
    let time = globals.camera_forward.w;

    let is_scenery = particle.life.w < 0.0;
    let age = time - particle.life.x;

    var world = mix(
        particle.from_radius.xyz,
        particle.to_departure.xyz,
        journey(particle.to_departure.w),
    );

    if is_scenery {
        world += drift(particle.life.y);
    }

    let centre = globals.view_projection * vec4<f32>(world, 1.0);

    var radius = particle.from_radius.w;
    var brightness = particle.color_brightness.a;

    // Keep distant particles at least a pixel wide, and dim them by the
    // area they gained so they do not look brighter than they are.
    let pixels_per_unit = globals.viewport.y / (2.0 * globals.camera_up.w * max(centre.w, 1e-3));
    let core_pixels = radius * pixels_per_unit;
    let growth = max(1.0, MIN_CORE_PIXELS / max(core_pixels, 1e-4));

    radius *= growth;
    brightness /= min(growth * growth, 24.0);

    // The quad is no wider than its light: scenery needs only its halo, a
    // newborn needs room for its shockwave, a selected memory for its ring.
    var reach = 5.0;

    if !is_scenery {
        let newborn = 1.0 - birth_progress(age);

        reach = 8.0 + 14.0 * newborn + 3.0 * particle.life.z;
    }

    let offset = (globals.camera_right.xyz * corner.x + globals.camera_up.xyz * corner.y)
        * radius
        * reach;

    var output: VertexOutput;

    output.position = globals.view_projection * vec4<f32>(world + offset, 1.0);
    output.local = corner * reach;
    output.color_brightness = vec4<f32>(particle.color_brightness.rgb, brightness);
    output.life = vec4<f32>(age, particle.life.yzw);
    output.reach = reach;

    return output;
}

// A thin band of light at `radius`, `width` core radii across.
fn ring(distance: f32, radius: f32, width: f32) -> f32 {
    let offset = (distance - radius) / width;

    return exp(-offset * offset);
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let time = globals.camera_forward.w;
    let focus = globals.camera_position.w;

    let distance = length(input.local);
    let color = input.color_brightness.rgb;

    let age = input.life.x;
    let phase = input.life.y;
    let emphasis = input.life.z;
    let is_scenery = input.life.w < 0.0;
    let spike_strength = max(input.life.w, 0.0);

    var brightness = input.color_brightness.a;

    if is_scenery {
        brightness *= mix(1.0, 0.4, focus);
    }

    // Everything fades to nothing before the edge of the quad.
    let edge = smoothstep(input.reach, input.reach * 0.5, distance);

    let core = exp(-distance * distance * 1.5);
    let halo = 0.16 / (1.0 + distance * distance * 0.55);

    // Diffraction spikes: thin along one axis, long along the other.
    let across = abs(input.local);
    let spikes = spike_strength
        * 0.5
        * (exp(-across.x * 5.0 - across.y * 0.42) + exp(-across.y * 5.0 - across.x * 0.42));

    let twinkle = 1.0 + 0.16 * sin(time * (1.3 + fract(phase) * 1.9) + phase);

    let glow = (core + halo + spikes) * brightness * twinkle;

    // The centre burns white whatever the colour.
    var light = color * glow + vec3<f32>(1.0) * core * core * brightness * 0.7;

    if !is_scenery {
        // Birth: a flash at the centre and a shockwave rolling outward.
        let progress = birth_progress(age);
        let newborn = 1.0 - progress;

        let flash = newborn * newborn * 2.4 * exp(-distance * distance * 0.22);
        let shockwave = ring(distance, 2.0 + progress * 18.0, 0.9 + progress * 1.6) * newborn;

        light += mix(color, vec3<f32>(1.0), 0.6) * (flash + shockwave * 0.9);

        // Selection: a steady ring that breathes slowly.
        let breath = 0.75 + 0.25 * sin(time * 2.4);

        light += mix(color, vec3<f32>(1.0), 0.45)
            * ring(distance, 4.6, 0.32)
            * emphasis
            * breath
            * 0.85;
    }

    return vec4<f32>(soften(light * 1.35) * edge, 0.0);
}

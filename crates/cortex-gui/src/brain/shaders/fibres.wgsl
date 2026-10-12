// Fibres: ribbons of light between two points, a fixed number of pixels
// wide so they stay thin and crisp at any zoom. Links between memories and
// the scenery's tracts share this shader.

struct Fibre {
    // xyz: one end in world space. w: width in pixels.
    @location(0) start_width: vec4<f32>,

    // xyz: the other end. w: 1 when light should flow along the fibre.
    @location(1) end_flow: vec4<f32>,

    // rgb: colour. a: intensity.
    @location(2) color_intensity: vec4<f32>,

    // x: when the fibre appears.
    @location(3) timing: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,

    // x: 0 at the start, 1 at the end. y: -1 and 1 at the two edges.
    @location(0) along_across: vec2<f32>,

    @location(1) color_intensity: vec4<f32>,
    @location(2) flow: f32,
}

// How long a fibre takes to fade in, in seconds.
const FADE_SECONDS: f32 = 1.2;

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32, fibre: Fibre) -> VertexOutput {
    let corner = quad_corner(vertex_index);
    let along = corner.x * 0.5 + 0.5;

    let start = globals.view_projection * vec4<f32>(fibre.start_width.xyz, 1.0);
    let end = globals.view_projection * vec4<f32>(fibre.end_flow.xyz, 1.0);

    let shown = clamp((globals.camera_forward.w - fibre.timing.x) / FADE_SECONDS, 0.0, 1.0);

    var output: VertexOutput;

    output.along_across = vec2<f32>(along, corner.y);
    output.color_intensity = vec4<f32>(fibre.color_intensity.rgb, fibre.color_intensity.a * shown);
    output.flow = fibre.end_flow.w;

    // A fibre that has not appeared yet, or that reaches behind the
    // camera, is not drawn at all.
    if shown <= 0.0 || start.w <= 0.01 || end.w <= 0.01 {
        output.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);

        return output;
    }

    let start_pixels = start.xy / start.w * globals.viewport.xy;
    let end_pixels = end.xy / end.w * globals.viewport.xy;

    let span = end_pixels - start_pixels;
    let direction = span / max(length(span), 1e-4);
    let normal = vec2<f32>(-direction.y, direction.x);

    let centre = mix(start, end, along);

    // The ribbon is drawn wider than the visible line to leave room for
    // its soft edges.
    let half_width = fibre.start_width.w * 2.5;
    let offset = normal * corner.y * half_width / globals.viewport.xy * 2.0;

    output.position = vec4<f32>(centre.xy + offset * centre.w, centre.zw);

    return output;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let time = globals.camera_forward.w;

    let along = input.along_across.x;
    let across = input.along_across.y;

    let strand = exp(-across * across * 7.0);

    // Pulses of light that travel from one end to the other.
    let travel = fract(along * 2.0 - time * 0.45);
    let pulse = exp(-pow((travel - 0.5) * 5.0, 2.0)) * input.flow;

    let color = input.color_intensity.rgb;
    let intensity = input.color_intensity.a * strand;

    let light = color * intensity * (0.6 + 1.6 * pulse)
        + vec3<f32>(1.0) * intensity * pulse * 0.35;

    return vec4<f32>(soften(light), 0.0);
}

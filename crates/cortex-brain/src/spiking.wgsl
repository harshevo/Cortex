struct Node {
    voltage: f32,
    refractory: u32,
    fired: u32,
    current: atomic<u32>,
    slot: u32,
    gain: f32,
    drive: f32,
    feature: u32,
    count: u32,
    input: u32,
}
struct Config { neurons: u32, tick: u32, samples: u32, steps: u32, central_gain: f32, inhibition: f32, pad0: u32, pad1: u32 }
struct Observation { voltage: f32, spike: u32 }
@group(0) @binding(0) var<storage, read_write> nodes: array<Node>;
@group(0) @binding(1) var<storage, read> offsets: array<u32>;
@group(0) @binding(2) var<storage, read> destinations: array<u32>;
@group(0) @binding(3) var<storage, read> strengths: array<u32>;
@group(0) @binding(4) var<storage, read> features: array<f32>;
@group(0) @binding(5) var<storage, read_write> spikes: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read> samples: array<u32>;
@group(0) @binding(7) var<storage, read_write> observations: array<Observation>;
@group(0) @binding(8) var<uniform> config: Config;

fn add_current(destination: u32, arriving: f32) {
    var old = atomicLoad(&nodes[destination].current);
    loop {
        let next = bitcast<u32>(bitcast<f32>(old) + arriving);
        let result = atomicCompareExchangeWeak(&nodes[destination].current, old, next);
        if result.exchanged { break; }
        old = result.old_value;
    }
}

@compute @workgroup_size(128)
fn reset(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let id = invocation.x;
    if id >= config.neurons { return; }
    nodes[id].voltage = 0.0;
    nodes[id].refractory = 0u;
    nodes[id].fired = 0u;
    nodes[id].count = 0u;
    atomicStore(&nodes[id].current, 0u);
}

@compute @workgroup_size(128)
fn deliver(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let id = invocation.x;
    if id >= config.neurons || nodes[id].fired == 0u { return; }
    for (var edge = offsets[id]; edge < offsets[id + 1u]; edge += 1u) {
        let destination = destinations[edge];
        let packed = (strengths[edge / 2u] >> (16u * (edge % 2u))) & 65535u;
        let strength = f32(i32(packed << 16u) >> 16);
        let gain = select(config.central_gain, nodes[destination].gain, nodes[id].input != 0u);
        add_current(destination, gain * strength);
    }
}

@compute @workgroup_size(128)
fn update(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let id = invocation.x;
    if id >= config.neurons { return; }
    let arriving = bitcast<f32>(atomicExchange(&nodes[id].current, 0u));
    nodes[id].fired = 0u;
    if nodes[id].refractory > 0u {
        nodes[id].refractory -= 1u;
        return;
    }
    var driven = arriving;
    if nodes[id].input != 0u { driven += features[nodes[id].feature] * nodes[id].drive; }
    var inhibition = 0.0;
    if config.tick > 0u { inhibition = config.inhibition * f32(atomicLoad(&spikes[config.tick - 1u])) / f32(config.neurons); }
    let voltage = nodes[id].voltage * 0.8 + driven - inhibition;
    if voltage >= 1.0 {
        nodes[id].voltage = 0.0;
        nodes[id].refractory = 2u;
        nodes[id].fired = 1u;
        nodes[id].count += 1u;
        atomicAdd(&spikes[config.tick], 1u);
    } else {
        nodes[id].voltage = max(voltage, -1.0);
    }
}

@compute @workgroup_size(128)
fn capture(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if index >= config.samples { return; }
    let id = samples[index];
    observations[config.tick * config.samples + index].voltage = nodes[id].voltage;
    observations[config.tick * config.samples + index].spike = nodes[id].fired;
}

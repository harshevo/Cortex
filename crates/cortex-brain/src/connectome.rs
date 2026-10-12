//! The wiring of the network: who connects to whom, and how strongly.
//!
//! Neurons are numbered so that the three populations are contiguous:
//!
//! ```text
//! [ input neurons | Kenyon cells | central neurons ............ ]
//! ```
//!
//! - **Input neurons** carry the situation into the brain, as projection
//!   neurons carry odours.
//! - **Kenyon cells** each listen to a handful of input neurons, which
//!   spreads similar situations onto sparse, distinct patterns. Learning
//!   happens on their outputs.
//! - **Central neurons** are the rest of the brain: a large recurrent
//!   network, a fifth of it inhibitory, that also feeds the Kenyon cells.
//!
//! The wiring is stored as one row of outgoing synapses per neuron
//! (compressed sparse rows): 6 bytes per synapse, about 300 MB for 50
//! million generated synapses. The measured fly has 15 million connected
//! pairs, each row carrying its synapse count, and takes about 90 MB.

use std::fs;
use std::io;
use std::path::Path;

/// Synapse counts between a connected pair in the fly are small integers;
/// generated strengths are drawn from this range.
const MAX_STRENGTH: u32 = 4;

/// How much stronger an inhibitory synapse is than an excitatory one. With
/// one neuron in five inhibitory, a factor of four balances the two.
const INHIBITORY_GAIN: i16 = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub neurons: usize,
    pub inputs: usize,
    pub kenyon: usize,
}

impl Layout {
    pub fn kenyon_range(&self) -> std::ops::Range<usize> {
        self.inputs..self.inputs + self.kenyon
    }

    pub fn central_start(&self) -> usize {
        self.inputs + self.kenyon
    }

    pub fn central(&self) -> usize {
        self.neurons - self.central_start()
    }
}

pub struct Connectome {
    pub layout: Layout,

    /// `offsets[n]..offsets[n + 1]` are the synapses leaving neuron `n`.
    offsets: Vec<u32>,
    targets: Vec<u32>,

    /// Signed strength: positive excites, negative inhibits.
    strengths: Vec<i16>,
}

/// What the generator needs to know.
pub struct Wiring {
    pub layout: Layout,

    /// Synapses leaving each neuron.
    pub out_degree: usize,

    /// Input neurons each Kenyon cell listens to, on average. About seven
    /// in the fly.
    pub inputs_per_kenyon: usize,

    /// Share of central neurons that inhibit.
    pub inhibitory_share: f32,

    pub seed: u64,
}

impl Connectome {
    /// Generates wiring with the given statistics. The same seed always
    /// gives the same brain.
    pub fn generate(wiring: &Wiring) -> Self {
        let layout = wiring.layout.clone();

        let mut random = Random::new(wiring.seed);

        let total = layout.neurons * wiring.out_degree;

        let mut offsets = Vec::with_capacity(layout.neurons + 1);
        let mut targets = Vec::with_capacity(total);
        let mut strengths = Vec::with_capacity(total);

        let central_start = layout.central_start() as u32;
        let central = layout.central() as u32;
        let beyond_inputs = (layout.neurons - layout.inputs) as u32;

        // Each input neuron reaches enough Kenyon cells that a Kenyon cell
        // hears from `inputs_per_kenyon` of them on average.
        let kenyon_per_input =
            (layout.kenyon * wiring.inputs_per_kenyon).div_ceil(layout.inputs.max(1));

        let inhibitory_cutoff = (wiring.inhibitory_share * u32::MAX as f32) as u32;

        for neuron in 0..layout.neurons {
            offsets.push(targets.len() as u32);

            let is_input = neuron < layout.inputs;
            let is_central = neuron >= layout.central_start();

            // A neuron either excites or inhibits everything it reaches
            // (Dale's principle). Only central neurons inhibit.
            let sign: i16 = if is_central && random.draw() < inhibitory_cutoff {
                -INHIBITORY_GAIN
            } else {
                1
            };

            let mut to_kenyon = if is_input { kenyon_per_input } else { 0 };

            for _ in 0..wiring.out_degree {
                let target = if to_kenyon > 0 {
                    to_kenyon -= 1;

                    layout.inputs as u32 + random.below(layout.kenyon as u32)
                } else if is_input {
                    central_start + random.below(central)
                } else {
                    // Kenyon cells and central neurons reach anything but
                    // the inputs, so the central brain's state also
                    // shapes what the Kenyon cells do.
                    layout.inputs as u32 + random.below(beyond_inputs)
                };

                targets.push(target);
                strengths.push(sign * (1 + random.below(MAX_STRENGTH)) as i16);
            }
        }

        offsets.push(targets.len() as u32);

        Self {
            layout,
            offsets,
            targets,
            strengths,
        }
    }

    /// Loads measured wiring from a text file with one connection per
    /// line: `source,target,strength`. The strength is the number of
    /// synapses between the pair, negative when the source inhibits.
    /// Lines that do not parse (a header) are skipped. The number of
    /// neurons is one more than the largest number in the file.
    ///
    /// Measured neurons are not sorted into populations, so the layout
    /// has no input or Kenyon range; [`crate::Brain::from_file`] chooses
    /// which neurons to drive and which to read.
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(path)?;

        let mut edges: Vec<(u32, u32, i16)> = Vec::new();
        let mut neurons = 0_usize;

        for line in text.lines() {
            let mut fields = line.split(',').map(str::trim);

            let parsed = (
                fields.next().and_then(|field| field.parse::<u32>().ok()),
                fields.next().and_then(|field| field.parse::<u32>().ok()),
                fields.next().and_then(|field| field.parse::<i32>().ok()),
            );

            let (Some(source), Some(target), Some(strength)) = parsed else {
                continue;
            };

            neurons = neurons.max(source.max(target) as usize + 1);

            // The largest count in the fly is in the low thousands.
            let strength = strength.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;

            edges.push((source, target, strength));
        }

        if edges.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the file has no `source,target,strength` lines",
            ));
        }

        let layout = Layout {
            neurons,
            inputs: 0,
            kenyon: 0,
        };

        let mut rows: Vec<Vec<(u32, i16)>> = vec![Vec::new(); neurons];

        for (source, target, strength) in edges {
            rows[source as usize].push((target, strength));
        }

        let mut offsets = Vec::with_capacity(layout.neurons + 1);
        let mut targets = Vec::new();
        let mut strengths = Vec::new();

        for row in rows {
            offsets.push(targets.len() as u32);

            for (target, strength) in row {
                targets.push(target);
                strengths.push(strength);
            }
        }

        offsets.push(targets.len() as u32);

        Ok(Self {
            layout,
            offsets,
            targets,
            strengths,
        })
    }

    pub fn synapses(&self) -> usize {
        self.targets.len()
    }

    /// Bytes held by the wiring.
    pub fn memory_bytes(&self) -> usize {
        self.offsets.len() * 4 + self.targets.len() * 4 + self.strengths.len() * 2
    }

    /// The targets and strengths of the synapses leaving `neuron`.
    pub fn outgoing(&self, neuron: usize) -> (&[u32], &[i16]) {
        let range = self.offsets[neuron] as usize..self.offsets[neuron + 1] as usize;

        (&self.targets[range.clone()], &self.strengths[range])
    }
}

/// A small fast random number generator (xorshift). Wiring a brain needs
/// a hundred million draws and no cryptographic quality.
pub struct Random {
    state: u64,
}

impl Random {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed.max(1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
        }
    }

    pub fn draw(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;

        (self.state >> 32) as u32
    }

    /// A number from 0 up to but not including `bound`.
    pub fn below(&mut self, bound: u32) -> u32 {
        ((u64::from(self.draw()) * u64::from(bound.max(1))) >> 32) as u32
    }

    /// A number from 0 up to but not including 1.
    pub fn unit(&mut self) -> f32 {
        self.draw() as f32 / (u32::MAX as f32 + 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Wiring {
        Wiring {
            layout: Layout {
                neurons: 1_000,
                inputs: 100,
                kenyon: 200,
            },
            out_degree: 20,
            inputs_per_kenyon: 7,
            inhibitory_share: 0.2,
            seed: 7,
        }
    }

    #[test]
    fn generated_wiring_has_the_requested_shape() {
        let connectome = Connectome::generate(&small());

        assert_eq!(connectome.synapses(), 20_000);

        // Input neurons reach Kenyon cells and central neurons only.
        let (targets, strengths) = connectome.outgoing(0);

        assert_eq!(targets.len(), 20);
        assert!(targets.iter().all(|target| *target >= 100));
        assert!(targets.iter().any(|target| (100..300).contains(target)));
        assert!(strengths.iter().all(|strength| *strength > 0));

        // A neuron's synapses all have the same sign.
        for neuron in 300..1_000 {
            let (_, strengths) = connectome.outgoing(neuron);

            assert!(
                strengths.iter().all(|strength| *strength > 0)
                    || strengths.iter().all(|strength| *strength < 0)
            );
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_brain() {
        let first = Connectome::generate(&small());
        let second = Connectome::generate(&small());

        assert_eq!(first.outgoing(500), second.outgoing(500));
    }

    #[test]
    fn measured_wiring_is_read_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synapses.csv");

        fs::write(&path, "source,target,strength\n0,2,3\n0,1,-5\n2,0,1\n").unwrap();

        let connectome = Connectome::from_file(&path).unwrap();

        assert_eq!(connectome.layout.neurons, 3);
        assert_eq!(connectome.synapses(), 3);
        assert_eq!(connectome.outgoing(0), (&[2, 1][..], &[3, -5][..]));
        assert_eq!(connectome.outgoing(1).0.len(), 0);

        fs::write(&path, "no synapses here\n").unwrap();

        assert!(Connectome::from_file(&path).is_err());
    }
}

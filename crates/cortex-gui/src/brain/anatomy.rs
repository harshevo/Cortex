//! The shape of the brain and where each kind of memory lives in it.
//!
//! The brain faces +X, with +Y up. Sizes are in world units; the whole
//! brain is about four units long.

use glam::Vec3;

use crate::memory::Region;

pub const CEREBRUM_CENTRE: Vec3 = Vec3::new(0.0, 0.15, 0.0);
pub const CEREBRUM_RADII: Vec3 = Vec3::new(1.9, 1.2, 1.4);

pub const CEREBELLUM_CENTRE: Vec3 = Vec3::new(-1.3, -0.85, 0.0);
pub const CEREBELLUM_RADII: Vec3 = Vec3::new(0.62, 0.42, 0.8);

/// The temporal lobes: one on each side, low and toward the front. This
/// is the left one; the right mirrors it across Z.
pub const TEMPORAL_CENTRE: Vec3 = Vec3::new(0.35, -0.62, 0.8);
pub const TEMPORAL_RADII: Vec3 = Vec3::new(0.95, 0.36, 0.42);

/// Where the brain stem leaves the brain: the root of the tracts.
pub const STEM: Vec3 = Vec3::new(-0.45, -1.05, 0.0);

/// Short-term memory: the prefrontal cortex, behind the forehead.
pub const PREFRONTAL: Vec3 = Vec3::new(1.35, 0.3, 0.0);

/// Episodic memory: the hippocampus, deep in the middle.
pub const HIPPOCAMPUS: Vec3 = Vec3::new(-0.1, -0.2, 0.0);

/// The cortex below this height (as a share of its radius) is the brain's
/// underside, where nothing is placed.
const UNDERSIDE: f32 = -0.35;

/// Where a region's name is written.
pub fn label_anchor(region: Region) -> Vec3 {
    match region {
        Region::ShortTerm => PREFRONTAL + Vec3::new(0.35, 0.75, 0.0),
        Region::Episodic => HIPPOCAMPUS + Vec3::new(0.0, -0.75, 0.0),
        Region::LongTerm => CEREBRUM_CENTRE + Vec3::new(-0.6, 1.5, 0.0),
        Region::Neural => Vec3::new(-1.3, 0.8, 0.0),
        Region::Intuition => CEREBELLUM_CENTRE + Vec3::new(-0.5, -0.7, 0.0),
    }
}

/// The point on the surface of an ellipsoid, centred on the origin, in
/// `direction` from its centre.
pub fn on_ellipsoid(radii: Vec3, direction: Vec3) -> Vec3 {
    let direction = direction.normalize_or(Vec3::Y);

    direction / (direction / radii).length()
}

/// Home of the `position`th thought: a sunflower spiral on the forehead
/// side of the prefrontal cortex, the newest thoughts outermost.
pub fn thought_home(position: usize) -> Vec3 {
    const GOLDEN_ANGLE: f32 = 2.399_963;

    let angle = position as f32 * GOLDEN_ANGLE;
    let radius = 0.085 * (position as f32 + 0.6).sqrt();

    PREFRONTAL + Vec3::new(-0.35 * radius, radius * angle.sin(), radius * angle.cos())
}

/// Home of an episode. `point` is where the meaning of its task puts it,
/// in a cloud with a typical radius of one; the cloud is fitted into the
/// hippocampus and bent into its curve.
pub fn episode_home(point: Vec3) -> Vec3 {
    // Far outliers are pulled in so the region keeps its shape.
    let point = point.clamp_length_max(1.9);

    let along = point.x * 0.46;
    let bend = -0.36 * along * along;

    HIPPOCAMPUS + Vec3::new(along, point.z * 0.24 + bend + 0.1, point.y * 0.46)
}

/// Home of long-term knowledge: on the cortex, on the side of the brain
/// its episodes lean toward, spread out by its name.
pub fn knowledge_home(name: &str, episodes_lean: Vec3) -> Vec3 {
    let scatter = unit_from_name(name);

    let mut direction = (episodes_lean.normalize_or_zero() * 0.55 + scatter).normalize_or(Vec3::Y);

    direction.y = direction.y.max(UNDERSIDE);

    // Just under the surface, among the cortex's own dots.
    CEREBRUM_CENTRE + on_ellipsoid(CEREBRUM_RADII, direction) * 0.93
}

/// Home of one of the intuition's neurons: inside the cerebellum, where
/// what has been practised until it needs no thought is kept. The place
/// depends only on the concept's name.
pub fn concept_home(name: &str) -> Vec3 {
    let hash = hash_name(name);

    let part = |shift: u32| ((hash >> shift) & 0x3ff) as f32 / 511.5 - 1.0;

    let direction = Vec3::new(part(0), part(10), part(20)).normalize_or(Vec3::Y);

    // Spread through the depth, not only over the surface.
    let depth = 0.35 + 0.5 * ((hash >> 7) & 0xff) as f32 / 255.0;

    CEREBELLUM_CENTRE + direction * CEREBELLUM_RADII * depth
}

/// A direction that depends only on `name`, so knowledge keeps its place
/// from one launch to the next.
fn unit_from_name(name: &str) -> Vec3 {
    let hash = hash_name(name);

    let part = |shift: u32| ((hash >> shift) & 0x3ff) as f32 / 511.5 - 1.0;

    Vec3::new(part(0), part(10) * 0.5 + 0.45, part(20)).normalize_or(Vec3::Y)
}

fn hash_name(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;

    for byte in name.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }

    hash
}

pub fn neuron_home(id: u32) -> Vec3 {
    let angle = id as f32 * 2.399_963;
    let radius = 0.2 + (id % 19) as f32 * 0.025;
    Vec3::new(
        -1.0 + (id % 7) as f32 * 0.05,
        0.35 + radius * angle.sin(),
        radius * angle.cos(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside_cerebrum(point: Vec3) -> bool {
        ((point - CEREBRUM_CENTRE) / CEREBRUM_RADII).length() <= 1.0
    }

    #[test]
    fn every_kind_of_memory_is_inside_the_brain() {
        for position in 0..36 {
            assert!(
                inside_cerebrum(thought_home(position)),
                "thought {position}"
            );
        }

        for point in [Vec3::ZERO, Vec3::X * 3.0, Vec3::new(-1.0, 1.0, -1.0)] {
            assert!(inside_cerebrum(episode_home(point)));
        }

        for name in ["a.py", "journal/importer.py", "risk/sizing.py"] {
            assert!(inside_cerebrum(knowledge_home(name, Vec3::X)));
        }
    }

    #[test]
    fn the_intuition_lives_in_the_cerebellum() {
        for name in ["word:calendar", "file:src/calendar.ts", "command:npm test"] {
            let home = concept_home(name);

            assert!(
                ((home - CEREBELLUM_CENTRE) / CEREBELLUM_RADII).length() <= 1.0,
                "{name}"
            );
            assert_eq!(home, concept_home(name));
        }

        assert_ne!(concept_home("word:calendar"), concept_home("word:week"));
    }

    #[test]
    fn the_regions_are_apart() {
        let thought = thought_home(3);
        let episode = episode_home(Vec3::new(0.5, 0.2, -0.4));
        let knowledge = knowledge_home("a.py", Vec3::ZERO);

        assert!(thought.distance(HIPPOCAMPUS) > 0.9);
        assert!(episode.distance(HIPPOCAMPUS) < 0.75);
        assert!(knowledge.distance(HIPPOCAMPUS) > 0.9);
    }

    #[test]
    fn knowledge_keeps_its_place() {
        assert_eq!(
            knowledge_home("a.py", Vec3::X),
            knowledge_home("a.py", Vec3::X)
        );
        assert_ne!(
            knowledge_home("a.py", Vec3::X),
            knowledge_home("b.py", Vec3::X)
        );
    }
}

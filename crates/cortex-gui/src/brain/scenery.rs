//! What gives the brain its shape: a sparse shell of dots over the cortex
//! and the cerebellum, and faint tracts fanning up from the brain stem.
//! None of it is data, and none of it changes after it is made.

use std::f32::consts::TAU;

use glam::Vec3;

use super::anatomy::{
    CEREBELLUM_CENTRE, CEREBELLUM_RADII, CEREBRUM_CENTRE, CEREBRUM_RADII, STEM, TEMPORAL_CENTRE,
    TEMPORAL_RADII, on_ellipsoid,
};
use super::instances::{Fibre, LONG_AGO, Particle, SCENERY};

const CEREBRUM_DOTS: usize = 1_050;
const CEREBELLUM_DOTS: usize = 190;
const TEMPORAL_DOTS: usize = 170;

/// Below this height the cerebrum is cut flat, as a real brain's base is.
const BASE: f32 = -0.52;

/// Half the width of the gap between the two hemispheres.
const FISSURE: f32 = 0.05;

const TRACTS: usize = 18;
const SEGMENTS_PER_TRACT: usize = 9;

/// A small repeatable random number generator, so the brain looks the
/// same every time the app opens.
struct Random(u64);

impl Random {
    /// Uniform in `0..1`.
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);

        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }

    /// Uniform over the directions of a sphere.
    fn direction(&mut self) -> Vec3 {
        let height = self.unit() * 2.0 - 1.0;
        let angle = self.unit() * TAU;
        let ring = (1.0 - height * height).sqrt();

        Vec3::new(ring * angle.cos(), height, ring * angle.sin())
    }
}

pub fn dots() -> Vec<Particle> {
    let mut random = Random(0xB7A1_5EED);
    let mut dots = Vec::with_capacity(CEREBRUM_DOTS + CEREBELLUM_DOTS + TEMPORAL_DOTS);

    while dots.len() < CEREBRUM_DOTS {
        let direction = random.direction();

        // The cortex is folded, so its dots gather in patches.
        let in_a_fold = patch(direction) > 0.42;
        let keep = if in_a_fold { 1.0 } else { 0.16 };

        if random.unit() > keep {
            continue;
        }

        let depth = 1.0 - 0.2 * random.unit().powi(2);
        let surface = on_ellipsoid(CEREBRUM_RADII, direction) * depth;

        if surface.z.abs() < FISSURE || surface.y < BASE {
            continue;
        }

        let tint = random.unit();
        let color = Vec3::new(0.70 + 0.30 * tint, 0.46 + 0.12 * tint, 1.0 - 0.14 * tint);

        dots.push(dot(&mut random, CEREBRUM_CENTRE + surface, color));
    }

    while dots.len() < CEREBRUM_DOTS + CEREBELLUM_DOTS {
        let depth = 1.0 - 0.3 * random.unit();
        let surface = on_ellipsoid(CEREBELLUM_RADII, random.direction()) * depth;

        let color = Vec3::new(0.55, 0.50, 1.0);

        dots.push(dot(&mut random, CEREBELLUM_CENTRE + surface, color));
    }

    while dots.len() < CEREBRUM_DOTS + CEREBELLUM_DOTS + TEMPORAL_DOTS {
        let depth = 1.0 - 0.25 * random.unit();
        let surface = on_ellipsoid(TEMPORAL_RADII, random.direction()) * depth;

        // Half on the left, half mirrored to the right.
        let side = if dots.len() % 2 == 0 { 1.0 } else { -1.0 };
        let centre = TEMPORAL_CENTRE * Vec3::new(1.0, 1.0, side);

        let color = Vec3::new(0.86, 0.50, 0.92);

        dots.push(dot(&mut random, centre + surface, color));
    }

    dots
}

/// A number from 0 to 1 that is the same for nearby directions, so that
/// accepting only high values leaves patches.
fn patch(direction: Vec3) -> f32 {
    let cell = (direction * 3.6).floor();

    let mixed = (cell.x * 127.1 + cell.y * 311.7 + cell.z * 74.7).sin() * 43_758.547;

    mixed - mixed.floor()
}

fn dot(random: &mut Random, position: Vec3, color: Vec3) -> Particle {
    // Most dots are tiny; a few are noticeably larger.
    let radius = 0.006 + 0.010 * random.unit().powi(3);
    let brightness = 0.22 + 0.4 * random.unit();

    Particle {
        from_radius: position.extend(radius).to_array(),
        to_departure: position.extend(LONG_AGO).to_array(),
        color_brightness: color.extend(brightness).to_array(),
        life: [LONG_AGO, random.unit() * 100.0, 0.0, SCENERY],
    }
}

/// Curved bundles from the brain stem up to the cortex, each a chain of
/// short straight fibres.
pub fn tracts() -> Vec<Fibre> {
    let mut random = Random(0x7EAC_7500);
    let mut fibres = Vec::with_capacity(TRACTS * SEGMENTS_PER_TRACT);

    for _ in 0..TRACTS {
        let mut direction = random.direction();

        direction.y = direction.y.abs() * 0.8 + 0.1;

        let end = CEREBRUM_CENTRE + on_ellipsoid(CEREBRUM_RADII, direction) * 0.9;

        // Tracts leave the stem together before they fan out.
        let bend = Vec3::new(end.x * 0.25 - 0.1, -0.15, end.z * 0.3);

        let intensity = 0.07 + 0.06 * random.unit();
        let width = 3.0 + 4.0 * random.unit();

        let point = |step: usize| {
            let along = step as f32 / SEGMENTS_PER_TRACT as f32;

            // A quadratic curve from the stem, past the bend, to the end.
            let first = STEM.lerp(bend, along);
            let second = bend.lerp(end, along);

            first.lerp(second, along)
        };

        for step in 0..SEGMENTS_PER_TRACT {
            fibres.push(Fibre {
                start_width: point(step).extend(width).to_array(),
                end_flow: point(step + 1).extend(0.0).to_array(),
                color_intensity: [0.25, 1.0, 0.42, intensity],
                timing: [LONG_AGO, 0.0, 0.0, 0.0],
            });
        }
    }

    fibres
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scenery_is_the_same_every_time() {
        assert_eq!(
            dots().len(),
            CEREBRUM_DOTS + CEREBELLUM_DOTS + TEMPORAL_DOTS
        );
        assert_eq!(dots()[77].from_radius, dots()[77].from_radius);
        assert_eq!(tracts().len(), TRACTS * SEGMENTS_PER_TRACT);
    }

    #[test]
    fn the_hemispheres_are_parted() {
        let in_the_gap = dots()[..CEREBRUM_DOTS]
            .iter()
            .filter(|dot| dot.from_radius[2].abs() < FISSURE)
            .count();

        assert_eq!(in_the_gap, 0);
    }

    #[test]
    fn each_tract_is_one_unbroken_curve() {
        let fibres = tracts();

        for pair in fibres[..SEGMENTS_PER_TRACT].windows(2) {
            assert_eq!(pair[0].end_flow[..3], pair[1].start_width[..3]);
        }
    }
}

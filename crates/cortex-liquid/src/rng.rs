//! A small fixed-seed random number source, so that training gives the
//! same result every time for the same seed. Not for anything secret.

/// An xorshift generator with a multiply on the way out.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// The same seed always gives the same numbers. Zero is replaced,
    /// because an xorshift state of zero stays zero.
    pub fn new(seed: u64) -> Self {
        let mut rng = Self(seed ^ 0x9E37_79B9_7F4A_7C15);

        if rng.0 == 0 {
            rng.0 = 0x2545_F491_4F6C_DD1D;
        }

        // The first few outputs of a small seed are poorly mixed.
        for _ in 0..8 {
            rng.draw();
        }

        rng
    }

    pub fn draw(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;

        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number from 0 up to, not including, 1.
    pub fn unit(&mut self) -> f64 {
        (self.draw() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A number from `-reach` to `reach`.
    pub fn around(&mut self, reach: f64) -> f64 {
        (self.unit() * 2.0 - 1.0) * reach
    }

    /// A whole number below `limit`, which must not be zero.
    pub fn below(&mut self, limit: usize) -> usize {
        (self.draw() % limit as u64) as usize
    }

    /// A draw from the standard normal distribution.
    pub fn normal(&mut self) -> f64 {
        let radius = (-2.0 * (1.0 - self.unit()).ln()).sqrt();

        radius * (std::f64::consts::TAU * self.unit()).cos()
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for at in (1..items.len()).rev() {
            items.swap(at, self.below(at + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_numbers() {
        let mut left = Rng::new(7);
        let mut right = Rng::new(7);

        assert!((0..100).all(|_| left.draw() == right.draw()));
        assert_ne!(Rng::new(7).draw(), Rng::new(8).draw());
    }

    #[test]
    fn normal_draws_have_about_the_right_spread() {
        let mut rng = Rng::new(3);
        let draws: Vec<f64> = (0..20_000).map(|_| rng.normal()).collect();

        let mean = draws.iter().sum::<f64>() / draws.len() as f64;
        let spread =
            (draws.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / draws.len() as f64).sqrt();

        assert!(mean.abs() < 0.03, "{mean}");
        assert!((spread - 1.0).abs() < 0.03, "{spread}");
    }
}

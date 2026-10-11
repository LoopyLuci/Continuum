use rand::SeedableRng;
use rand::Rng;
use std::time::{Duration, Instant};

/// Fuzz harness for protocol testing
pub struct FuzzHarness {
    iterations: usize,
    seed: u64,
}

/// Fuzz test result
#[derive(Debug, Clone)]
pub struct FuzzResult {
    pub iterations: usize,
    pub failures: usize,
    pub duration: Duration,
    pub coverage_percent: f32,
}

impl FuzzHarness {
    pub fn new(iterations: usize) -> Self {
        Self {
            iterations,
            seed: rand::random(),
        }
    }

    /// Run fuzz tests on a target function
    pub fn run<F, T>(&self, target: F) -> FuzzResult
    where
        F: Fn(&[u8]) -> Result<T, String>,
    {
        let start = Instant::now();
        let mut failures = 0;
        // Seeded from `self.seed`, not `thread_rng`.
        //
        // A fuzz harness whose seed is stored and then ignored cannot reproduce
        // a failure: the seed is the only handle on which input caused it, and
        // this throws that handle away. It is also the reason `seed` existed
        // and was never read.
        let mut rng = rand::rngs::StdRng::seed_from_u64(self.seed);

        for _ in 0..self.iterations {
            let len = rng.gen_range(1..=1024);
            let mut input = vec![0u8; len];
            rng.fill(&mut input[..]);

            if target(&input).is_err() {
                failures += 1;
            }
        }

        FuzzResult {
            iterations: self.iterations,
            failures,
            duration: start.elapsed(),
            coverage_percent: 0.0,
        }
    }

    /// Generate random bytes
    pub fn random_bytes(&self, len: usize) -> Vec<u8> {
        let mut rng = rand::thread_rng();
        let mut bytes = vec![0u8; len];
        rng.fill(&mut bytes[..]);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzz_harness() {
        let harness = FuzzHarness::new(100);
        let result = harness.run(|_input| Ok(()));
        assert_eq!(result.iterations, 100);
        assert_eq!(result.failures, 0);
    }

    #[test]
    fn test_random_bytes() {
        let harness = FuzzHarness::new(10);
        let bytes = harness.random_bytes(32);
        assert_eq!(bytes.len(), 32);
    }
}

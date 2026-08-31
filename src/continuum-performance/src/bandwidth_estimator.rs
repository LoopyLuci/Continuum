use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Bandwidth sample
#[derive(Debug, Clone)]
pub struct BandwidthSample {
    pub bytes: u64,
    pub duration: Duration,
    pub timestamp: Instant,
}

/// Bandwidth estimator using sliding window
pub struct BandwidthEstimator {
    samples: VecDeque<BandwidthSample>,
    window_size: usize,
    max_window_duration: Duration,
}

impl BandwidthEstimator {
    pub fn new() -> Self {
        Self {
            samples: VecDeque::new(),
            window_size: 100,
            max_window_duration: Duration::from_secs(5),
        }
    }

    /// Add a new sample
    pub fn add_sample(&mut self, bytes: u64, duration: Duration) {
        let sample = BandwidthSample {
            bytes,
            duration,
            timestamp: Instant::now(),
        };

        self.samples.push_back(sample);

        // Remove old samples
        let cutoff = Instant::now() - self.max_window_duration;
        while let Some(front) = self.samples.front() {
            if front.timestamp < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }

        // Limit window size
        while self.samples.len() > self.window_size {
            self.samples.pop_front();
        }
    }

    /// Estimate current bandwidth in kbps
    pub fn estimate_kbps(&self) -> f64 {
        if self.samples.len() < 2 {
            return 0.0;
        }

        let total_bytes: u64 = self.samples.iter().map(|s| s.bytes).sum();
        let total_duration: Duration = self.samples.iter().map(|s| s.duration).sum();

        if total_duration.as_secs_f64() == 0.0 {
            return 0.0;
        }

        (total_bytes as f64 * 8.0) / total_duration.as_secs_f64() / 1000.0
    }

    /// Get the number of samples
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_estimator() {
        let estimator = BandwidthEstimator::new();
        assert_eq!(estimator.estimate_kbps(), 0.0);
    }

    #[test]
    fn test_single_sample() {
        let mut estimator = BandwidthEstimator::new();
        estimator.add_sample(1000, Duration::from_millis(100));
        assert_eq!(estimator.sample_count(), 1);
    }

    #[test]
    fn test_bandwidth_estimate() {
        let mut estimator = BandwidthEstimator::new();
        // Add 10 samples of 1000 bytes each over 100ms = 80 kbps
        for _ in 0..10 {
            estimator.add_sample(1000, Duration::from_millis(100));
        }
        let kbps = estimator.estimate_kbps();
        assert!(kbps > 0.0);
    }
}

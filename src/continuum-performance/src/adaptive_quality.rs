use serde::{Deserialize, Serialize};

/// Connection statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionStats {
    pub rtt_ms: f32,
    pub packet_loss: f32,
    pub bandwidth_kbps: f32,
    pub jitter_ms: f32,
}

/// Quality decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityDecision {
    pub quality: u8,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub resolution_scale: f32,
}

/// Adaptive quality controller
pub struct AdaptiveQualityController {
    target_latency_ms: f32,
    current_quality: u8,
    current_fps: u32,
    min_quality: u8,
    max_quality: u8,
}

impl AdaptiveQualityController {
    pub fn new(target_latency_ms: f32) -> Self {
        Self {
            target_latency_ms,
            current_quality: 80,
            current_fps: 60,
            min_quality: 20,
            max_quality: 100,
        }
    }

    /// Update quality based on connection stats
    pub fn update(&mut self, stats: &ConnectionStats) -> QualityDecision {
        if stats.rtt_ms > self.target_latency_ms * 1.5 {
            // Reduce quality
            self.current_quality = (self.current_quality.saturating_sub(5)).max(self.min_quality);
            if stats.rtt_ms > self.target_latency_ms * 3.0 {
                self.current_fps = (self.current_fps / 2).max(15);
            }
        } else if stats.rtt_ms < self.target_latency_ms * 0.5 {
            // Increase quality
            self.current_quality = (self.current_quality + 5).min(self.max_quality);
            if stats.rtt_ms < self.target_latency_ms * 0.2 {
                self.current_fps = (self.current_fps + 10).min(144);
            }
        }

        QualityDecision {
            quality: self.current_quality,
            fps: self.current_fps,
            bitrate_kbps: (self.current_quality as f32 * 100.0) as u32,
            resolution_scale: self.current_quality as f32 / 100.0,
        }
    }

    /// Get current quality
    pub fn current_quality(&self) -> u8 {
        self.current_quality
    }

    /// Get current FPS
    pub fn current_fps(&self) -> u32 {
        self.current_fps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_quality_reduces_on_high_latency() {
        let mut controller = AdaptiveQualityController::new(20.0);
        let stats = ConnectionStats {
            rtt_ms: 100.0, // Very high latency
            packet_loss: 0.0,
            bandwidth_kbps: 10000.0,
            jitter_ms: 0.0,
        };

        let decision = controller.update(&stats);
        assert!(decision.quality < 80);
    }

    #[test]
    fn test_adaptive_quality_increases_on_low_latency() {
        let mut controller = AdaptiveQualityController::new(20.0);
        let stats = ConnectionStats {
            rtt_ms: 1.0, // Very low latency
            packet_loss: 0.0,
            bandwidth_kbps: 10000.0,
            jitter_ms: 0.0,
        };

        let decision = controller.update(&stats);
        assert!(decision.quality > 80);
    }
}

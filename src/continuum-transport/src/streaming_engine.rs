use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RateControlConfig {
    pub min_quality: u8,
    pub max_quality: u8,
    pub target_fps: u32,
    pub min_fps: u32,
    pub max_fps: u32,
    pub target_latency_ms: f32,
    pub bandwidth_target_kbps: u32,
    pub resolution_tiers: u8,
}

impl Default for RateControlConfig {
    fn default() -> Self {
        Self {
            min_quality: 30,
            max_quality: 95,
            target_fps: 30,
            min_fps: 10,
            max_fps: 60,
            target_latency_ms: 50.0,
            bandwidth_target_kbps: 10000,
            resolution_tiers: 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateControlSample {
    pub timestamp: DateTime<Utc>,
    pub encode_time_us: u64,
    pub frame_size_bytes: u32,
    pub rtt_ms: f32,
    pub client_decode_time_us: Option<u64>,
    pub bandwidth_estimate_kbps: f32,
    pub packet_loss: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionTier {
    Original,
    Tier1, // 75% of original
    Tier2, // 50% of original
}

#[derive(Debug, Clone)]
pub struct RateControlDecision {
    pub quality: u8,
    pub fps: u32,
    pub resolution: ResolutionTier,
    pub bitrate_kbps: u32,
    pub should_send_keyframe: bool,
    pub skip_frame: bool,
    pub reason: &'static str,
}

pub struct RateController {
    config: RateControlConfig,
    history: Vec<RateControlSample>,
    max_history: usize,
    current_quality: u8,
    current_fps: u32,
    consecutive_skips: u32,
    last_keyframe_seq: u64,
    frame_seq: u64,
}

impl RateController {
    pub fn new(config: RateControlConfig) -> Self {
        Self {
            current_quality: config.max_quality,
            current_fps: config.target_fps,
            config,
            history: Vec::new(),
            max_history: 120,
            consecutive_skips: 0,
            last_keyframe_seq: 0,
            frame_seq: 0,
        }
    }

    pub fn record_sample(&mut self, sample: RateControlSample) {
        self.history.push(sample);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    pub fn decide(&mut self) -> RateControlDecision {
        self.frame_seq += 1;

        if self.history.len() < 3 {
            return RateControlDecision {
                quality: self.current_quality,
                fps: self.current_fps,
                resolution: ResolutionTier::Original,
                bitrate_kbps: self.config.bandwidth_target_kbps,
                should_send_keyframe: true,
                skip_frame: false,
                reason: "startup",
            };
        }

        let recent: Vec<_> = self.history.iter().rev().take(10).collect();
        let avg_encode: f64 =
            recent.iter().map(|s| s.encode_time_us as f64).sum::<f64>() / recent.len() as f64;
        let avg_rtt: f64 =
            recent.iter().map(|s| s.rtt_ms as f64).sum::<f64>() / recent.len() as f64;
        let avg_bw: f64 = recent
            .iter()
            .map(|s| s.bandwidth_estimate_kbps as f64)
            .sum::<f64>()
            / recent.len() as f64;
        let avg_loss: f64 =
            recent.iter().map(|s| s.packet_loss as f64).sum::<f64>() / recent.len() as f64;

        let target_frame_time_us = 1_000_000.0 / self.current_fps as f64;

        // Encode time too high → reduce quality
        if avg_encode > target_frame_time_us * 1.5 && self.current_quality > self.config.min_quality
        {
            self.current_quality = self
                .current_quality
                .saturating_sub(5)
                .max(self.config.min_quality);
            return RateControlDecision {
                quality: self.current_quality,
                fps: self.current_fps,
                resolution: ResolutionTier::Original,
                bitrate_kbps: avg_bw as u32,
                should_send_keyframe: false,
                skip_frame: false,
                reason: "encode-time",
            };
        }

        // Latency too high → reduce FPS or resolution
        if avg_rtt > self.config.target_latency_ms as f64 * 2.0 {
            if self.current_fps > self.config.min_fps {
                self.current_fps = (self.current_fps / 2).max(self.config.min_fps);
                return RateControlDecision {
                    quality: self.current_quality,
                    fps: self.current_fps,
                    resolution: ResolutionTier::Original,
                    bitrate_kbps: avg_bw as u32,
                    should_send_keyframe: false,
                    skip_frame: false,
                    reason: "high-rtt",
                };
            }
            return RateControlDecision {
                quality: self.current_quality,
                fps: self.current_fps,
                resolution: ResolutionTier::Tier1,
                bitrate_kbps: avg_bw as u32,
                should_send_keyframe: false,
                skip_frame: false,
                reason: "high-latency-reduce-res",
            };
        }

        // Packet loss → reduce quality aggressively
        if avg_loss > 0.05 {
            self.current_quality = self
                .current_quality
                .saturating_sub(10)
                .max(self.config.min_quality);
            return RateControlDecision {
                quality: self.current_quality,
                fps: (self.current_fps / 2).max(self.config.min_fps),
                resolution: ResolutionTier::Tier2,
                bitrate_kbps: (avg_bw * 0.5) as u32,
                should_send_keyframe: true,
                skip_frame: false,
                reason: "packet-loss",
            };
        }

        // Bandwidth constraint
        let avg_frame_size = recent
            .iter()
            .map(|s| s.frame_size_bytes as f64)
            .sum::<f64>()
            / recent.len() as f64;
        let bits_per_second = avg_frame_size * 8.0 * self.current_fps as f64 / 1000.0;
        if bits_per_second > avg_bw * 0.9 {
            self.current_quality = self
                .current_quality
                .saturating_sub(5)
                .max(self.config.min_quality);
            return RateControlDecision {
                quality: self.current_quality,
                fps: self.current_fps,
                resolution: ResolutionTier::Original,
                bitrate_kbps: avg_bw as u32,
                should_send_keyframe: false,
                skip_frame: false,
                reason: "bandwidth",
            };
        }

        // Everything fine → increase quality if possible
        if avg_encode < target_frame_time_us * 0.5 && self.current_quality < self.config.max_quality
        {
            self.current_quality = (self.current_quality + 3).min(self.config.max_quality);
        }

        // Stable → increase FPS towards target
        if avg_rtt < self.config.target_latency_ms as f64 * 0.5
            && self.current_fps < self.config.target_fps
        {
            self.current_fps = (self.current_fps + 5).min(self.config.target_fps);
        }

        // Skip frame if encode is too slow
        if avg_encode > target_frame_time_us * 2.0 && self.consecutive_skips < 5 {
            self.consecutive_skips += 1;
            return RateControlDecision {
                quality: self.current_quality,
                fps: self.current_fps,
                resolution: ResolutionTier::Original,
                bitrate_kbps: avg_bw as u32,
                should_send_keyframe: false,
                skip_frame: true,
                reason: "encode-too-slow",
            };
        }
        self.consecutive_skips = 0;

        let should_keyframe = (self.frame_seq - self.last_keyframe_seq) >= 30;
        let is_keyframe = should_keyframe;
        if is_keyframe {
            self.last_keyframe_seq = self.frame_seq;
        }

        RateControlDecision {
            quality: self.current_quality,
            fps: self.current_fps,
            resolution: ResolutionTier::Original,
            bitrate_kbps: avg_bw as u32,
            should_send_keyframe: is_keyframe,
            skip_frame: false,
            reason: "normal",
        }
    }

    pub fn config(&self) -> &RateControlConfig {
        &self.config
    }

    pub fn current_quality(&self) -> u8 {
        self.current_quality
    }

    pub fn current_fps(&self) -> u32 {
        self.current_fps
    }

    pub fn frame_seq(&self) -> u64 {
        self.frame_seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sample(encode_us: u64, frame_size: u32, rtt: f32, loss: f32) -> RateControlSample {
        RateControlSample {
            timestamp: chrono::Utc::now(),
            encode_time_us: encode_us,
            frame_size_bytes: frame_size,
            rtt_ms: rtt,
            client_decode_time_us: None,
            bandwidth_estimate_kbps: 10000.0,
            packet_loss: loss,
        }
    }

    #[test]
    fn test_rate_controller_default_config() {
        let config = RateControlConfig::default();
        assert_eq!(config.min_quality, 30);
        assert_eq!(config.max_quality, 95);
        assert_eq!(config.target_fps, 30);
    }

    #[test]
    fn test_rate_controller_startup() {
        let mut rc = RateController::new(RateControlConfig::default());
        let decision = rc.decide();
        assert_eq!(decision.reason, "startup");
        assert!(decision.should_send_keyframe);
    }

    #[test]
    fn test_rate_controller_normal_operation() {
        let mut rc = RateController::new(RateControlConfig::default());
        for _ in 0..5 {
            rc.record_sample(make_sample(5000, 10000, 20.0, 0.0));
        }
        let decision = rc.decide();
        assert_eq!(decision.reason, "normal");
    }

    #[test]
    fn test_rate_controller_high_encode_time() {
        let mut rc = RateController::new(RateControlConfig::default());
        for _ in 0..5 {
            rc.record_sample(make_sample(100000, 50000, 20.0, 0.0));
        }
        let decision = rc.decide();
        assert_eq!(decision.reason, "encode-time");
    }

    #[test]
    fn test_rate_controller_packet_loss() {
        let mut rc = RateController::new(RateControlConfig::default());
        for _ in 0..5 {
            rc.record_sample(make_sample(5000, 10000, 20.0, 0.1));
        }
        let decision = rc.decide();
        assert_eq!(decision.reason, "packet-loss");
    }

    #[test]
    fn test_rate_controller_current_quality() {
        let rc = RateController::new(RateControlConfig::default());
        assert_eq!(rc.current_quality(), 95);
    }

    #[test]
    fn test_rate_controller_current_fps() {
        let rc = RateController::new(RateControlConfig::default());
        assert_eq!(rc.current_fps(), 30);
    }

    #[test]
    fn test_rate_controller_frame_seq() {
        let mut rc = RateController::new(RateControlConfig::default());
        assert_eq!(rc.frame_seq(), 0);
        rc.decide();
        assert_eq!(rc.frame_seq(), 1);
    }
}

pub struct NetworkForecaster {
    history: Vec<NetworkSample>,
    max_history: usize,
}

#[derive(Debug, Clone)]
pub struct NetworkSample {
    pub timestamp: std::time::Instant,
    pub rtt_ms: f32,
    pub bandwidth_kbps: f32,
    pub packet_loss: f32,
}

#[derive(Debug, Clone)]
pub struct NetworkPrediction {
    pub predicted_bandwidth_kbps: f32,
    pub predicted_rtt_ms: f32,
    pub recommended_quality: u8,
    pub recommended_fps: u32,
    pub confidence: f32,
}

impl NetworkForecaster {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            max_history: 300,
        }
    }

    pub fn record_sample(&mut self, sample: NetworkSample) {
        self.history.push(sample);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    pub fn predict(&self) -> Option<NetworkPrediction> {
        if self.history.len() < 5 {
            return None;
        }

        let recent = &self.history[self.history.len() - 10..];

        let avg_bw: f32 =
            recent.iter().map(|s| s.bandwidth_kbps).sum::<f32>() / recent.len() as f32;
        let avg_rtt: f32 = recent.iter().map(|s| s.rtt_ms).sum::<f32>() / recent.len() as f32;
        let avg_loss: f32 = recent.iter().map(|s| s.packet_loss).sum::<f32>() / recent.len() as f32;

        let bw_trend = if recent.len() >= 2 {
            let first_half: f32 = recent[..recent.len() / 2]
                .iter()
                .map(|s| s.bandwidth_kbps)
                .sum::<f32>()
                / (recent.len() / 2) as f32;
            let second_half: f32 = recent[recent.len() / 2..]
                .iter()
                .map(|s| s.bandwidth_kbps)
                .sum::<f32>()
                / (recent.len() / 2) as f32;
            second_half - first_half
        } else {
            0.0
        };

        let predicted_bw = avg_bw + bw_trend * 0.5;
        let predicted_rtt = avg_rtt;

        let recommended_quality = if avg_loss > 0.05 {
            40
        } else if avg_loss > 0.01 || predicted_bw < 2000.0 {
            60
        } else if predicted_bw < 5000.0 {
            75
        } else {
            85
        };

        let recommended_fps = if avg_loss > 0.05 {
            15
        } else if avg_loss > 0.01 {
            24
        } else if predicted_bw < 5000.0 {
            30
        } else {
            60
        };

        let confidence = (self.history.len() as f32 / 30.0).min(1.0);

        Some(NetworkPrediction {
            predicted_bandwidth_kbps: predicted_bw.max(100.0),
            predicted_rtt_ms: predicted_rtt.max(1.0),
            recommended_quality,
            recommended_fps,
            confidence,
        })
    }

    pub fn predict_bitrate_kbps(&self, jitter_ms: f32, packet_loss: f32) -> u32 {
        ((8000.0 - jitter_ms * 50.0 - packet_loss * 2000.0).max(2000.0)) as u32
    }
}

impl Default for NetworkForecaster {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_predict_bitrate() {
        let forecaster = NetworkForecaster::new();
        let bitrate = forecaster.predict_bitrate_kbps(10.0, 0.01);
        assert!(bitrate > 5000);
        assert!(bitrate <= 8000);
    }

    #[test]
    fn test_prediction_with_history() {
        let mut forecaster = NetworkForecaster::new();
        for i in 0..10 {
            forecaster.record_sample(NetworkSample {
                timestamp: std::time::Instant::now(),
                rtt_ms: 20.0 + (i as f32 * 0.5),
                bandwidth_kbps: 10000.0 - (i as f32 * 100.0),
                packet_loss: 0.001 * i as f32,
            });
        }
        let prediction = forecaster.predict();
        assert!(prediction.is_some());
        let p = prediction.unwrap();
        assert!(p.recommended_quality > 0);
        assert!(p.recommended_fps > 0);
    }

    #[test]
    fn test_predict_none_with_few_samples() {
        let mut forecaster = NetworkForecaster::new();
        for _i in 0..3 {
            forecaster.record_sample(NetworkSample {
                timestamp: std::time::Instant::now(),
                rtt_ms: 20.0,
                bandwidth_kbps: 10000.0,
                packet_loss: 0.0,
            });
        }
        assert!(forecaster.predict().is_none());
    }

    #[test]
    fn test_predict_high_loss_reduces_quality() {
        let mut forecaster = NetworkForecaster::new();
        for _ in 0..10 {
            forecaster.record_sample(NetworkSample {
                timestamp: std::time::Instant::now(),
                rtt_ms: 50.0,
                bandwidth_kbps: 5000.0,
                packet_loss: 0.1,
            });
        }
        let p = forecaster.predict().unwrap();
        assert!(p.recommended_quality <= 40);
    }

    #[test]
    fn test_predict_bitrate_high_jitter() {
        let forecaster = NetworkForecaster::new();
        let bitrate = forecaster.predict_bitrate_kbps(50.0, 0.0);
        assert!(bitrate < 8000);
    }

    #[test]
    fn test_predict_bitrate_high_loss() {
        let forecaster = NetworkForecaster::new();
        let bitrate = forecaster.predict_bitrate_kbps(10.0, 0.1);
        // 8000 - 10*50 - 0.1*2000 = 8000 - 500 - 200 = 7300
        assert!(bitrate > 5000);
        assert!(bitrate <= 8000);
    }

    #[test]
    fn test_predict_confidence_grows() {
        let mut forecaster = NetworkForecaster::new();
        for _i in 0..30 {
            forecaster.record_sample(NetworkSample {
                timestamp: std::time::Instant::now(),
                rtt_ms: 20.0,
                bandwidth_kbps: 10000.0,
                packet_loss: 0.0,
            });
        }
        let p = forecaster.predict().unwrap();
        assert!((p.confidence - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_history_bounded() {
        let mut forecaster = NetworkForecaster::new();
        for _ in 0..400 {
            forecaster.record_sample(NetworkSample {
                timestamp: std::time::Instant::now(),
                rtt_ms: 20.0,
                bandwidth_kbps: 10000.0,
                packet_loss: 0.0,
            });
        }
        assert!(forecaster.history.len() <= 300);
    }
}

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct AnomalyDetector {
    encode_times: VecDeque<f64>,
    frame_sizes: VecDeque<f64>,
    window_size: usize,
    threshold_sigma: f64,
    warnings: Vec<AnomalyWarning>,
}

#[derive(Debug, Clone)]
pub struct AnomalyWarning {
    pub metric: String,
    pub value: f64,
    pub mean: f64,
    pub std_dev: f64,
    pub sigma: f64,
    pub message: String,
}

impl AnomalyDetector {
    pub fn new(window_size: usize, threshold_sigma: f64) -> Self {
        Self {
            encode_times: VecDeque::with_capacity(window_size),
            frame_sizes: VecDeque::with_capacity(window_size),
            window_size,
            threshold_sigma,
            warnings: Vec::new(),
        }
    }

    pub fn record_encode_time(&mut self, time_us: f64) {
        self.encode_times.push_back(time_us);
        if self.encode_times.len() > self.window_size {
            self.encode_times.pop_front();
        }
        self.check_anomaly("encode_time", time_us);
    }

    pub fn record_frame_size(&mut self, size_bytes: f64) {
        self.frame_sizes.push_back(size_bytes);
        if self.frame_sizes.len() > self.window_size {
            self.frame_sizes.pop_front();
        }
        self.check_anomaly("frame_size", size_bytes);
    }

    fn check_anomaly(&mut self, metric: &str, value: f64) {
        let data = match metric {
            "encode_time" => &self.encode_times,
            "frame_size" => &self.frame_sizes,
            _ => return,
        };

        if data.len() < 10 {
            return;
        }

        let mean = data.iter().sum::<f64>() / data.len() as f64;
        let variance =
            data.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / data.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev < 1e-10 {
            return;
        }

        let sigma = (value - mean).abs() / std_dev;

        if sigma > self.threshold_sigma {
            let warning = AnomalyWarning {
                metric: metric.to_string(),
                value,
                mean,
                std_dev,
                sigma,
                message: format!(
                    "Anomaly detected: {} = {:.1} (mean={:.1}, σ={:.1}, {:.1}σ deviation)",
                    metric, value, mean, std_dev, sigma
                ),
            };
            tracing::warn!("{}", warning.message);
            self.warnings.push(warning);
        }
    }

    pub fn warnings(&self) -> &[AnomalyWarning] {
        &self.warnings
    }

    pub fn clear_warnings(&mut self) {
        self.warnings.clear();
    }

    pub fn mean_encode_time(&self) -> f64 {
        if self.encode_times.is_empty() {
            return 0.0;
        }
        self.encode_times.iter().sum::<f64>() / self.encode_times.len() as f64
    }

    pub fn mean_frame_size(&self) -> f64 {
        if self.frame_sizes.is_empty() {
            return 0.0;
        }
        self.frame_sizes.iter().sum::<f64>() / self.frame_sizes.len() as f64
    }

    pub fn is_anomaly(&self, metric: &str, value: f64) -> bool {
        let data = match metric {
            "encode_time" => &self.encode_times,
            "frame_size" => &self.frame_sizes,
            _ => return false,
        };

        if data.len() < 10 {
            return false;
        }

        let mean = data.iter().sum::<f64>() / data.len() as f64;
        let variance =
            data.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / data.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev < 1e-10 {
            return false;
        }

        let sigma = (value - mean).abs() / std_dev;
        sigma > self.threshold_sigma
    }
}

impl Default for AnomalyDetector {
    fn default() -> Self {
        Self::new(100, 3.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anomaly_detector_normal_values() {
        let mut detector = AnomalyDetector::new(100, 3.0);
        for i in 0..50 {
            detector.record_encode_time(8000.0 + (i as f64 * 10.0));
        }
        assert_eq!(detector.warnings().len(), 0);
    }

    #[test]
    fn test_anomaly_detector_spike() {
        let mut detector = AnomalyDetector::new(100, 3.0);
        for _ in 0..50 {
            detector.record_encode_time(8000.0);
        }
        detector.record_encode_time(50000.0);
        assert!(detector.warnings().len() > 0);
    }

    #[test]
    fn test_anomaly_detector_frame_size_spike() {
        let mut detector = AnomalyDetector::new(100, 3.0);
        for _ in 0..50 {
            detector.record_frame_size(15000.0);
        }
        detector.record_frame_size(500000.0);
        assert!(detector.warnings().len() > 0);
    }

    #[test]
    fn test_anomaly_detector_insufficient_data() {
        let mut detector = AnomalyDetector::new(100, 3.0);
        for i in 0..5 {
            detector.record_encode_time(8000.0 + i as f64);
        }
        assert_eq!(detector.warnings().len(), 0);
    }

    #[test]
    fn test_anomaly_detector_mean() {
        let mut detector = AnomalyDetector::new(100, 3.0);
        for _ in 0..100 {
            detector.record_encode_time(10000.0);
        }
        assert!((detector.mean_encode_time() - 10000.0).abs() < 1.0);
    }

    #[test]
    fn test_anomaly_detector_clear_warnings() {
        let mut detector = AnomalyDetector::new(10, 1.0);
        for _ in 0..20 {
            detector.record_encode_time(8000.0);
        }
        detector.record_encode_time(100000.0);
        assert!(detector.warnings().len() > 0);
        detector.clear_warnings();
        assert_eq!(detector.warnings().len(), 0);
    }
}

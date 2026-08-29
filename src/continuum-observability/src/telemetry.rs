use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub struct TelemetryLayer {
    metrics: Arc<MetricsRegistry>,
}

#[derive(Default)]
pub struct MetricsRegistry {
    pub frames_encoded: AtomicI64,
    pub frames_decoded: AtomicI64,
    pub bytes_sent: AtomicI64,
    pub bytes_received: AtomicI64,
    pub active_connections: AtomicI64,
    pub errors: AtomicI64,
    pub encode_time_total_us: AtomicI64,
    pub decode_time_total_us: AtomicI64,
    pub encode_count: AtomicI64,
    pub decode_count: AtomicI64,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_encode(&self, time_us: u64, size: u64) {
        self.frames_encoded.fetch_add(1, Ordering::Relaxed);
        self.encode_time_total_us
            .fetch_add(time_us as i64, Ordering::Relaxed);
        self.bytes_sent.fetch_add(size as i64, Ordering::Relaxed);
        self.encode_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_decode(&self, time_us: u64, size: u64) {
        self.frames_decoded.fetch_add(1, Ordering::Relaxed);
        self.decode_time_total_us
            .fetch_add(time_us as i64, Ordering::Relaxed);
        self.bytes_received
            .fetch_add(size as i64, Ordering::Relaxed);
        self.decode_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn avg_encode_time_us(&self) -> f64 {
        let count = self.encode_count.load(Ordering::Relaxed).max(1);
        self.encode_time_total_us.load(Ordering::Relaxed) as f64 / count as f64
    }

    pub fn avg_decode_time_us(&self) -> f64 {
        let count = self.decode_count.load(Ordering::Relaxed).max(1);
        self.decode_time_total_us.load(Ordering::Relaxed) as f64 / count as f64
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            frames_encoded: self.frames_encoded.load(Ordering::Relaxed),
            frames_decoded: self.frames_decoded.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            avg_encode_time_us: self.avg_encode_time_us(),
            avg_decode_time_us: self.avg_decode_time_us(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub frames_encoded: i64,
    pub frames_decoded: i64,
    pub bytes_sent: i64,
    pub bytes_received: i64,
    pub active_connections: i64,
    pub errors: i64,
    pub avg_encode_time_us: f64,
    pub avg_decode_time_us: f64,
}

impl TelemetryLayer {
    pub fn new(service_name: &str) -> Self {
        let _ = service_name;

        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_target(true))
            .init();

        Self {
            metrics: Arc::new(MetricsRegistry::new()),
        }
    }

    pub fn metrics(&self) -> Arc<MetricsRegistry> {
        self.metrics.clone()
    }

    pub fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_registry_new() {
        let registry = MetricsRegistry::new();
        let snapshot = registry.snapshot();
        assert_eq!(snapshot.frames_encoded, 0);
        assert_eq!(snapshot.frames_decoded, 0);
        assert_eq!(snapshot.bytes_sent, 0);
        assert_eq!(snapshot.bytes_received, 0);
        assert_eq!(snapshot.active_connections, 0);
        assert_eq!(snapshot.errors, 0);
    }

    #[test]
    fn test_metrics_registry_record_encode() {
        let registry = MetricsRegistry::new();
        registry.record_encode(1000, 5000);
        registry.record_encode(2000, 6000);

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.frames_encoded, 2);
        assert_eq!(snapshot.bytes_sent, 11000);
        assert_eq!(snapshot.avg_encode_time_us, 1500.0);
    }

    #[test]
    fn test_metrics_registry_record_decode() {
        let registry = MetricsRegistry::new();
        registry.record_decode(500, 3000);
        registry.record_decode(1500, 4000);

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.frames_decoded, 2);
        assert_eq!(snapshot.bytes_received, 7000);
        assert_eq!(snapshot.avg_decode_time_us, 1000.0);
    }

    #[test]
    fn test_metrics_registry_avg_encode_empty() {
        let registry = MetricsRegistry::new();
        assert_eq!(registry.avg_encode_time_us(), 0.0);
    }

    #[test]
    fn test_metrics_registry_avg_decode_empty() {
        let registry = MetricsRegistry::new();
        assert_eq!(registry.avg_decode_time_us(), 0.0);
    }

    #[test]
    fn test_metrics_snapshot_serialization() {
        let registry = MetricsRegistry::new();
        registry.record_encode(1000, 5000);
        let snapshot = registry.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("frames_encoded"));
    }
}

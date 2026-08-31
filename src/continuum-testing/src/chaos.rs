use rand::Rng;
use std::time::Duration;

/// Network conditions for chaos testing
#[derive(Debug, Clone)]
pub struct NetworkConditions {
    pub latency_ms: u64,
    pub jitter_ms: u64,
    pub packet_loss_percent: f32,
    pub bandwidth_kbps: u64,
}

impl Default for NetworkConditions {
    fn default() -> Self {
        Self {
            latency_ms: 0,
            jitter_ms: 0,
            packet_loss_percent: 0.0,
            bandwidth_kbps: 100000,
        }
    }
}

/// CPU load simulation
#[derive(Debug, Clone)]
pub struct CpuLoad {
    pub target_percent: u8,
    pub duration: Duration,
}

/// Chaos engineering engine
pub struct ChaosEngine {
    network_conditions: NetworkConditions,
    cpu_load: Option<CpuLoad>,
}

impl ChaosEngine {
    pub fn new() -> Self {
        Self {
            network_conditions: NetworkConditions::default(),
            cpu_load: None,
        }
    }

    /// Set network conditions
    pub fn with_network_conditions(mut self, conditions: NetworkConditions) -> Self {
        self.network_conditions = conditions;
        self
    }

    /// Set CPU load
    pub fn with_cpu_load(mut self, load: CpuLoad) -> Self {
        self.cpu_load = Some(load);
        self
    }

    /// Simulate network latency
    pub async fn simulate_latency(&self) {
        let mut rng = rand::thread_rng();
        let jitter = if self.network_conditions.jitter_ms > 0 {
            rng.gen_range(0..self.network_conditions.jitter_ms)
        } else {
            0
        };
        let total_ms = self.network_conditions.latency_ms + jitter;
        if total_ms > 0 {
            tokio::time::sleep(Duration::from_millis(total_ms)).await;
        }
    }

    /// Simulate packet loss
    pub fn should_drop_packet(&self) -> bool {
        let mut rng = rand::thread_rng();
        let roll: f32 = rng.gen();
        roll < (self.network_conditions.packet_loss_percent / 100.0)
    }

    /// Get current network conditions
    pub fn network_conditions(&self) -> &NetworkConditions {
        &self.network_conditions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_network_conditions() {
        let conditions = NetworkConditions::default();
        assert_eq!(conditions.latency_ms, 0);
        assert_eq!(conditions.packet_loss_percent, 0.0);
    }

    #[test]
    fn test_chaos_engine_builder() {
        let engine = ChaosEngine::new()
            .with_network_conditions(NetworkConditions {
                latency_ms: 50,
                packet_loss_percent: 5.0,
                ..Default::default()
            });

        assert_eq!(engine.network_conditions().latency_ms, 50);
    }

    #[test]
    fn test_packet_loss_simulation() {
        let engine = ChaosEngine::new()
            .with_network_conditions(NetworkConditions {
                packet_loss_percent: 100.0,
                ..Default::default()
            });

        // With 100% packet loss, all packets should be dropped
        assert!(engine.should_drop_packet());
    }
}

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::time::Duration;

/// A discovered peer on the local network
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub machine_id: String,
    pub display_name: String,
    pub address: String,
    pub port: u16,
    pub pairing_code: Option<String>,
    pub protocol_version: u16,
}

/// mDNS discovery configuration
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub service_type: String,
    pub service_name: String,
    pub port: u16,
    pub browse_timeout: Duration,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            service_type: "_continuum._tcp.local.".to_string(),
            service_name: "continuum".to_string(),
            port: 4433,
            browse_timeout: Duration::from_secs(2),
        }
    }
}

/// mDNS service discovery
pub struct Discovery {
    daemon: ServiceDaemon,
    config: DiscoveryConfig,
}

impl Discovery {
    /// Create a new mDNS discovery service
    pub fn new(config: DiscoveryConfig) -> continuum_core::ContinuumResult<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| continuum_core::ContinuumError::Internal(e.to_string()))?;
        Ok(Self { daemon, config })
    }

    /// Advertise this machine on the local network
    pub fn advertise(
        &self,
        machine_id: &str,
        display_name: &str,
        pairing_code: Option<&str>,
    ) -> continuum_core::ContinuumResult<()> {
        let service_type = &self.config.service_type;
        let instance_name = &self.config.service_name;
        let host_name = &format!("{}.local.", display_name);
        let port = self.config.port;

        let mut properties: Vec<(&str, &str)> = Vec::new();
        properties.push(("machine_id", machine_id));
        properties.push(("display_name", display_name));
        properties.push(("protocol_version", "1"));

        if let Some(code) = pairing_code {
            properties.push(("pairing_code", code));
        }

        let service_info = ServiceInfo::new(
            service_type,
            instance_name,
            host_name,
            "",
            port,
            &properties[..],
        )
        .map_err(|e| continuum_core::ContinuumError::Internal(e.to_string()))?;

        let _receiver = self
            .daemon
            .register(service_info)
            .map_err(|e| continuum_core::ContinuumError::Internal(e.to_string()))?;

        Ok(())
    }

    /// Browse for available peers
    pub fn browse(&self) -> continuum_core::ContinuumResult<Vec<DiscoveredPeer>> {
        let receiver = self
            .daemon
            .browse(&self.config.service_type)
            .map_err(|e| continuum_core::ContinuumError::Internal(e.to_string()))?;

        let mut peers = Vec::new();
        let deadline = std::time::Instant::now() + self.config.browse_timeout;

        while std::time::Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    if let ServiceEvent::ServiceResolved(info) = event {
                        let props: HashMap<String, String> = info
                            .get_properties()
                            .iter()
                            .filter_map(|p| {
                                let key = p.key().to_string();
                                let val = p.val().map(|v| String::from_utf8_lossy(v).to_string())?;
                                Some((key, val))
                            })
                            .collect();

                        if let Some(machine_id) = props.get("machine_id") {
                            peers.push(DiscoveredPeer {
                                machine_id: machine_id.clone(),
                                display_name: props
                                    .get("display_name")
                                    .cloned()
                                    .unwrap_or_default(),
                                address: info
                                    .get_addresses()
                                    .iter()
                                    .next()
                                    .map(|a| a.to_string())
                                    .unwrap_or_default(),
                                port: info.get_port(),
                                pairing_code: props.get("pairing_code").cloned(),
                                protocol_version: props
                                    .get("protocol_version")
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(1),
                            });
                        }
                    }
                }
                Err(_) => break,
            }
        }

        Ok(peers)
    }

    /// Shutdown the discovery service
    pub fn shutdown(&self) -> continuum_core::ContinuumResult<()> {
        self.daemon
            .shutdown()
            .map(|_receiver| ())
            .map_err(|e| continuum_core::ContinuumError::Internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_config_default() {
        let config = DiscoveryConfig::default();
        assert_eq!(config.service_type, "_continuum._tcp.local.");
        assert_eq!(config.port, 4433);
    }

    #[test]
    fn test_discovered_peer() {
        let peer = DiscoveredPeer {
            machine_id: "test-id".to_string(),
            display_name: "Test Machine".to_string(),
            address: "192.168.1.1".to_string(),
            port: 4433,
            pairing_code: Some("abc123".to_string()),
            protocol_version: 1,
        };
        assert_eq!(peer.machine_id, "test-id");
        assert_eq!(peer.port, 4433);
    }
}

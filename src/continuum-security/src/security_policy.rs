use serde::{Deserialize, Serialize};

/// Security levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityLevel {
    Low,
    Medium,
    High,
    Maximum,
}

/// Security policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPolicy {
    pub level: SecurityLevel,
    pub require_pq: bool,
    pub require_hybrid: bool,
    pub min_key_size: u32,
    pub allowed_algorithms: Vec<String>,
    pub audit_enabled: bool,
    pub session_timeout_secs: u64,
    pub max_failed_attempts: u32,
    pub require_sas_verification: bool,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            level: SecurityLevel::High,
            require_pq: false,
            require_hybrid: false,
            min_key_size: 256,
            allowed_algorithms: vec!["X25519".to_string(), "X25519Kyber768".to_string()],
            audit_enabled: true,
            session_timeout_secs: 3600,
            max_failed_attempts: 5,
            require_sas_verification: true,
        }
    }
}

/// Policy enforcement
pub struct PolicyEnforcement {
    policy: SecurityPolicy,
}

impl PolicyEnforcement {
    pub fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }

    /// Check if an algorithm is allowed
    pub fn is_algorithm_allowed(&self, algorithm: &str) -> bool {
        self.policy.allowed_algorithms.contains(&algorithm.to_string())
    }

    /// Check if post-quantum is required
    pub fn requires_pq(&self) -> bool {
        self.policy.require_pq || self.policy.level == SecurityLevel::Maximum
    }

    /// Check if SAS verification is required
    pub fn requires_sas(&self) -> bool {
        self.policy.require_sas_verification
    }

    /// Get session timeout
    pub fn session_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.policy.session_timeout_secs)
    }

    /// Get max failed attempts
    pub fn max_failed_attempts(&self) -> u32 {
        self.policy.max_failed_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = SecurityPolicy::default();
        assert_eq!(policy.level, SecurityLevel::High);
        assert!(policy.audit_enabled);
    }

    #[test]
    fn test_policy_enforcement() {
        let policy = SecurityPolicy::default();
        let enforcement = PolicyEnforcement::new(policy);
        
        assert!(enforcement.is_algorithm_allowed("X25519"));
        assert!(!enforcement.is_algorithm_allowed("RSA"));
        assert!(enforcement.requires_sas());
    }

    #[test]
    fn test_maximum_requires_pq() {
        let policy = SecurityPolicy {
            level: SecurityLevel::Maximum,
            ..Default::default()
        };
        let enforcement = PolicyEnforcement::new(policy);
        assert!(enforcement.requires_pq());
    }
}

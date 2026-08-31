use serde::{Deserialize, Serialize};

/// Version compatibility entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionCompatibility {
    pub client_version: String,
    pub server_version: String,
    pub compatible: bool,
    pub notes: String,
}

/// Compatibility matrix for version testing
pub struct CompatibilityMatrix {
    entries: Vec<VersionCompatibility>,
}

impl CompatibilityMatrix {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a compatibility entry
    pub fn add_entry(&mut self, client: &str, server: &str, compatible: bool, notes: &str) {
        self.entries.push(VersionCompatibility {
            client_version: client.to_string(),
            server_version: server.to_string(),
            compatible,
            notes: notes.to_string(),
        });
    }

    /// Check if two versions are compatible
    pub fn check(&self, client: &str, server: &str) -> Option<bool> {
        self.entries
            .iter()
            .find(|e| e.client_version == client && e.server_version == server)
            .map(|e| e.compatible)
    }

    /// Get all entries
    pub fn entries(&self) -> &[VersionCompatibility] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compatibility_matrix() {
        let mut matrix = CompatibilityMatrix::new();
        matrix.add_entry("1.0.0", "1.0.0", true, "Same version");
        matrix.add_entry("1.0.0", "1.1.0", true, "Backward compatible");
        matrix.add_entry("1.1.0", "1.0.0", false, "Forward incompatible");

        assert_eq!(matrix.check("1.0.0", "1.0.0"), Some(true));
        assert_eq!(matrix.check("1.1.0", "1.0.0"), Some(false));
    }
}

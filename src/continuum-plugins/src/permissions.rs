use serde::{Deserialize, Serialize};

/// Plugin permission levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionLevel {
    None,
    ReadOnly,
    ReadWrite,
    Full,
}

/// Plugin permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginPermissions {
    pub level: PermissionLevel,
    pub allow_file_access: bool,
    pub allow_network: bool,
    pub allow_ui: bool,
}

impl Default for PluginPermissions {
    fn default() -> Self {
        Self {
            level: PermissionLevel::ReadOnly,
            allow_file_access: false,
            allow_network: false,
            allow_ui: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_permissions() {
        let perms = PluginPermissions::default();
        assert_eq!(perms.level, PermissionLevel::ReadOnly);
        assert!(!perms.allow_file_access);
    }
}

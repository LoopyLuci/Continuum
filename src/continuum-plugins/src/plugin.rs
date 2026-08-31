use serde::{Deserialize, Serialize};

/// Plugin capability types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginCapability {
    FrameProcessing,
    InputProcessing,
    SessionManagement,
    UiExtension,
    NetworkExtension,
}

/// Plugin metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<PluginCapability>,
}

/// Plugin context (passed to plugins on init)
#[derive(Debug, Clone)]
pub struct PluginContext {
    pub config_path: String,
    pub data_path: String,
}

/// Plugin lifecycle trait
pub trait Plugin: Send + Sync {
    /// Get plugin metadata
    fn metadata(&self) -> &PluginMetadata;

    /// Initialize the plugin
    fn init(&mut self, ctx: &PluginContext) -> Result<(), PluginError>;

    /// Shutdown the plugin
    fn shutdown(&mut self) -> Result<(), PluginError>;

    /// Handle a frame event (for frame processing plugins)
    fn on_frame(&mut self, _frame: &[u8]) -> Result<Vec<u8>, PluginError> {
        Ok(_frame.to_vec())
    }

    /// Handle a session event
    fn on_session_event(&mut self, _event: &str) -> Result<(), PluginError> {
        Ok(())
    }

    /// Handle an input event
    fn on_input(&mut self, _event: &str) -> Result<(), PluginError> {
        Ok(())
    }
}

/// Plugin errors
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Initialization failed: {0}")]
    InitFailed(String),
    #[error("Shutdown failed: {0}")]
    ShutdownFailed(String),
    #[error("Processing error: {0}")]
    ProcessingError(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
}

/// Placeholder plugin implementation
pub struct PlaceholderPlugin {
    metadata: PluginMetadata,
}

impl PlaceholderPlugin {
    pub fn new() -> Self {
        Self {
            metadata: PluginMetadata {
                name: "placeholder".to_string(),
                version: "0.1.0".to_string(),
                author: "Continuum".to_string(),
                description: "A placeholder plugin".to_string(),
                capabilities: vec![PluginCapability::FrameProcessing],
            },
        }
    }
}

impl Plugin for PlaceholderPlugin {
    fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    fn init(&mut self, _ctx: &PluginContext) -> Result<(), PluginError> {
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder_plugin() {
        let mut plugin = PlaceholderPlugin::new();
        assert_eq!(plugin.metadata().name, "placeholder");
        
        let ctx = PluginContext {
            config_path: "/tmp/config".to_string(),
            data_path: "/tmp/data".to_string(),
        };
        assert!(plugin.init(&ctx).is_ok());
        assert!(plugin.shutdown().is_ok());
    }
}

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::plugin::{Plugin, PluginError};

/// Plugin loader
pub struct PluginLoader {
    plugins: Arc<RwLock<HashMap<String, Box<dyn Plugin>>>>,
}

impl PluginLoader {
    pub fn new() -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load a plugin
    pub async fn load(&self, plugin: Box<dyn Plugin>) -> Result<(), PluginError> {
        let name = plugin.metadata().name.clone();
        self.plugins.write().await.insert(name, plugin);
        Ok(())
    }

    /// Unload a plugin
    pub async fn unload(&self, name: &str) -> bool {
        self.plugins.write().await.remove(name).is_some()
    }

    /// Get a plugin by name
    pub async fn get(&self, name: &str) -> Option<String> {
        self.plugins.read().await.get(name).map(|p| p.metadata().name.clone())
    }

    /// List all loaded plugins
    pub async fn list(&self) -> Vec<String> {
        self.plugins.read().await.keys().cloned().collect()
    }
}

/// Plugin registry
pub struct PluginRegistry {
    loader: PluginLoader,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            loader: PluginLoader::new(),
        }
    }

    /// Get loader
    pub fn loader(&self) -> &PluginLoader {
        &self.loader
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PlaceholderPlugin;

    #[tokio::test]
    async fn test_plugin_loader() {
        let loader = PluginLoader::new();
        let plugin = Box::new(PlaceholderPlugin::new());
        loader.load(plugin).await.unwrap();
        assert!(loader.get("placeholder").await.is_some());
    }
}

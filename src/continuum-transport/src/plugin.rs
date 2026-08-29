use crate::types::FrameSemantics;

/// Trait for native plugins compiled into the server
pub trait ServerPlugin: Send + Sync {
    fn name(&self) -> &str;

    /// Called after a frame is captured and encoded, before encryption and send.
    /// Can modify the JPEG data in-place or just observe it.
    fn on_frame_encoded(&self, _frame_data: &mut Vec<u8>, _semantics: &mut FrameSemantics) {}

    /// Called when a session starts
    fn on_session_start(&self, _session_id: &str) {}

    /// Called when a session ends
    fn on_session_end(&self, _session_id: &str) {}
}

/// Built-in demo plugin: stamps a frame counter watermark
pub struct FrameCounterPlugin {
    pub frame_count: std::sync::atomic::AtomicU64,
}

impl Default for FrameCounterPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameCounterPlugin {
    pub fn new() -> Self {
        Self {
            frame_count: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl ServerPlugin for FrameCounterPlugin {
    fn name(&self) -> &str {
        "frame-counter"
    }

    fn on_frame_encoded(&self, _frame_data: &mut Vec<u8>, _semantics: &mut FrameSemantics) {
        let count = self
            .frame_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        if count % 100 == 0 {
            tracing::info!(
                plugin = "frame-counter",
                frames = count,
                "Plugin hook fired"
            );
        }
    }

    fn on_session_start(&self, _session_id: &str) {
        self.frame_count
            .store(0, std::sync::atomic::Ordering::Relaxed);
        tracing::info!(plugin = "frame-counter", "Session started, counter reset");
    }

    fn on_session_end(&self, session_id: &str) {
        let total = self.frame_count.load(std::sync::atomic::Ordering::Relaxed);
        tracing::info!(
            plugin = "frame-counter",
            session = session_id,
            total_frames = total,
            "Session ended"
        );
    }
}

/// Registry that holds all loaded native plugins
pub struct NativePluginRegistry {
    plugins: Vec<Box<dyn ServerPlugin>>,
}

impl NativePluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register(&mut self, plugin: Box<dyn ServerPlugin>) {
        tracing::info!(plugin = plugin.name(), "Registering native plugin");
        self.plugins.push(plugin);
    }

    pub fn on_frame_encoded(&self, frame_data: &mut Vec<u8>, semantics: &mut FrameSemantics) {
        for plugin in &self.plugins {
            plugin.on_frame_encoded(frame_data, semantics);
        }
    }

    pub fn on_session_start(&self, session_id: &str) {
        for plugin in &self.plugins {
            plugin.on_session_start(session_id);
        }
    }

    pub fn on_session_end(&self, session_id: &str) {
        for plugin in &self.plugins {
            plugin.on_session_end(session_id);
        }
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }
}

impl Default for NativePluginRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(FrameCounterPlugin::new()));
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_registry_default_has_frame_counter() {
        let registry = NativePluginRegistry::default();
        assert_eq!(registry.plugin_count(), 1);
        assert_eq!(registry.plugins[0].name(), "frame-counter");
    }

    #[test]
    fn test_frame_counter_plugin_increments() {
        let plugin = FrameCounterPlugin::new();
        let mut data = vec![1u8, 2, 3];
        let mut sem = FrameSemantics::default();

        plugin.on_frame_encoded(&mut data, &mut sem);
        assert_eq!(
            plugin
                .frame_count
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );

        plugin.on_frame_encoded(&mut data, &mut sem);
        assert_eq!(
            plugin
                .frame_count
                .load(std::sync::atomic::Ordering::Relaxed),
            2
        );
    }

    #[test]
    fn test_session_start_resets_counter() {
        let plugin = FrameCounterPlugin::new();
        let mut data = vec![1u8];
        let mut sem = FrameSemantics::default();

        plugin.on_frame_encoded(&mut data, &mut sem);
        plugin.on_frame_encoded(&mut data, &mut sem);
        assert_eq!(
            plugin
                .frame_count
                .load(std::sync::atomic::Ordering::Relaxed),
            2
        );

        plugin.on_session_start("test-session");
        assert_eq!(
            plugin
                .frame_count
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn test_registry_calls_all_plugins() {
        let mut registry = NativePluginRegistry::new();
        registry.register(Box::new(FrameCounterPlugin::new()));
        registry.register(Box::new(FrameCounterPlugin::new()));

        let mut data = vec![1u8];
        let mut sem = FrameSemantics::default();
        registry.on_frame_encoded(&mut data, &mut sem);

        assert_eq!(registry.plugin_count(), 2);
    }
}

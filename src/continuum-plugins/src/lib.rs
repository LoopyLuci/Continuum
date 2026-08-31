pub mod plugin;
pub mod loader;
pub mod permissions;

pub use plugin::{Plugin, PluginMetadata, PluginCapability, PluginContext};
pub use loader::{PluginLoader, PluginRegistry};
pub use permissions::{PluginPermissions, PermissionLevel};

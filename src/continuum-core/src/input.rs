use crate::*;
///
/// Implement this to add support for new input methods
/// (touch, gamepad, gestures, etc.).
pub trait InputInjector: Send + Sync {
    /// Apply a remote input event.
    fn apply(&mut self, event: &RemoteInputEvent) -> ContinuumResult<()>;

    /// Get the injector name.
    fn name(&self) -> &str;

    /// Check if the injector is available on the current platform.
    fn is_available(&self) -> bool;
}

/// A remote input event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteInputEvent {
    pub action: InputAction,
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub button: Option<MouseButton>,
    pub key: Option<String>,
    pub modifiers: Option<ModifierKeys>,
    pub scroll_x: Option<f32>,
    pub scroll_y: Option<f32>,
    pub monitor_id: Option<u32>,
}

/// Input action types.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum InputAction {
    MouseMove,
    MouseDown,
    MouseUp,
    MouseDoubleClick,
    KeyDown,
    KeyUp,
    Scroll,
    Touch,
}

/// Mouse buttons.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// Modifier keys.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ModifierKeys {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

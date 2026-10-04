use crate::*;

/// Implement this to add support for new input methods
/// (touch, gamepad, gestures, QMP/QEMU, etc.).
///
/// Mirrors `continuum_transport::types::InputInjector` semantics. The two
/// definitions of the event vocabulary used to disagree — this crate had
/// `x: Option<u32>` where transport had `Option<i32>`, and a `Scroll`/`Touch`
/// pair where transport had `MouseScroll`/`KeyPress`. A signed coordinate is
/// not an optional nicety: a QMP tablet's absolute axis is signed, and a
/// pointer can legitimately leave the framebuffer. They are now identical,
/// and `continuum_transport::input_qmp` holds the `From` conversion.
///
/// Two implementations exist and both are live:
///   * `continuum_transport::input_qmp::QmpInputInjector` — a QEMU guest,
///     keys via HMP `sendkey`, pointer via `input-send-event`.
///   * `continuum_transport::input::InputInjector` — the host desktop
///     (`enigo`/`CGEvent`/`xdo`). This is the default; QMP is selected per-VM.
pub trait InputInjector: Send + Sync {
    /// Apply a remote input event.
    fn apply(&mut self, event: &RemoteInputEvent) -> ContinuumResult<()>;

    /// Get the injector name.
    fn name(&self) -> &str;

    /// Check if the injector is available on the current platform.
    fn is_available(&self) -> bool;
}

/// A remote input event.
///
/// Identical to `continuum_transport::types::RemoteInputEvent`, including the
/// `Option` fields: a `MouseMove` carries no `key`, and requiring an empty
/// `""` sentinel is how "no key" turns into a spurious keystroke.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteInputEvent {
    pub action: InputAction,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub button: Option<MouseButton>,
    pub key: Option<String>,
    pub modifiers: Option<ModifierKeys>,
    pub scroll_x: Option<f32>,
    pub scroll_y: Option<f32>,
    pub monitor_id: Option<u32>,
}

/// Input action types.
///
/// Mirrors `continuum_transport::types::InputAction`, serde renames included,
/// so an event serialised by either crate deserialises in the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputAction {
    #[serde(rename = "mouse_move")]
    MouseMove,
    #[serde(rename = "mouse_click")]
    MouseClick,
    #[serde(rename = "mouse_down")]
    MouseDown,
    #[serde(rename = "mouse_up")]
    MouseUp,
    #[serde(rename = "mouse_scroll")]
    MouseScroll,
    #[serde(rename = "key_press")]
    KeyPress,
    #[serde(rename = "key_down")]
    KeyDown,
    #[serde(rename = "key_up")]
    KeyUp,
}

/// Mouse buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MouseButton {
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "right")]
    Right,
    #[serde(rename = "middle")]
    Middle,
}

/// Modifier keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModifierKeys {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_action_wire_names_match_transport() {
        // These strings are the QUIC intent-stream encoding. If they drift, a
        // client built against the old table sends "scroll" and the server
        // deserialises nothing at all.
        let cases = [
            (InputAction::MouseMove, "\"mouse_move\""),
            (InputAction::MouseClick, "\"mouse_click\""),
            (InputAction::MouseDown, "\"mouse_down\""),
            (InputAction::MouseUp, "\"mouse_up\""),
            (InputAction::MouseScroll, "\"mouse_scroll\""),
            (InputAction::KeyPress, "\"key_press\""),
            (InputAction::KeyDown, "\"key_down\""),
            (InputAction::KeyUp, "\"key_up\""),
        ];
        for (action, expected) in cases {
            assert_eq!(
                serde_json::to_string(&action).unwrap(),
                expected,
                "wire name drift for {action:?}"
            );
        }
    }

    #[test]
    fn test_mouse_button_wire_names() {
        assert_eq!(serde_json::to_string(&MouseButton::Left).unwrap(), "\"left\"");
        assert_eq!(
            serde_json::to_string(&MouseButton::Right).unwrap(),
            "\"right\""
        );
        assert_eq!(
            serde_json::to_string(&MouseButton::Middle).unwrap(),
            "\"middle\""
        );
    }

    #[test]
    fn test_coordinates_are_signed() {
        let event = RemoteInputEvent {
            action: InputAction::MouseMove,
            x: Some(-1),
            y: Some(-40),
            button: None,
            key: None,
            modifiers: None,
            scroll_x: None,
            scroll_y: None,
            monitor_id: Some(0),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"x\":-1"), "got {json}");
        let back: RemoteInputEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.x, Some(-1));
        assert_eq!(back.y, Some(-40));
    }

    #[test]
    fn test_absent_fields_round_trip_as_none() {
        let event = RemoteInputEvent {
            action: InputAction::MouseMove,
            x: None,
            y: None,
            button: None,
            key: None,
            modifiers: None,
            scroll_x: None,
            scroll_y: None,
            monitor_id: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "{\"action\":\"mouse_move\",\"x\":null,\"y\":null,\"button\":null,\"key\":null,\"modifiers\":null,\"scroll_x\":null,\"scroll_y\":null,\"monitor_id\":null}");
        let back: RemoteInputEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.action, InputAction::MouseMove);
        assert!(back.key.is_none());
    }

    #[test]
    fn test_modifier_keys_default_is_all_false() {
        let mods = ModifierKeys::default();
        assert!(!mods.ctrl && !mods.alt && !mods.shift && !mods.super_key);
    }
}
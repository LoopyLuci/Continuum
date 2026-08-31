use serde::{Deserialize, Serialize};

/// Mobile client configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileClientConfig {
    pub pairing_code: String,
    pub server_address: String,
    pub enable_audio: bool,
    pub enable_clipboard: bool,
    pub touch_input_mode: TouchInputMode,
}

/// Touch input modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TouchInputMode {
    Touchpad,
    DirectTouch,
    Gamepad,
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

/// Input event from mobile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { x: f32, y: f32 },
    MouseDown { button: u8, x: f32, y: f32 },
    MouseUp { button: u8, x: f32, y: f32 },
    KeyDown { key_code: u32 },
    KeyUp { key_code: u32 },
    Scroll { dx: f32, dy: f32 },
}

/// Mobile client (FFI bridge)
pub struct MobileClient {
    config: MobileClientConfig,
    state: ConnectionState,
}

impl MobileClient {
    pub fn new(config: MobileClientConfig) -> Self {
        Self {
            config,
            state: ConnectionState::Disconnected,
        }
    }

    /// Connect to server
    pub fn connect(&mut self) -> bool {
        self.state = ConnectionState::Connecting;
        // Placeholder: FFI bridge would connect here
        self.state = ConnectionState::Connected;
        true
    }

    /// Disconnect
    pub fn disconnect(&mut self) {
        self.state = ConnectionState::Disconnected;
    }

    /// Send input event
    pub fn send_input(&self, _event: &InputEvent) -> bool {
        self.state == ConnectionState::Connected
    }

    /// Get connection state
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// Get config
    pub fn config(&self) -> &MobileClientConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mobile_client() {
        let config = MobileClientConfig {
            pairing_code: "test123".to_string(),
            server_address: "192.168.1.1:4433".to_string(),
            enable_audio: true,
            enable_clipboard: true,
            touch_input_mode: TouchInputMode::Touchpad,
        };
        let mut client = MobileClient::new(config);
        assert_eq!(client.state(), ConnectionState::Disconnected);
        assert!(client.connect());
        assert_eq!(client.state(), ConnectionState::Connected);
    }
}

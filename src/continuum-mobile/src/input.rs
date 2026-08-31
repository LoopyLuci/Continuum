use serde::{Deserialize, Serialize};

/// Touch input event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TouchInput {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub timestamp: u64,
}

/// Gesture input event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GestureInput {
    Tap { x: f32, y: f32 },
    DoubleTap { x: f32, y: f32 },
    Pinch { scale: f32, velocity: f32 },
    Rotate { angle: f32, velocity: f32 },
    Pan { dx: f32, dy: f32, velocity: f32 },
    Swipe { direction: SwipeDirection, velocity: f32 },
}

/// Swipe direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

/// Accelerometer input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccelerometerInput {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_touch_input() {
        let touch = TouchInput {
            id: 1,
            x: 100.0,
            y: 200.0,
            pressure: 0.5,
            timestamp: 0,
        };
        assert_eq!(touch.x, 100.0);
    }

    #[test]
    fn test_gesture_input() {
        let gesture = GestureInput::Tap { x: 50.0, y: 50.0 };
        match gesture {
            GestureInput::Tap { x, y } => {
                assert_eq!(x, 50.0);
                assert_eq!(y, 50.0);
            }
            _ => panic!("Wrong gesture type"),
        }
    }
}

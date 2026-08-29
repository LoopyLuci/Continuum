use crate::types::{InputAction, ModifierKeys, MouseButton, RemoteInputEvent};
use anyhow::Result;
use quinn::Connection;
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatagramInput {
    pub sequence: u64,
    pub action: InputAction,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub button: Option<MouseButton>,
    pub key: Option<String>,
    pub modifiers: Option<ModifierKeys>,
    pub scroll_x: Option<f32>,
    pub scroll_y: Option<f32>,
    pub monitor_id: Option<u32>,
    pub timestamp: i64,
}

pub struct DatagramHandler {
    next_seq: u64,
    last_process_time: Instant,
    pending_count: u64,
}

impl DatagramHandler {
    pub fn new() -> Self {
        Self {
            next_seq: 0,
            last_process_time: Instant::now(),
            pending_count: 0,
        }
    }

    pub fn send_input(&mut self, conn: &Connection, event: RemoteInputEvent) -> Result<()> {
        let datagram = DatagramInput {
            sequence: self.next_seq,
            action: event.action,
            x: event.x,
            y: event.y,
            button: event.button,
            key: event.key,
            modifiers: event.modifiers,
            scroll_x: event.scroll_x,
            scroll_y: event.scroll_y,
            monitor_id: event.monitor_id,
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        self.next_seq += 1;

        let payload = serde_json::to_vec(&datagram)?;
        conn.send_datagram(bytes::Bytes::from(payload))?;
        self.pending_count += 1;
        Ok(())
    }

    pub fn process_datagram(&mut self, data: &[u8]) -> Result<Option<RemoteInputEvent>> {
        let input: DatagramInput = match serde_json::from_slice(data) {
            Ok(input) => input,
            Err(_) => return Ok(None),
        };

        if input.sequence > self.next_seq {
            tracing::warn!(
                expected = self.next_seq,
                got = input.sequence,
                "Out-of-order datagram input"
            );
        }
        self.next_seq = self.next_seq.max(input.sequence + 1);

        let event = RemoteInputEvent {
            action: input.action,
            x: input.x,
            y: input.y,
            button: input.button,
            key: input.key,
            modifiers: input.modifiers,
            scroll_x: input.scroll_x,
            scroll_y: input.scroll_y,
            monitor_id: input.monitor_id,
        };

        self.last_process_time = Instant::now();
        Ok(Some(event))
    }

    pub fn pending_count(&self) -> u64 {
        self.pending_count
    }

    pub fn throughput_per_sec(&self) -> f64 {
        let elapsed = self.last_process_time.elapsed().as_secs_f64().max(0.001);
        self.pending_count as f64 / elapsed
    }
}

impl Default for DatagramHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_datagram_invalid_data() {
        let mut handler = DatagramHandler::new();
        let result = handler.process_datagram(&[0u8; 10]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_process_datagram_empty() {
        let mut handler = DatagramHandler::new();
        let result = handler.process_datagram(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_process_datagram_valid_input_event() {
        let mut handler = DatagramHandler::new();
        let event = RemoteInputEvent {
            action: InputAction::MouseMove,
            x: Some(100),
            y: Some(200),
            button: None,
            key: None,
            modifiers: None,
            scroll_x: None,
            scroll_y: None,
            monitor_id: None,
        };
        let datagram = DatagramInput {
            sequence: 0,
            action: event.action,
            x: event.x,
            y: event.y,
            button: event.button,
            key: event.key,
            modifiers: event.modifiers,
            scroll_x: event.scroll_x,
            scroll_y: event.scroll_y,
            monitor_id: event.monitor_id,
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        let data = serde_json::to_vec(&datagram).unwrap();
        let result = handler.process_datagram(&data);
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert!(parsed.is_some());
        assert_eq!(parsed.unwrap().action, InputAction::MouseMove);
    }

    #[test]
    fn test_sequence_tracking() {
        let mut handler = DatagramHandler::new();

        let make_datagram = |seq: u64| -> Vec<u8> {
            let datagram = DatagramInput {
                sequence: seq,
                action: InputAction::MouseMove,
                x: Some(10),
                y: Some(20),
                button: None,
                key: None,
                modifiers: None,
                scroll_x: None,
                scroll_y: None,
                monitor_id: None,
                timestamp: chrono::Utc::now().timestamp_micros(),
            };
            serde_json::to_vec(&datagram).unwrap()
        };

        let data = make_datagram(5);
        handler.process_datagram(&data).unwrap();
        assert_eq!(handler.next_seq, 6);

        let data = make_datagram(3);
        handler.process_datagram(&data).unwrap();
        assert_eq!(handler.next_seq, 6);

        let data = make_datagram(10);
        handler.process_datagram(&data).unwrap();
        assert_eq!(handler.next_seq, 11);
    }
}

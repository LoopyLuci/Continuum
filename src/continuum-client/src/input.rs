#![allow(dead_code)]

use continuum_transport::datagram::DatagramHandler;
use continuum_transport::{ControlCommand, InputAction, MouseButton, RemoteInputEvent};
use eframe::egui::{self, PointerButton, Response};
use std::sync::mpsc as std_mpsc;

pub struct InputHandler {
    command_tx: std_mpsc::Sender<ControlCommand>,
    datagram: DatagramHandler,
    last_mouse_pos: Option<(i32, i32)>,
    mouse_down: bool,
    view_only: bool,
    current_monitor: u32,
    monitor_resolution: (u32, u32),
}

impl InputHandler {
    pub fn new(command_tx: std_mpsc::Sender<ControlCommand>, view_only: bool) -> Self {
        Self {
            command_tx,
            datagram: DatagramHandler::new(),
            last_mouse_pos: None,
            mouse_down: false,
            view_only,
            current_monitor: 0,
            monitor_resolution: (1920, 1080),
        }
    }

    pub fn set_monitor(&mut self, id: u32, resolution: (u32, u32)) {
        self.current_monitor = id;
        self.monitor_resolution = resolution;
    }

    pub fn handle_frame_interaction(&mut self, response: &Response, ui: &egui::Ui) {
        if self.view_only {
            return;
        }
        let rect = response.rect;
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }

        let pointer_under = ui.input(|i| i.pointer.latest_pos());
        if let Some(pos) = pointer_under {
            if rect.contains(pos) {
                let x = ((pos.x - rect.left()) / rect.width() * self.monitor_resolution.0 as f32)
                    .round() as i32;
                let y = ((pos.y - rect.top()) / rect.height() * self.monitor_resolution.1 as f32)
                    .round() as i32;
                let cx = x.clamp(0, self.monitor_resolution.0 as i32 - 1);
                let cy = y.clamp(0, self.monitor_resolution.1 as i32 - 1);

                if self.last_mouse_pos != Some((cx, cy)) {
                    self.last_mouse_pos = Some((cx, cy));
                    self.send_input(RemoteInputEvent {
                        action: InputAction::MouseMove,
                        x: Some(cx),
                        y: Some(cy),
                        button: None,
                        key: None,
                        modifiers: None,
                        scroll_x: None,
                        scroll_y: None,
                        monitor_id: Some(self.current_monitor),
                    });
                }
            }
        }

        if response.clicked_by(PointerButton::Primary) {
            self.send_click(MouseButton::Left);
        }
        if response.clicked_by(PointerButton::Secondary) {
            self.send_click(MouseButton::Right);
        }
        if response.clicked_by(PointerButton::Middle) {
            self.send_click(MouseButton::Middle);
        }

        let primary_down = ui.input(|i| i.pointer.button_down(PointerButton::Primary));
        if primary_down && !self.mouse_down && response.hovered() {
            self.mouse_down = true;
            self.send_pos(InputAction::MouseDown, MouseButton::Left);
        } else if !primary_down && self.mouse_down {
            self.mouse_down = false;
            self.send_pos(InputAction::MouseUp, MouseButton::Left);
        }

        let scroll = ui.input(|i| i.raw_scroll_delta);
        if scroll.y.abs() > 0.5 || scroll.x.abs() > 0.5 {
            self.send_input(RemoteInputEvent {
                action: InputAction::MouseScroll,
                x: self.last_mouse_pos.map(|p| p.0),
                y: self.last_mouse_pos.map(|p| p.1),
                button: None,
                key: None,
                modifiers: None,
                scroll_x: Some(scroll.x / 50.0),
                scroll_y: Some(scroll.y / 50.0),
                monitor_id: Some(self.current_monitor),
            });
        }
    }

    fn send_click(&self, button: MouseButton) {
        if let Some((x, y)) = self.last_mouse_pos {
            self.send_input(RemoteInputEvent {
                action: InputAction::MouseClick,
                x: Some(x),
                y: Some(y),
                button: Some(button),
                key: None,
                modifiers: None,
                scroll_x: None,
                scroll_y: None,
                monitor_id: Some(self.current_monitor),
            });
        }
    }

    fn send_pos(&self, action: InputAction, button: MouseButton) {
        if let Some((x, y)) = self.last_mouse_pos {
            self.send_input(RemoteInputEvent {
                action,
                x: Some(x),
                y: Some(y),
                button: Some(button),
                key: None,
                modifiers: None,
                scroll_x: None,
                scroll_y: None,
                monitor_id: Some(self.current_monitor),
            });
        }
    }

    fn send_input(&self, event: RemoteInputEvent) {
        let _ = self.command_tx.send(ControlCommand::Input(event));
    }
}

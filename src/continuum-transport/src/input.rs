use crate::types::{InputAction, ModifierKeys, MouseButton, RemoteInputEvent};
use anyhow::Result;

pub struct InputInjector {
    #[cfg(target_os = "windows")]
    enigo: enigo::Enigo,
    #[cfg(target_os = "macos")]
    event_source: Option<core_graphics::event_source::CGEventSource>,
    #[cfg(target_os = "linux")]
    xdo: Option<libxdo::Xdo>,
}

impl InputInjector {
    pub fn new() -> Result<Self> {
        #[cfg(target_os = "windows")]
        {
            use enigo::Settings;
            match enigo::Enigo::new(&Settings::default()) {
                Ok(enigo) => Ok(Self { enigo }),
                Err(e) => Err(anyhow::anyhow!(
                    "Failed to initialize input injector: {}",
                    e
                )),
            }
        }

        #[cfg(target_os = "macos")]
        {
            let event_source = core_graphics::event_source::CGEventSource::new(
                core_graphics::event_source::CGEventSourceStateID::CombinedSessionState,
            )
            .ok();
            Ok(Self { event_source })
        }

        #[cfg(target_os = "linux")]
        {
            let xdo = libxdo::Xdo::new(None).ok();
            Ok(Self { xdo })
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            Ok(Self {})
        }
    }

    pub fn apply(&mut self, event: &RemoteInputEvent) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            use enigo::{Coordinate, Direction, Keyboard, Mouse};
            match event.action {
                InputAction::MouseMove => {
                    if let (Some(x), Some(y)) = (event.x, event.y) {
                        self.enigo
                            .move_mouse(x, y, Coordinate::Abs)
                            .map_err(|e| anyhow::anyhow!("MouseMove failed: {}", e))?;
                    }
                }
                InputAction::MouseClick => {
                    if let (Some(x), Some(y)) = (event.x, event.y) {
                        self.enigo.move_mouse(x, y, Coordinate::Abs)?;
                    }
                    let button = to_enigo_button(event.button.unwrap_or(MouseButton::Left));
                    self.enigo.button(button, Direction::Click)?;
                }
                InputAction::MouseDown => {
                    if let (Some(x), Some(y)) = (event.x, event.y) {
                        self.enigo.move_mouse(x, y, Coordinate::Abs)?;
                    }
                    let button = to_enigo_button(event.button.unwrap_or(MouseButton::Left));
                    self.enigo.button(button, Direction::Press)?;
                }
                InputAction::MouseUp => {
                    let button = to_enigo_button(event.button.unwrap_or(MouseButton::Left));
                    self.enigo.button(button, Direction::Release)?;
                }
                InputAction::MouseScroll => {
                    let x = event.scroll_x.unwrap_or(0.0) as i32;
                    let y = event.scroll_y.unwrap_or(0.0) as i32;
                    if y != 0 {
                        self.enigo.scroll(y, enigo::Axis::Vertical)?;
                    }
                    if x != 0 {
                        self.enigo.scroll(x, enigo::Axis::Horizontal)?;
                    }
                }
                InputAction::KeyPress => {
                    apply_modifiers(&mut self.enigo, event.modifiers, Direction::Press)?;
                    if let Some(key) = &event.key {
                        let enigo_key = str_to_key(key);
                        self.enigo.key(enigo_key, Direction::Click)?;
                    }
                    apply_modifiers(&mut self.enigo, event.modifiers, Direction::Release)?;
                }
                InputAction::KeyDown => {
                    apply_modifiers(&mut self.enigo, event.modifiers, Direction::Press)?;
                    if let Some(key) = &event.key {
                        let enigo_key = str_to_key(key);
                        self.enigo.key(enigo_key, Direction::Press)?;
                    }
                }
                InputAction::KeyUp => {
                    if let Some(key) = &event.key {
                        let enigo_key = str_to_key(key);
                        self.enigo.key(enigo_key, Direction::Release)?;
                    }
                    apply_modifiers(&mut self.enigo, event.modifiers, Direction::Release)?;
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            use core_graphics::event::{
                CGEvent, CGEventFlags, CGEventMouseSubtype, CGMouseButton, CGPoint,
            };
            use core_graphics::event_source::CGEventSource;

            let source = match &self.event_source {
                Some(source) => source,
                None => {
                    tracing::debug!("Input event dropped: no macOS event source available");
                    return Ok(());
                }
            };

            match event.action {
                InputAction::MouseMove => {
                    if let (Some(x), Some(y)) = (event.x, event.y) {
                        let point = CGPoint::new(x as f64, y as f64);
                        let ev = CGEvent::new_mouse_event(
                            source,
                            core_graphics::event::CGEventType::MouseMoved,
                            point,
                            CGMouseButton::Left,
                        )
                        .map_err(|e| anyhow::anyhow!("macOS mouse event create failed: {}", e))?;
                        ev.post(core_graphics::event::CGEventTapLocation::HID);
                    }
                }
                InputAction::MouseClick => {
                    if let (Some(x), Some(y)) = (event.x, event.y) {
                        let point = CGPoint::new(x as f64, y as f64);
                        let button = match event.button.unwrap_or(MouseButton::Left) {
                            MouseButton::Left => CGMouseButton::Left,
                            MouseButton::Right => CGMouseButton::Right,
                            MouseButton::Middle => CGMouseButton::Center,
                        };
                        let down = CGEvent::new_mouse_event(
                            source,
                            core_graphics::event::CGEventType::LeftMouseDown,
                            point,
                            button,
                        )
                        .map_err(|e| anyhow::anyhow!("macOS mouse down failed: {}", e))?;
                        let up = CGEvent::new_mouse_event(
                            source,
                            core_graphics::event::CGEventType::LeftMouseUp,
                            point,
                            button,
                        )
                        .map_err(|e| anyhow::anyhow!("macOS mouse up failed: {}", e))?;
                        down.post(core_graphics::event::CGEventTapLocation::HID);
                        up.post(core_graphics::event::CGEventTapLocation::HID);
                    }
                }
                InputAction::MouseDown => {
                    if let (Some(x), Some(y)) = (event.x, event.y) {
                        let point = CGPoint::new(x as f64, y as f64);
                        let button = match event.button.unwrap_or(MouseButton::Left) {
                            MouseButton::Left => CGMouseButton::Left,
                            MouseButton::Right => CGMouseButton::Right,
                            MouseButton::Middle => CGMouseButton::Center,
                        };
                        let ev = CGEvent::new_mouse_event(
                            source,
                            core_graphics::event::CGEventType::LeftMouseDown,
                            point,
                            button,
                        )
                        .map_err(|e| anyhow::anyhow!("macOS mouse down failed: {}", e))?;
                        ev.post(core_graphics::event::CGEventTapLocation::HID);
                    }
                }
                InputAction::MouseUp => {
                    let button = match event.button.unwrap_or(MouseButton::Left) {
                        MouseButton::Left => CGMouseButton::Left,
                        MouseButton::Right => CGMouseButton::Right,
                        MouseButton::Middle => CGMouseButton::Center,
                    };
                    let point = CGPoint::new(0.0, 0.0);
                    let ev = CGEvent::new_mouse_event(
                        source,
                        core_graphics::event::CGEventType::LeftMouseUp,
                        point,
                        button,
                    )
                    .map_err(|e| anyhow::anyhow!("macOS mouse up failed: {}", e))?;
                    ev.post(core_graphics::event::CGEventTapLocation::HID);
                }
                InputAction::MouseScroll => {
                    let y = event.scroll_y.unwrap_or(0.0) as i32;
                    let scroll = CGEvent::new_scroll_event(
                        source,
                        core_graphics::event::CGScrollEventUnit::Line,
                        y,
                    )
                    .map_err(|e| anyhow::anyhow!("macOS scroll failed: {}", e))?;
                    scroll.post(core_graphics::event::CGEventTapLocation::HID);
                }
                InputAction::KeyPress | InputAction::KeyDown | InputAction::KeyUp => {
                    let key = event.key.clone().unwrap_or_default();
                    let keycode = macos_keycode(&key);
                    let direction = match event.action {
                        InputAction::KeyDown => core_graphics::event::CGEventFlag::None,
                        InputAction::KeyUp => core_graphics::event::CGEventFlag::None,
                        InputAction::KeyPress => {
                            let down = CGEvent::new_keyboard_event(source, keycode, true)
                                .map_err(|e| anyhow::anyhow!("macOS key down failed: {}", e))?;
                            down.post(core_graphics::event::CGEventTapLocation::HID);
                            core_graphics::event::CGEventFlag::None
                        }
                        _ => core_graphics::event::CGEventFlag::None,
                    };

                    if matches!(event.action, InputAction::KeyUp | InputAction::KeyPress) {
                        let up = CGEvent::new_keyboard_event(source, keycode, false)
                            .map_err(|e| anyhow::anyhow!("macOS key up failed: {}", e))?;
                        up.post(core_graphics::event::CGEventTapLocation::HID);
                    }

                    let _ = direction;
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            let xdo = match &self.xdo {
                Some(xdo) => xdo,
                None => {
                    tracing::debug!("Input event dropped: no Linux xdo instance available");
                    return Ok(());
                }
            };

            match event.action {
                InputAction::MouseMove => {
                    if let (Some(x), Some(y)) = (event.x, event.y) {
                        xdo.move_mouse(x, y, 0)
                            .map_err(|e| anyhow::anyhow!("xdo move failed: {}", e))?;
                    }
                }
                InputAction::MouseClick => {
                    if let (Some(x), Some(y)) = (event.x, event.y) {
                        xdo.move_mouse(x, y, 0)
                            .map_err(|e| anyhow::anyhow!("xdo move failed: {}", e))?;
                    }
                    let button = match event.button.unwrap_or(MouseButton::Left) {
                        MouseButton::Left => 1,
                        MouseButton::Middle => 2,
                        MouseButton::Right => 3,
                    };
                    xdo.mouse_down(button)
                        .map_err(|e| anyhow::anyhow!("xdo mouse down failed: {}", e))?;
                    xdo.mouse_up(button)
                        .map_err(|e| anyhow::anyhow!("xdo mouse up failed: {}", e))?;
                }
                InputAction::MouseDown => {
                    if let (Some(x), Some(y)) = (event.x, event.y) {
                        xdo.move_mouse(x, y, 0)
                            .map_err(|e| anyhow::anyhow!("xdo move failed: {}", e))?;
                    }
                    let button = match event.button.unwrap_or(MouseButton::Left) {
                        MouseButton::Left => 1,
                        MouseButton::Middle => 2,
                        MouseButton::Right => 3,
                    };
                    xdo.mouse_down(button)
                        .map_err(|e| anyhow::anyhow!("xdo mouse down failed: {}", e))?;
                }
                InputAction::MouseUp => {
                    let button = match event.button.unwrap_or(MouseButton::Left) {
                        MouseButton::Left => 1,
                        MouseButton::Middle => 2,
                        MouseButton::Right => 3,
                    };
                    xdo.mouse_up(button)
                        .map_err(|e| anyhow::anyhow!("xdo mouse up failed: {}", e))?;
                }
                InputAction::MouseScroll => {
                    let y = event.scroll_y.unwrap_or(0.0) as i32;
                    if y > 0 {
                        xdo.mouse_down(4)
                            .map_err(|e| anyhow::anyhow!("xdo scroll down failed: {}", e))?;
                        xdo.mouse_up(4)
                            .map_err(|e| anyhow::anyhow!("xdo scroll up failed: {}", e))?;
                    } else if y < 0 {
                        xdo.mouse_down(5)
                            .map_err(|e| anyhow::anyhow!("xdo scroll up failed: {}", e))?;
                        xdo.mouse_up(5)
                            .map_err(|e| anyhow::anyhow!("xdo scroll down failed: {}", e))?;
                    }
                }
                InputAction::KeyPress | InputAction::KeyDown | InputAction::KeyUp => {
                    let key = event.key.clone().unwrap_or_default();
                    let keyseq = linux_key_sequence(&key);
                    if event.action != InputAction::KeyUp {
                        xdo.keysequence_down(keyseq.as_str())
                            .map_err(|e| anyhow::anyhow!("xdo key down failed: {}", e))?;
                    }
                    if event.action != InputAction::KeyDown {
                        xdo.keysequence_up(keyseq.as_str())
                            .map_err(|e| anyhow::anyhow!("xdo key up failed: {}", e))?;
                    }
                }
            }
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            tracing::debug!(action = ?event.action, "Input event (no-op on unsupported platform)");
        }

        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn to_enigo_button(button: MouseButton) -> enigo::Button {
    match button {
        MouseButton::Left => enigo::Button::Left,
        MouseButton::Right => enigo::Button::Right,
        MouseButton::Middle => enigo::Button::Middle,
    }
}

#[cfg(target_os = "windows")]
fn apply_modifiers(
    enigo: &mut enigo::Enigo,
    modifiers: Option<ModifierKeys>,
    direction: enigo::Direction,
) -> Result<()> {
    use enigo::{Key, Keyboard};
    if let Some(mods) = modifiers {
        if mods.ctrl {
            enigo.key(Key::Control, direction)?;
        }
        if mods.alt {
            enigo.key(Key::Alt, direction)?;
        }
        if mods.shift {
            enigo.key(Key::Shift, direction)?;
        }
        if mods.super_key {
            enigo.key(Key::Meta, direction)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn str_to_key(key: &str) -> enigo::Key {
    use enigo::Key;
    match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => Key::Return,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" | "ins" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page_up" => Key::PageUp,
        "pagedown" | "page_down" => Key::PageDown,
        "up" | "arrowup" => Key::UpArrow,
        "down" | "arrowdown" => Key::DownArrow,
        "left" | "arrowleft" => Key::LeftArrow,
        "right" | "arrowright" => Key::RightArrow,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        "ctrl" | "control" => Key::Control,
        "alt" => Key::Alt,
        "shift" => Key::Shift,
        "super" | "meta" | "win" => Key::Meta,
        "capslock" | "caps_lock" => Key::CapsLock,
        "numlock" | "num_lock" => Key::Numlock,
        "scrolllock" | "scroll_lock" => Key::Unicode('\u{0091}'),
        "printscreen" | "print_screen" => Key::Print,
        "pause" => Key::Pause,
        _ => Key::Unicode(key.chars().next().unwrap_or('a')),
    }
}

#[cfg(target_os = "macos")]
fn macos_keycode(key: &str) -> u16 {
    match key.to_ascii_lowercase().as_str() {
        "return" | "enter" => 0x24,
        "tab" => 0x30,
        "space" => 0x31,
        "delete" => 0x33,
        "escape" | "esc" => 0x35,
        "command" | "cmd" | "super" => 0x37,
        "shift" => 0x38,
        "capslock" | "caps_lock" => 0x39,
        "option" | "alt" => 0x3A,
        "control" | "ctrl" => 0x3B,
        "rightshift" => 0x3C,
        "rightoption" | "rightalt" => 0x3D,
        "rightcontrol" | "rightctrl" => 0x3E,
        "f17" => 0x40,
        "volumeup" => 0x48,
        "volumedown" => 0x49,
        "mute" => 0x4A,
        "f18" => 0x4F,
        "f19" => 0x50,
        "f20" => 0x5A,
        "f5" => 0x60,
        "f6" => 0x61,
        "f7" => 0x62,
        "f3" => 0x63,
        "f8" => 0x64,
        "f9" => 0x65,
        "f11" => 0x67,
        "f13" => 0x69,
        "f16" => 0x6A,
        "f14" => 0x6B,
        "f10" => 0x6D,
        "f12" => 0x6F,
        "f15" => 0x71,
        "help" => 0x72,
        "home" => 0x73,
        "pageup" | "page_up" => 0x74,
        "deleteforward" => 0x75,
        "end" => 0x77,
        "pagedown" | "page_down" => 0x79,
        "leftarrow" | "arrowleft" => 0x7B,
        "rightarrow" | "arrowright" => 0x7C,
        "downarrow" | "arrowdown" => 0x7D,
        "uparrow" | "arrowup" => 0x7E,
        _ => 0x00,
    }
}

#[cfg(target_os = "linux")]
fn linux_key_sequence(key: &str) -> String {
    let lower = key.to_ascii_lowercase();
    let normalized = match lower.as_str() {
        "return" | "enter" => "Return",
        "escape" | "esc" => "Escape",
        "space" => "space",
        "tab" => "Tab",
        "backspace" => "BackSpace",
        "delete" | "del" => "Delete",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page_up" => "Page_Up",
        "pagedown" | "page_down" => "Page_Down",
        "up" | "arrowup" => "Up",
        "down" | "arrowdown" => "Down",
        "left" | "arrowleft" => "Left",
        "right" | "arrowright" => "Right",
        "f1" => "F1",
        "f2" => "F2",
        "f3" => "F3",
        "f4" => "F4",
        "f5" => "F5",
        "f6" => "F6",
        "f7" => "F7",
        "f8" => "F8",
        "f9" => "F9",
        "f10" => "F10",
        "f11" => "F11",
        "f12" => "F12",
        "ctrl" | "control" => "Control_L",
        "alt" => "Alt_L",
        "shift" => "Shift_L",
        "super" | "meta" | "win" => "Super_L",
        "capslock" | "caps_lock" => "Caps_Lock",
        "numlock" | "num_lock" => "Num_Lock",
        "scrolllock" | "scroll_lock" => "Scroll_Lock",
        "printscreen" | "print_screen" => "Print",
        "pause" => "Pause",
        _ => return key,
    };
    normalized.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn test_str_to_key_common() {
        assert_eq!(str_to_key("enter"), enigo::Key::Return);
        assert_eq!(str_to_key("tab"), enigo::Key::Tab);
        assert_eq!(str_to_key("escape"), enigo::Key::Escape);
        assert_eq!(str_to_key("space"), enigo::Key::Space);
        assert_eq!(str_to_key("backspace"), enigo::Key::Backspace);
        assert_eq!(str_to_key("f1"), enigo::Key::F1);
        assert_eq!(str_to_key("up"), enigo::Key::UpArrow);
        assert_eq!(str_to_key("ctrl"), enigo::Key::Control);
        assert_eq!(str_to_key("alt"), enigo::Key::Alt);
        assert_eq!(str_to_key("shift"), enigo::Key::Shift);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_str_to_key_unicode() {
        match str_to_key("a") {
            enigo::Key::Unicode(c) => assert_eq!(c, 'a'),
            _ => panic!("Expected Unicode key"),
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_macos_keycode_mapping() {
        assert!(macos_keycode("return") > 0);
        assert!(macos_keycode("unknown") == 0x00);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_linux_key_sequence_normalization() {
        assert_eq!(linux_key_sequence("Return"), "Return");
        assert_eq!(linux_key_sequence("escape"), "Escape");
        assert_eq!(linux_key_sequence("arrowup"), "Up");
        assert_eq!(linux_key_sequence("a"), "a");
    }
}

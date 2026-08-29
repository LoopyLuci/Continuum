use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuItem, PredefinedMenuItem, MenuEvent},
    MouseButton, MouseButtonState,
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    Disconnected,
    Connecting,
    Connected,
    Recording,
}

pub type TrayActionCallback = Box<dyn Fn(TrayMenuAction) + Send>;

pub struct TrayManager {
    icon: Option<TrayIcon>,
    status: TrayStatus,
    on_action: Arc<Mutex<Option<TrayActionCallback>>>,
}

#[derive(Debug, Clone)]
pub enum TrayMenuAction {
    ShowWindow,
    ToggleRecording,
    ToggleAudio,
    About,
    Quit,
}

impl TrayManager {
    pub fn new() -> Self {
        Self {
            icon: None,
            status: TrayStatus::Disconnected,
            on_action: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_action_handler(&mut self, handler: Box<dyn Fn(TrayMenuAction) + Send>) {
        *self.on_action.lock().unwrap() = Some(handler);
    }

    pub fn create(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let menu = Menu::new();
        let show = MenuItem::new("Show Window", true, None);
        let recording = MenuItem::new("Start Recording", true, None);
        let audio = MenuItem::new("Mute Audio", true, None);
        let about = MenuItem::new("About", true, None);
        let quit = MenuItem::new("Quit", true, None);

        menu.append(&show)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&recording)?;
        menu.append(&audio)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&about)?;
        menu.append(&quit)?;

        let icon = Self::create_icon(TrayStatus::Disconnected)?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Continuum — Disconnected")
            .with_icon(icon)
            .build()?;

        self.icon = Some(tray);

        let on_action = self.on_action.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(ref handler) = *on_action.lock().unwrap() {
                    handler(TrayMenuAction::ShowWindow);
                }
            }
        }));

        let on_action = self.on_action.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = match event.id.0.as_str() {
                "Show Window" => Some(TrayMenuAction::ShowWindow),
                "Start Recording" | "Stop Recording" => Some(TrayMenuAction::ToggleRecording),
                "Mute Audio" | "Unmute Audio" => Some(TrayMenuAction::ToggleAudio),
                "About" => Some(TrayMenuAction::About),
                "Quit" => Some(TrayMenuAction::Quit),
                _ => None,
            };
            if let Some(action) = action {
                if let Some(ref handler) = *on_action.lock().unwrap() {
                    handler(action);
                }
            }
        }));

        Ok(())
    }

    pub fn set_status(&mut self, status: TrayStatus) {
        self.status = status;
        if let Some(ref icon) = self.icon {
            let tooltip = match status {
                TrayStatus::Disconnected => "Continuum — Disconnected",
                TrayStatus::Connecting => "Continuum — Connecting...",
                TrayStatus::Connected => "Continuum — Connected",
                TrayStatus::Recording => "Continuum — Recording",
            };
            let _ = icon.set_tooltip(Some(tooltip));
            if let Ok(new_icon) = Self::create_icon(status) {
                let _ = icon.set_icon(Some(new_icon));
            }
        }
    }

    fn create_icon(status: TrayStatus) -> Result<Icon, Box<dyn std::error::Error>> {
        let mut rgba = vec![0u8; 32 * 32 * 4];
        let color = match status {
            TrayStatus::Disconnected => [200u8, 60, 60, 255],
            TrayStatus::Connecting => [240, 180, 40, 255],
            TrayStatus::Connected => [60, 200, 80, 255],
            TrayStatus::Recording => [255, 40, 40, 255],
        };

        let center = 16.0f32;
        let radius = 12.0f32;
        for y in 0..32 {
            for x in 0..32 {
                let dx = x as f32 - center;
                let dy = y as f32 - center;
                if dx * dx + dy * dy <= radius * radius {
                    let idx = (y * 32 + x) * 4;
                    rgba[idx] = color[0];
                    rgba[idx + 1] = color[1];
                    rgba[idx + 2] = color[2];
                    rgba[idx + 3] = color[3];
                }
            }
        }

        if status == TrayStatus::Recording {
            let dot_x = 22usize;
            let dot_y = 8usize;
            let dot_r = 4.0f32;
            for y in 0..32 {
                for x in 0..32 {
                    let dx = x as f32 - dot_x as f32;
                    let dy = y as f32 - dot_y as f32;
                    if dx * dx + dy * dy <= dot_r * dot_r {
                        let idx = (y * 32 + x) * 4;
                        rgba[idx] = 255;
                        rgba[idx + 1] = 255;
                        rgba[idx + 2] = 255;
                        rgba[idx + 3] = 255;
                    }
                }
            }
        }

        Ok(Icon::from_rgba(rgba, 32, 32)?)
    }

    pub fn status(&self) -> TrayStatus {
        self.status
    }
}

impl Default for TrayManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TrayManager {
    fn drop(&mut self) {
        // Tray icon is dropped automatically
    }
}

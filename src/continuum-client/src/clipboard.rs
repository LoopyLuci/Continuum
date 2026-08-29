use arboard::Clipboard;
use chrono::Utc;
use continuum_transport::types::{ClipboardContentType, ClipboardData};

#[allow(dead_code)]
pub struct ClipboardMonitor {
    clipboard: Option<Clipboard>,
    last_content: String,
}

#[allow(dead_code)]
impl ClipboardMonitor {
    pub fn new() -> Self {
        let clipboard = Clipboard::new().ok();
        Self {
            clipboard,
            last_content: String::new(),
        }
    }

    pub fn poll(&mut self) -> Option<ClipboardData> {
        let clipboard = self.clipboard.as_mut()?;
        let content = clipboard.get_text().ok()?;
        if content != self.last_content && !content.is_empty() {
            self.last_content = content.clone();
            Some(ClipboardData {
                content_type: ClipboardContentType::Text,
                data: content.into_bytes(),
                timestamp: Utc::now(),
            })
        } else {
            None
        }
    }

    pub fn set_text(&mut self, text: &str) {
        if let Some(ref mut clipboard) = self.clipboard {
            let _ = clipboard.set_text(text.to_string());
            self.last_content = text.to_string();
        }
    }
}

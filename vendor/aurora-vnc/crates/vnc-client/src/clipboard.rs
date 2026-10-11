//! Clipboard state for one session.
//!
//! When the server answers our ExtendedClipboard pseudo-encoding with its
//! Caps, text travels as UTF-8 (any language, up to
//! `Limits::max_cut_text_len`) using the lazy Notify / Request / Provide
//! exchange, so a large clipboard only crosses the wire when the other side
//! actually pastes it. Otherwise it falls back to RFC 6143 cut text, which
//! is Latin-1 only.
//!
//! Servers differ in when they ask for our text: vnc-host sends a Request
//! as soon as we Notify, while Xvnc waits until an X application reads the
//! clipboard. Xvnc also pushes desktop changes as a Provide without a Notify
//! first. Both are handled here.

use rfb_proto::clipboard::{self as xc, ExtClipboard};

/// What the server said it supports in its Caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Peer {
    flags: u32,
    /// Largest text it accepts without asking for it (unsolicited Provide).
    text_limit: u32,
}

/// A message the session should send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    Extended(ExtClipboard),
    /// Plain ClientCutText (Latin-1).
    Legacy(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct Clipboard {
    peer: Option<Peer>,
    /// Our side's current text, kept until the server asks for it.
    local: Option<String>,
    /// The last text the server gave us. A viewer puts it on the local
    /// clipboard and then reads it straight back; that is not a new copy.
    remote: Option<String>,
    max_len: u32,
}

/// Actions and formats we offer in our own Caps.
pub const CLIENT_FLAGS: u32 = xc::TEXT | xc::REQUEST | xc::PEEK | xc::NOTIFY | xc::PROVIDE;

impl Clipboard {
    pub fn new(max_len: u32) -> Self {
        Clipboard { peer: None, local: None, remote: None, max_len }
    }

    /// The server negotiated Extended Clipboard (UTF-8 text).
    pub fn is_extended(&self) -> bool {
        self.peer.is_some()
    }

    fn peer_can(&self, action: u32) -> bool {
        self.peer.is_some_and(|p| p.flags & action != 0 && p.flags & xc::TEXT != 0)
    }

    /// Handles one server message. Returns the replies to send and any
    /// clipboard text the server delivered.
    pub fn on_server(&mut self, msg: ExtClipboard) -> (Vec<ExtClipboard>, Option<String>) {
        let mut out = Vec::new();
        let mut text = None;
        match msg {
            ExtClipboard::Caps { flags, sizes } => {
                let text_limit = if flags & xc::TEXT != 0 { sizes.first().copied().unwrap_or(0) } else { 0 };
                self.peer = Some(Peer { flags, text_limit });
                out.push(ExtClipboard::Caps { flags: CLIENT_FLAGS, sizes: vec![self.max_len] });
                if self.local.is_some() {
                    out.extend(self.announce());
                }
            }
            ExtClipboard::Request { formats } => {
                if formats & xc::TEXT != 0 && self.peer_can(xc::PROVIDE) {
                    if let Some(t) = &self.local {
                        out.push(ExtClipboard::provide_text(t));
                    }
                }
            }
            ExtClipboard::Peek => {
                if self.peer_can(xc::NOTIFY) {
                    out.push(ExtClipboard::Notify { formats: if self.local.is_some() { xc::TEXT } else { 0 } });
                }
            }
            ExtClipboard::Notify { formats } => {
                // The server's clipboard has new content, so ours is no
                // longer the current one; fetch the text. An empty Notify
                // (the desktop app that owned it went away) leaves ours be:
                // it often crosses our own Notify on the wire.
                if formats != 0 {
                    self.local = None;
                }
                if formats & xc::TEXT != 0 && self.peer_can(xc::REQUEST) {
                    out.push(ExtClipboard::Request { formats: xc::TEXT });
                }
            }
            m @ ExtClipboard::Provide { .. } => {
                self.local = None;
                text = m.text();
                self.remote.clone_from(&text);
            }
            ExtClipboard::Other { .. } => {}
        }
        (out, text)
    }

    /// Our clipboard changed to `text`. Returns what to send now (nothing
    /// before the handshake; the text is announced once Caps arrive).
    pub fn set_local(&mut self, text: &str, connected: bool) -> Vec<Outgoing> {
        if self.local.as_deref() == Some(text) || self.remote.as_deref() == Some(text) {
            return Vec::new();
        }
        self.remote = None;
        self.local = Some(text.to_owned());
        if self.peer.is_some() {
            self.announce().into_iter().map(Outgoing::Extended).collect()
        } else if connected {
            vec![Outgoing::Legacy(to_latin1(text, self.max_len))]
        } else {
            Vec::new()
        }
    }

    /// The server sent legacy cut text; remember it so it is not echoed back.
    pub fn note_remote(&mut self, text: &str) {
        self.local = None;
        self.remote = Some(text.to_owned());
    }

    /// Tells the server about our text: a Notify if it can Request it, else
    /// the text itself if it fits the server's unsolicited limit.
    fn announce(&self) -> Option<ExtClipboard> {
        let text = self.local.as_ref()?;
        if self.peer_can(xc::NOTIFY) {
            Some(ExtClipboard::Notify { formats: xc::TEXT })
        } else if self.peer_can(xc::PROVIDE)
            && self.peer.is_some_and(|p| xc::text_to_wire(text).len() as u64 <= u64::from(p.text_limit))
        {
            Some(ExtClipboard::provide_text(text))
        } else {
            None
        }
    }
}

/// RFC 6143 cut text is Latin-1: other characters become `?`, line endings
/// are LF, and the result is cut at `max_len` bytes.
pub fn to_latin1(text: &str, max_len: u32) -> Vec<u8> {
    text.replace("\r\n", "\n")
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .take(max_len as usize)
        .collect()
}

/// Decodes RFC 6143 cut text (Latin-1, every byte is a code point).
pub fn from_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(flags: u32, text_limit: u32) -> ExtClipboard {
        ExtClipboard::Caps { flags: flags | xc::TEXT, sizes: vec![text_limit] }
    }

    #[test]
    fn caps_are_answered_and_pending_text_announced() {
        let mut c = Clipboard::new(1000);
        assert!(c.set_local("früh", false).is_empty());
        let (out, text) = c.on_server(caps(xc::REQUEST | xc::NOTIFY | xc::PROVIDE, 100));
        assert_eq!(text, None);
        assert_eq!(
            out,
            vec![
                ExtClipboard::Caps { flags: CLIENT_FLAGS, sizes: vec![1000] },
                ExtClipboard::Notify { formats: xc::TEXT }
            ]
        );
        assert!(c.is_extended());
        // vnc-host style: Request right away; we Provide UTF-8.
        let (out, _) = c.on_server(ExtClipboard::Request { formats: xc::TEXT });
        assert_eq!(out, vec![ExtClipboard::provide_text("früh")]);
        assert_eq!(out[0].text().as_deref(), Some("früh"));
    }

    #[test]
    fn server_changes_are_fetched_or_taken_directly() {
        let mut c = Clipboard::new(1000);
        c.on_server(caps(xc::REQUEST | xc::NOTIFY | xc::PROVIDE, 100));
        c.set_local("mine", true);
        let (out, _) = c.on_server(ExtClipboard::Notify { formats: xc::TEXT });
        assert_eq!(out, vec![ExtClipboard::Request { formats: xc::TEXT }]);
        // Ours is stale now: a late Request gets nothing.
        assert!(c.on_server(ExtClipboard::Request { formats: xc::TEXT }).0.is_empty());
        // Xvnc style: Provide without a Notify.
        let (out, text) = c.on_server(ExtClipboard::provide_text("日本語\nline"));
        assert!(out.is_empty());
        assert_eq!(text.as_deref(), Some("日本語\nline"));
        // Reading it back off the local clipboard is not a new copy.
        assert!(c.set_local("日本語\nline", true).is_empty());
        // An empty Notify asks for nothing and keeps our text.
        c.set_local("again", true);
        assert!(c.on_server(ExtClipboard::Notify { formats: 0 }).0.is_empty());
        assert_eq!(
            c.on_server(ExtClipboard::Request { formats: xc::TEXT }).0,
            vec![ExtClipboard::provide_text("again")]
        );
    }

    #[test]
    fn without_notify_small_text_is_pushed_and_large_waits() {
        let mut c = Clipboard::new(1000);
        c.on_server(caps(xc::PROVIDE | xc::REQUEST, 8));
        assert_eq!(c.set_local("short", true), vec![Outgoing::Extended(ExtClipboard::provide_text("short"))]);
        assert!(c.set_local("much too long", true).is_empty());
        assert!(c.set_local("much too long", true).is_empty(), "unchanged text is not resent");
    }

    #[test]
    fn peek_reports_what_we_hold() {
        let mut c = Clipboard::new(1000);
        c.on_server(caps(xc::NOTIFY, 0));
        assert_eq!(c.on_server(ExtClipboard::Peek).0, vec![ExtClipboard::Notify { formats: 0 }]);
        c.set_local("x", true);
        assert_eq!(c.on_server(ExtClipboard::Peek).0, vec![ExtClipboard::Notify { formats: xc::TEXT }]);
    }

    #[test]
    fn legacy_servers_get_latin1() {
        let mut c = Clipboard::new(6);
        assert_eq!(c.set_local("café ✓ ok\r\n!", true), vec![Outgoing::Legacy(b"caf\xe9 ?".to_vec())]);
        assert_eq!(from_latin1(b"na\xefve"), "naïve");
    }
}

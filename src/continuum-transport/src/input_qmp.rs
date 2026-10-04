//! Inject keyboard and pointer input into a QEMU guest over QMP.
//!
//! ## Two protocols, one per device
//!
//! Keys go through HMP's `sendkey`, which is QMP's passthrough to the human
//! monitor prompt:
//!
//! ```json
//! {"execute":"human-monitor-command","arguments":{"command-line":"sendkey shift-a"}}
//! ```
//!
//! `sendkey` names keys directly ("a", "ret", "shift-minus") and operates on
//! QEMU's always-present PS/2 keyboard, so it needs nothing on the guest's
//! command line. QMP's own `input-send-event` would need a USB keyboard and a
//! keycode table of its own; `sendkey` is one call per keystroke and reads like
//! the key it types.
//!
//! Pointer input cannot go through HMP and needs the second protocol:
//!
//! ```json
//! {"execute":"input-send-event","arguments":{"events":[
//!   {"type":"abs","data":{"axis":"x","value":16384.0}},
//!   {"type":"abs","data":{"axis":"y","value":8192.0}}]}}
//! {"execute":"input-send-event","arguments":{"events":[
//!   {"type":"btn","data":{"down":true,"button":"left"}}]}}
//! {"execute":"input-send-event","arguments":{"events":[
//!   {"type":"wheel","data":{"axis":"up","value":3}}]}}
//! ```
//!
//! ## REQUIREMENT: the guest needs `-device usb-tablet`
//!
//! **The VM must be launched with `-device usb-tablet`, or pointer input does
//! nothing at all — no error, no event, just a pointer that never moves.**
//! `input-send-event` with `type: "abs"` is only meaningful for a device that
//! declares absolute axes; a PS/2 mouse declares relative ones, so the events
//! are silently dropped by QEMU rather than rejected. Give the tablet an
//! explicit id (`-device usb-tablet,id=tablet0`) and pass
//! `Some("tablet0")` as `tablet_device` so the events cannot land on the wrong
//! device when QEMU's auto-numbering shifts. When `tablet_device` is `None`
//! the `device` field is omitted entirely and QEMU routes to its sole input
//! device, which is correct only for a VM with exactly one pointing device.
//!
//! ## Why keystrokes are paced
//!
//! HMP takes exactly one key per invocation, and [`QmpClient`] serialises every
//! command behind one lock, so typing is unavoidably serial. That serialisation
//! is necessary but not sufficient: guests *poll* the keyboard, and input
//! arriving faster than they poll is consumed only in part. The failure is
//! invisible — a password comes out one character short and produces a lockout
//! counter rather than an error. [`DEFAULT_KEY_DELAY`] of 50 ms was
//! established empirically in the VM-Harness guest driver; 1 ms per character
//! loses characters, 50 ms types a password in about a second.
//!
//! ## Why every unmappable character is an error
//!
//! [`key_for`] returns `Err` for a character it cannot encode and never falls
//! back to skipping it. A dropped character in a password produces a wrong
//! password, not a visible failure. A loud error at type time is far cheaper.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use continuum_core::input::{InputAction, InputInjector, ModifierKeys, MouseButton, RemoteInputEvent};
use continuum_core::{ContinuumError, ContinuumResult};

use crate::qmp::QmpClient;

/// Minimum interval between two keystrokes delivered to a guest.
///
/// See the module docs: below roughly 50 ms guests drop characters.
pub const DEFAULT_KEY_DELAY: Duration = Duration::from_millis(50);

/// Characters HMP spells differently from ASCII.
///
/// Kept separate from the shift rule because these are *names*, not
/// punctuation.
pub static NAMED_KEYS: &[(&str, &str)] = &[
    (" ", "spc"),
    ("\n", "ret"),
    ("\r", "ret"),
    ("\t", "tab"),
    ("-", "minus"),
    ("=", "equal"),
    ("[", "bracket_left"),
    ("]", "bracket_right"),
    (";", "semicolon"),
    ("'", "apostrophe"),
    ("`", "grave_accent"),
    ("\\", "backslash"),
    (",", "comma"),
    (".", "dot"),
    ("/", "slash"),
];

/// Punctuation that lives on a shifted key, as HMP sees it.
///
/// `!` is `shift-1`, `_` is `shift-minus`, and so on. Without this layer a
/// password containing ``!@#$%^&*()_+{}|:"<>?~`` is untypeable, which is most
/// strong passwords.
///
/// This is a US keyboard layout, and it is the *guest's* layout that decides
/// what these keys mean. HMP names the physical key, not the character, so on a
/// DE layout `shift-7` produces a slash rather than an ampersand. A guest with a
/// non-US layout needs its own table, not a different caller.
pub static SHIFTED_KEYS: &[(&str, &str)] = &[
    ("!", "shift-1"),
    ("@", "shift-2"),
    ("#", "shift-3"),
    ("$", "shift-4"),
    ("%", "shift-5"),
    ("^", "shift-6"),
    ("&", "shift-7"),
    ("*", "shift-8"),
    ("(", "shift-9"),
    (")", "shift-0"),
    ("_", "shift-minus"),
    ("+", "shift-equal"),
    ("{", "shift-bracket_left"),
    ("}", "shift-bracket_right"),
    ("|", "shift-backslash"),
    (":", "shift-semicolon"),
    ("\"", "shift-apostrophe"),
    ("~", "shift-grave_accent"),
    ("<", "shift-comma"),
    (">", "shift-dot"),
    ("?", "shift-slash"),
];

/// Control keys, addressable by name for callers that do not want to type them.
pub static CONTROL_KEYS: &[(&str, &str)] = &[
    ("ret", "ret"),
    ("enter", "ret"),
    ("tab", "tab"),
    ("esc", "esc"),
    ("escape", "esc"),
    ("backspace", "backspace"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("home", "home"),
    ("end", "end"),
    ("delete", "delete"),
];

/// The 36 ASCII key names HMP spells as themselves: ten digits then twenty-six
/// letters.
///
/// A `static` table rather than `String::leak` or an intern cache: the result
/// has to be `&'static str`, 36 literals cost nothing, and the table is
/// inspectable. Digits come first so `ascii_key_name` is one offset lookup
/// per class rather than two tables to keep in sync.
static ASCII_KEY_NAMES: [&str; 36] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "a", "b", "c", "d", "e", "f", "g", "h", "i",
    "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
];

/// The same twenty-six letters with shift folded into the name.
///
/// HMP has no notion of "uppercase A"; it has twenty-six keys and twenty-six
/// shifted keys. Building these as literals rather than with `format!` keeps
/// `key_for` allocation-free on the letter path.
static SHIFTED_ASCII_KEY_NAMES: [&str; 26] = [
    "shift-a", "shift-b", "shift-c", "shift-d", "shift-e", "shift-f", "shift-g", "shift-h",
    "shift-i", "shift-j", "shift-k", "shift-l", "shift-m", "shift-n", "shift-o", "shift-p",
    "shift-q", "shift-r", "shift-s", "shift-t", "shift-u", "shift-v", "shift-w", "shift-x",
    "shift-y", "shift-z",
];

/// HMP's name for an ASCII digit or lowercase letter, if it has one.
fn ascii_key_name(byte: u8) -> Option<&'static str> {
    match byte {
        b'0'..=b'9' => ASCII_KEY_NAMES.get((byte - b'0') as usize).copied(),
        b'a'..=b'z' => ASCII_KEY_NAMES.get(10 + (byte - b'a') as usize).copied(),
        _ => None,
    }
}

/// HMP's name for an ASCII letter, shifted when `uppercase`.
fn ascii_letter_key_name(byte: u8, uppercase: bool) -> Option<&'static str> {
    if uppercase {
        SHIFTED_ASCII_KEY_NAMES.get((byte - b'a') as usize).copied()
    } else {
        ASCII_KEY_NAMES.get(10 + (byte - b'a') as usize).copied()
    }
}

fn lookup(table: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn unsupported(input: &str, code_point: Option<u32>, reason: &'static str) -> UnsupportedKey {
    UnsupportedKey {
        input: input.to_string(),
        code_point,
        reason,
    }
}

/// A key that cannot be encoded for the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedKey {
    /// The single character, or the key name, that could not be encoded.
    pub input: String,
    /// Unicode scalar value, when the input was a character.
    pub code_point: Option<u32>,
    /// Why it was rejected.
    pub reason: &'static str,
}

impl std::fmt::Display for UnsupportedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code_point {
            Some(cp) => write!(
                f,
                "cannot type {:?} (U+{cp:04X}): {}. Extend the key table, or set the \
                 guest password to avoid this character.",
                self.input, self.reason
            ),
            None => write!(
                f,
                "unknown key {:?}: {}. Send a single character, or one of {:?}.",
                self.input, self.reason, control_key_names()
            ),
        }
    }
}

impl std::error::Error for UnsupportedKey {}

/// Every accepted control-key spelling, for error messages.
pub fn control_key_names() -> Vec<&'static str> {
    CONTROL_KEYS.iter().map(|(k, _)| *k).collect()
}

/// The HMP `sendkey` name for a single character.
///
/// Covers all 95 printable ASCII characters for a US layout.
///
/// # Errors
///
/// [`UnsupportedKey`] for anything else. It never falls back to skipping the
/// character: a dropped character in a password produces a wrong password,
/// not an error.
///
/// # Examples
///
/// ```
/// use continuum_transport::input_qmp::key_for;
/// assert_eq!(key_for('a').unwrap(), "a");
/// assert_eq!(key_for('A').unwrap(), "shift-a");
/// assert_eq!(key_for('!').unwrap(), "shift-1");
/// assert!(key_for('\u{00e9}').is_err());
/// ```
pub fn key_for(ch: char) -> Result<&'static str, UnsupportedKey> {
    // A char always stringifies to at most 4 bytes; allocating to look it up
    // is not the hot path (one call per keystroke, paced at 50 ms).
    let as_str = ch.to_string();

    if let Some(name) = lookup(NAMED_KEYS, &as_str) {
        return Ok(name);
    }
    if let Some(name) = lookup(SHIFTED_KEYS, &as_str) {
        return Ok(name);
    }
    if ch.is_ascii_alphabetic() {
        // HMP has no uppercase; shift is part of the key name. Lowercase must
        // NOT get the shift prefix — `shift-a` types "A", so a blanket shift
        // here would type every letter of a password in capitals.
        return ascii_letter_key_name(ch.to_ascii_lowercase() as u8, ch.is_ascii_uppercase())
            .ok_or_else(|| unsupported(&as_str, Some(ch as u32), "no HMP sendkey name for it"));
    }
    if ch.is_ascii_digit() {
        return ascii_key_name(ch as u8)
            .ok_or_else(|| unsupported(&as_str, Some(ch as u32), "no HMP sendkey name for it"));
    }

    Err(unsupported(
        &as_str,
        Some(ch as u32),
        "no HMP sendkey name for it",
    ))
}

/// Resolve a control-key *name* ("ret", "escape") to its HMP name.
pub fn control_key(name: &str) -> Option<&'static str> {
    lookup(CONTROL_KEYS, &name.to_ascii_lowercase())
}

/// Build the full `sendkey` argument for a key and a modifier set.
///
/// HMP composes chords with `-`, so ctrl+alt+Delete is `ctrl-alt-delete`.
/// Shift is folded in only when the key name does not already carry it:
/// [`key_for`] returns `shift-a` for "A", and prefixing another shift yields
/// `shift-shift-a`, which HMP rejects.
///
/// Accepts either a single character (`"A"`) or a control-key name (`"ret"`),
/// because callers speak both vocabularies — a GUI key handler sends names, a
/// paste path sends characters.
///
/// # Examples
///
/// ```
/// use continuum_transport::input_qmp::resolve_sendkey;
/// use continuum_core::input::ModifierKeys;
/// assert_eq!(resolve_sendkey("a", ModifierKeys::default()).unwrap(), "a");
/// assert_eq!(
///     resolve_sendkey("A", ModifierKeys { ctrl: true, ..Default::default() }).unwrap(),
///     "ctrl-shift-a"
/// );
/// assert_eq!(resolve_sendkey("ret", ModifierKeys::default()).unwrap(), "ret");
/// ```
pub fn resolve_sendkey(key: &str, modifiers: ModifierKeys) -> Result<String, UnsupportedKey> {
    if key.is_empty() {
        return Err(unsupported(key, None, "empty key name"));
    }
    let base = match key.chars().next() {
        Some(first) if key.chars().count() == 1 => key_for(first)?.to_string(),
        _ => control_key(key)
            .map(str::to_string)
            .ok_or_else(|| unsupported(key, None, "not a control key name"))?,
    };

    let mut prefix: Vec<&str> = Vec::with_capacity(3);
    if modifiers.ctrl {
        prefix.push("ctrl");
    }
    if modifiers.alt {
        prefix.push("alt");
    }
    if modifiers.super_key {
        prefix.push("meta");
    }
    if modifiers.shift && !base.starts_with("shift-") {
        prefix.push("shift");
    }
    if prefix.is_empty() {
        return Ok(base);
    }
    Ok(format!("{}-{}", prefix.join("-"), base))
}

/// Map a client pixel coordinate onto QEMU's absolute tablet axis range.
///
/// Clamped, not wrapped: a pointer dragged past the edge of a widget must park
/// at the edge of the guest screen, and wrapping sends it to the opposite
/// corner.
///
/// A surface one pixel wide or narrower has no meaningful pointer mapping, so
/// it returns the axis minimum rather than dividing by an extent that is
/// effectively zero.
pub fn to_axis(pixel: i32, extent: u32) -> f32 {
    if extent <= 1 {
        return 0.0;
    }
    let max = extent - 1;
    let clamped = pixel.clamp(0, max as i32) as f32;
    (clamped / max as f32) * crate::capture_qmp::TABLET_AXIS_MAX
}

fn abs_event(axis: &str, value: f32) -> serde_json::Value {
    // QEMU rejects a float here: "Invalid parameter type for
    // 'events[0].data.value', expected: integer". to_axis scales
    // fractionally to keep pointer motion smooth, so the rounding belongs at
    // this boundary -- and it has to happen, or every pointer event fails and
    // the mouse silently never moves in the guest.
    serde_json::json!({
        "type": "abs",
        "data": { "axis": axis, "value": value.round() as i64 },
    })
}

fn wheel_event(axis: &str, value: u32) -> serde_json::Value {
    serde_json::json!({
        "type": "wheel",
        "data": { "axis": axis, "value": value },
    })
}

fn button_event(button: MouseButton, pressed: bool) -> serde_json::Value {
    serde_json::json!({
        "type": "btn",
        "data": { "down": pressed, "button": button_name(button) },
    })
}

fn button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    }
}

/// An [`InputInjector`] that drives a QEMU guest over QMP.
///
/// Cloned freely: it is a handful of handles, and the clone is what lets the
/// synchronous [`InputInjector::apply`] hand the work to a runtime on another
/// thread.
#[derive(Clone)]
pub struct QmpInputInjector {
    client: Arc<QmpClient>,
    vm_name: String,
    /// QEMU `id=` of the `usb-tablet` to route pointer events to. See the
    /// module docs: the guest needs that device or pointer input is silently
    /// discarded.
    tablet_device: Option<String>,
    key_delay: Duration,
    /// Guest surface size, used to map pixel coordinates onto the tablet's
    /// absolute axis range.
    guest_width: u32,
    guest_height: u32,
    available: bool,
}

impl QmpInputInjector {
    pub fn new(vm_name: &str, client: Arc<QmpClient>, tablet_device: Option<String>) -> Self {
        Self {
            client,
            vm_name: vm_name.to_string(),
            tablet_device,
            key_delay: DEFAULT_KEY_DELAY,
            guest_width: 1920,
            guest_height: 1080,
            available: true,
        }
    }

    pub fn with_key_delay(mut self, delay: Duration) -> Self {
        self.key_delay = delay;
        self
    }

    /// Tell the injector how large the guest's display is, so pointer
    /// coordinates can be normalised into the tablet's absolute axis range.
    ///
    /// Getting this wrong is not cosmetic: an injector that thinks the guest is
    /// 1920 wide when it is 1280 puts the pointer 50% of the screen too far
    /// right, and no amount of clicking fixes it.
    pub fn with_guest_size(mut self, width: u32, height: u32) -> Self {
        self.guest_width = width.max(1);
        self.guest_height = height.max(1);
        self
    }

    pub fn vm_name(&self) -> &str {
        &self.vm_name
    }

    pub fn key_delay(&self) -> Duration {
        self.key_delay
    }

    pub fn guest_size(&self) -> (u32, u32) {
        (self.guest_width, self.guest_height)
    }

    /// The `-device usb-tablet,id=...` this injector targets, if pinned.
    pub fn tablet_device(&self) -> Option<&str> {
        self.tablet_device.as_deref()
    }

    /// Send one keystroke, then wait out the pacing delay.
    ///
    /// HMP's `sendkey` presses *and* releases in a single call, so there is no
    /// half-key state to hold; the delay is applied after the key lands, which
    /// is what keeps the guest's poll loop from coalescing two keystrokes into
    /// one.
    pub async fn send_key_name(&self, name: &str) -> ContinuumResult<()> {
        self.client
            .hmp(&format!("sendkey {name}"))
            .await
            .map_err(|e| self.qmp_err("sendkey", e))?;
        if !self.key_delay.is_zero() {
            tokio::time::sleep(self.key_delay).await;
        }
        Ok(())
    }

    /// Type a whole string one character at a time.
    ///
    /// Validates the *entire* string before sending anything, so a typo in an
    /// unsupported character cannot leave half a password in the guest's input
    /// buffer — the failure mode that costs a login attempt.
    pub async fn type_text(&self, text: &str) -> ContinuumResult<()> {
        let mut keys = Vec::with_capacity(text.len());
        for ch in text.chars() {
            match key_for(ch) {
                Ok(name) => keys.push(name),
                Err(e) => return Err(ContinuumError::Input(e.to_string())),
            }
        }
        let count = keys.len();
        for name in keys {
            self.send_key_name(name).await?;
        }
        tracing::debug!(vm = %self.vm_name, chars = count, "typed into guest");
        Ok(())
    }

    /// Send raw `input-send-event` events, targeting the tablet when pinned.
    pub async fn send_input_events(&self, events: Vec<serde_json::Value>) -> ContinuumResult<()> {
        let mut args = serde_json::Map::new();
        args.insert("events".into(), serde_json::Value::Array(events));
        if let Some(device) = &self.tablet_device {
            args.insert("device".into(), serde_json::Value::String(device.clone()));
        }
        self.client
            .execute("input-send-event", Some(serde_json::Value::Object(args)))
            .await
            .map(|_| ())
            .map_err(|e| self.qmp_err("input-send-event", e))
    }

    /// Move the guest pointer to a pixel coordinate.
    ///
    /// Both axes travel in one `input-send-event`: splitting them costs two
    /// round trips and, in a guest that reads both axes per poll, draws a
    /// diagonal twitch from the stale one.
    pub async fn move_pointer(&self, x: i32, y: i32) -> ContinuumResult<()> {
        let events = vec![
            abs_event("x", to_axis(x, self.guest_width)),
            abs_event("y", to_axis(y, self.guest_height)),
        ];
        self.send_input_events(events).await
    }

    pub async fn button(&self, button: MouseButton, pressed: bool) -> ContinuumResult<()> {
        self.send_input_events(vec![button_event(button, pressed)])
            .await
    }

    /// Scroll by `(dx, dy)` detents. Positive `dy` scrolls up.
    pub async fn scroll(&self, dx: i32, dy: i32) -> ContinuumResult<()> {
        let mut events = Vec::with_capacity(2);
        if dy != 0 {
            // QEMU's wheel axis is directional *and* signed, so the sign is
            // carried by the axis name and the value stays a magnitude.
            // Folding the sign into the value too would scroll the wrong way
            // on the guests that already read the axis as the direction.
            events.push(wheel_event(if dy > 0 { "up" } else { "down" }, dy.unsigned_abs()));
        }
        if dx != 0 {
            events.push(wheel_event(if dx > 0 { "right" } else { "left" }, dx.unsigned_abs()));
        }
        if events.is_empty() {
            return Ok(());
        }
        self.send_input_events(events).await
    }

    /// Apply an event, translating it into the QMP protocol.
    pub async fn apply_async(&self, event: &RemoteInputEvent) -> ContinuumResult<()> {
        // QMP has no notion of a relative mouse path, and the host injectors
        // move-then-click. Coalescing here keeps the two backends
        // observationally identical from the guest's point of view.
        if let (Some(x), Some(y)) = (event.x, event.y) {
            if matches!(
                event.action,
                InputAction::MouseMove
                    | InputAction::MouseDown
                    | InputAction::MouseUp
                    | InputAction::MouseClick
            ) {
                self.move_pointer(x, y).await?;
            }
        }

        match event.action {
            InputAction::MouseMove => Ok(()),
            InputAction::MouseClick => {
                let button = event.button.unwrap_or(MouseButton::Left);
                self.button(button, true).await?;
                self.button(button, false).await
            }
            InputAction::MouseDown => self.button(event.button.unwrap_or(MouseButton::Left), true).await,
            InputAction::MouseUp => self.button(event.button.unwrap_or(MouseButton::Left), false).await,
            InputAction::MouseScroll => {
                self.scroll(
                    event.scroll_x.unwrap_or(0.0) as i32,
                    event.scroll_y.unwrap_or(0.0) as i32,
                )
                .await
            }
            InputAction::KeyPress | InputAction::KeyDown => {
                let key = event.key.as_deref().unwrap_or("");
                let name = resolve_sendkey(key, event.modifiers.unwrap_or_default())
                    .map_err(|e| ContinuumError::Input(e.to_string()))?;
                self.send_key_name(&name).await
            }
            InputAction::KeyUp => {
                // `sendkey` presses and releases atomically, so a release has
                // nothing left to send. Validated anyway, and logged rather
                // than silently dropped: a client holding a key down across
                // frames should know the guest sees a discrete tap.
                let key = event.key.as_deref().unwrap_or("");
                resolve_sendkey(key, event.modifiers.unwrap_or_default())
                    .map_err(|e| ContinuumError::Input(e.to_string()))?;
                tracing::debug!(
                    vm = %self.vm_name,
                    "key release is implicit in HMP sendkey; no command sent"
                );
                Ok(())
            }
        }
    }

    fn qmp_err(&self, what: &str, e: crate::qmp::QmpError) -> ContinuumError {
        ContinuumError::Input(format!(
            "QMP {what} failed for VM {}: {e}",
            self.vm_name
        ))
    }
}

impl InputInjector for QmpInputInjector {
    fn apply(&mut self, event: &RemoteInputEvent) -> ContinuumResult<()> {
        // `apply` is synchronous because the host injectors are, but QMP is a
        // socket round trip. Blocking the caller for a few milliseconds is
        // fine; nesting a runtime inside the caller's runtime deadlocks, so
        // the work goes to a fresh runtime on its own thread.
        let this = self.clone();
        // `async move` so the future owns the injector; a future that merely
        // borrowed it would not outlive the closure that returns it.
        run_blocking(move || async move { this.apply_async(event).await })
    }

    fn name(&self) -> &str {
        "qmp"
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

/// Run a future to completion on a dedicated thread with its own runtime.
///
/// `InputInjector::apply` is synchronous because the host injectors are, but
/// QMP is a socket round trip. Blocking the caller for a few milliseconds is
/// fine; nesting a runtime inside the caller's runtime deadlocks, so the work
/// goes to a fresh runtime on its own thread.
fn run_blocking<F, Fut>(f: F) -> ContinuumResult<()>
where
    F: FnOnce() -> Fut + Send,
    Fut: std::future::Future<Output = ContinuumResult<()>>,
{
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| ContinuumError::Input(format!("input runtime: {e}")))?
                    .block_on(f())
            })
            .join()
            .map_err(|_| ContinuumError::Input("input thread panicked".into()))?
    })
}

/// Connect to a VM's QMP endpoint and build an injector for it.
pub async fn connect(
    vm_name: &str,
    qmp_addr: SocketAddr,
    tablet_device: Option<String>,
) -> ContinuumResult<QmpInputInjector> {
    let client =
        QmpClient::connect(qmp_addr).await.map_err(|e| {
            ContinuumError::Input(format!("QMP connect to {qmp_addr} failed: {e}"))
        })?;
    Ok(QmpInputInjector::new(vm_name, Arc::new(client), tablet_device))
}

/// Convert a `continuum-core` event into the wire event the QUIC intent stream
/// carries.
///
/// The two crates define the same event twice; this makes the duplication a
/// checked conversion rather than two definitions free to drift.
pub fn core_event_to_transport(event: &RemoteInputEvent) -> crate::types::RemoteInputEvent {
    crate::types::RemoteInputEvent {
        action: match event.action {
            InputAction::MouseMove => crate::types::InputAction::MouseMove,
            InputAction::MouseClick => crate::types::InputAction::MouseClick,
            InputAction::MouseDown => crate::types::InputAction::MouseDown,
            InputAction::MouseUp => crate::types::InputAction::MouseUp,
            InputAction::MouseScroll => crate::types::InputAction::MouseScroll,
            InputAction::KeyPress => crate::types::InputAction::KeyPress,
            InputAction::KeyDown => crate::types::InputAction::KeyDown,
            InputAction::KeyUp => crate::types::InputAction::KeyUp,
        },
        x: event.x,
        y: event.y,
        button: event.button.map(|b| match b {
            MouseButton::Left => crate::types::MouseButton::Left,
            MouseButton::Right => crate::types::MouseButton::Right,
            MouseButton::Middle => crate::types::MouseButton::Middle,
        }),
        key: event.key.clone(),
        modifiers: event.modifiers.map(|m| crate::types::ModifierKeys {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            super_key: m.super_key,
        }),
        scroll_x: event.scroll_x,
        scroll_y: event.scroll_y,
        monitor_id: event.monitor_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every one of the 95 printable ASCII characters.
    fn printable_ascii() -> Vec<char> {
        (0x20u8..0x7Fu8).map(char::from).collect()
    }

    fn no_mods() -> ModifierKeys {
        ModifierKeys::default()
    }

    #[test]
    fn test_there_are_exactly_95_printable_ascii_chars() {
        assert_eq!(printable_ascii().len(), 95);
    }

    #[test]
    fn test_every_printable_ascii_char_maps() {
        let mut mapped = 0usize;
        for ch in printable_ascii() {
            match key_for(ch) {
                Ok(name) => {
                    assert!(!name.is_empty(), "{ch:?} mapped to an empty name");
                    assert!(
                        !name.contains(char::is_whitespace),
                        "{ch:?} mapped to {name:?}, which would break the sendkey argv"
                    );
                    mapped += 1;
                }
                Err(e) => panic!("printable ASCII {ch:?} has no key name: {e}"),
            }
        }
        assert_eq!(mapped, 95);
    }

    #[test]
    fn test_lowercase_letters_have_no_shift_prefix() {
        for b in b'a'..=b'z' {
            let ch = char::from(b);
            let name = key_for(ch).unwrap_or_else(|e| panic!("{ch:?}: {e}"));
            assert_eq!(name, ch.to_string(), "{ch:?} must be its own key name");
            assert!(
                !name.starts_with("shift"),
                "{ch:?} got a shift prefix: {name}"
            );
        }
    }

    #[test]
    fn test_uppercase_letters_are_shifted_lowercase() {
        for b in b'A'..=b'Z' {
            let ch = char::from(b);
            let name = key_for(ch).unwrap_or_else(|e| panic!("{ch:?}: {e}"));
            assert_eq!(name, format!("shift-{}", ch.to_ascii_lowercase()));
        }
    }

    #[test]
    fn test_digits_map_to_themselves_unshifted() {
        for b in b'0'..=b'9' {
            let ch = char::from(b);
            assert_eq!(key_for(ch).unwrap(), ch.to_string());
        }
    }

    #[test]
    fn test_shifted_punctuation_is_explicit() {
        // The table is not derived; assert the values that matter, because a
        // wrong shift here types the wrong character and nothing reports it.
        let cases = [
            ('!', "shift-1"),
            ('@', "shift-2"),
            ('#', "shift-3"),
            ('$', "shift-4"),
            ('%', "shift-5"),
            ('^', "shift-6"),
            ('&', "shift-7"),
            ('*', "shift-8"),
            ('(', "shift-9"),
            (')', "shift-0"),
            ('_', "shift-minus"),
            ('+', "shift-equal"),
            ('{', "shift-bracket_left"),
            ('}', "shift-bracket_right"),
            ('|', "shift-backslash"),
            (':', "shift-semicolon"),
            ('"', "shift-apostrophe"),
            ('~', "shift-grave_accent"),
            ('<', "shift-comma"),
            ('>', "shift-dot"),
            ('?', "shift-slash"),
        ];
        for (ch, expected) in cases {
            assert_eq!(key_for(ch).unwrap(), expected, "for {ch:?}");
        }
    }

    #[test]
    fn test_named_punctuation() {
        let cases = [
            (' ', "spc"),
            ('\n', "ret"),
            ('\r', "ret"),
            ('\t', "tab"),
            ('-', "minus"),
            ('=', "equal"),
            ('[', "bracket_left"),
            (']', "bracket_right"),
            (';', "semicolon"),
            ('\'', "apostrophe"),
            ('`', "grave_accent"),
            ('\\', "backslash"),
            (',', "comma"),
            ('.', "dot"),
            ('/', "slash"),
        ];
        for (ch, expected) in cases {
            assert_eq!(key_for(ch).unwrap(), expected, "for {ch:?}");
        }
    }

    #[test]
    fn test_unmappable_characters_error_rather_than_being_dropped() {
        // Non-ASCII, whitespace that is not tab/newline, and emoji all have no
        // HMP name. Each must surface as Err; a silent skip is a password one
        // character short and a lockout counter.
        for ch in ['\u{00e9}', '\u{4e2d}', '\u{1f600}', '\u{00a0}', '\u{200b}'] {
            let err = key_for(ch).unwrap_err();
            assert_eq!(err.code_point, Some(ch as u32));
            let text = format!("{err}");
            assert!(
                text.contains(&format!("U+{:04X}", ch as u32)),
                "error text should name the code point: {text}"
            );
            // `Display` embeds the character via `Debug`, so a non-printable
            // one is escaped rather than emitted literally.
            assert!(
                text.contains(&ch.escape_debug().to_string()),
                "error text should echo the character: {text}"
            );
        }
    }

    #[test]
    fn test_error_message_is_actionable() {
        let text = format!("{}", key_for('\u{00e9}').unwrap_err());
        assert!(text.contains("Extend the key table"), "got: {text}");
    }

    #[test]
    fn test_no_printable_char_collides_with_another() {
        // Two characters mapping to the same HMP name means one of them types
        // the wrong thing. The only legal exception is \n and \r, which really
        // are the same key.
        let mut seen: std::collections::HashMap<&str, char> = std::collections::HashMap::new();
        for ch in printable_ascii() {
            let name = key_for(ch).unwrap();
            if let Some(previous) = seen.insert(name, ch) {
                assert_eq!(
                    (previous, ch),
                    ('\n', '\r'),
                    "{name:?} is claimed by both {previous:?} and {ch:?}"
                );
            }
        }
    }

    #[test]
    fn test_control_keys_resolve_case_insensitively() {
        assert_eq!(control_key("ret"), Some("ret"));
        assert_eq!(control_key("ENTER"), Some("ret"));
        assert_eq!(control_key("Escape"), Some("esc"));
        assert_eq!(control_key("BackSpace"), Some("backspace"));
        assert_eq!(control_key("ctrl"), None, "ctrl is not a table entry");
    }

    #[test]
    fn test_control_key_names_are_complete() {
        let names = control_key_names();
        for expected in [
            "ret", "enter", "tab", "esc", "escape", "backspace", "up", "down", "left", "right",
            "home", "end", "delete",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn test_resolve_sendkey_accepts_characters_and_names() {
        assert_eq!(resolve_sendkey("a", no_mods()).unwrap(), "a");
        assert_eq!(resolve_sendkey("A", no_mods()).unwrap(), "shift-a");
        assert_eq!(resolve_sendkey("!", no_mods()).unwrap(), "shift-1");
        assert_eq!(resolve_sendkey("ret", no_mods()).unwrap(), "ret");
        assert_eq!(resolve_sendkey("Escape", no_mods()).unwrap(), "esc");
        assert_eq!(resolve_sendkey(" ", no_mods()).unwrap(), "spc");
    }

    #[test]
    fn test_resolve_sendkey_composes_chords() {
        let mods = ModifierKeys {
            ctrl: true,
            alt: true,
            shift: false,
            super_key: false,
        };
        assert_eq!(resolve_sendkey("Delete", mods).unwrap(), "ctrl-alt-delete");
        let super_only = ModifierKeys {
            ctrl: false,
            alt: false,
            shift: false,
            super_key: true,
        };
        assert_eq!(resolve_sendkey("a", super_only).unwrap(), "meta-a");
    }

    #[test]
    fn test_resolve_sendkey_never_double_shifts() {
        // "A" is already shift-a; adding the shift modifier must not produce
        // shift-shift-a, which HMP rejects.
        let shifted = ModifierKeys {
            ctrl: false,
            alt: false,
            shift: true,
            super_key: false,
        };
        assert_eq!(resolve_sendkey("A", shifted).unwrap(), "shift-a");
        assert_eq!(resolve_sendkey("!", shifted).unwrap(), "shift-1");
    }

    #[test]
    fn test_resolve_sendkey_rejects_unknown_input() {
        assert!(resolve_sendkey("", no_mods()).is_err());
        assert!(resolve_sendkey("f13", no_mods()).is_err());
        assert!(resolve_sendkey("\u{00e9}", no_mods()).is_err());
        let err = resolve_sendkey("f13", no_mods()).unwrap_err();
        assert!(format!("{err}").contains("ret"), "should list valid names");
    }

    #[test]
    fn test_to_axis_spans_the_tablet_range() {
        let max = crate::capture_qmp::TABLET_AXIS_MAX;
        assert_eq!(to_axis(0, 1920), 0.0);
        assert_eq!(to_axis(1919, 1920), max);
        let mid = to_axis(959, 1920);
        assert!(mid > 16000.0 && mid < 17000.0, "midpoint was {mid}");
    }

    #[test]
    fn test_to_axis_clamps_rather_than_wrapping() {
        let max = crate::capture_qmp::TABLET_AXIS_MAX;
        assert_eq!(
            to_axis(-5000, 800),
            0.0,
            "negative must clamp to the left edge, not wrap"
        );
        assert_eq!(
            to_axis(999_999, 800),
            max,
            "past the right edge must clamp, not wrap"
        );
    }

    #[test]
    fn test_to_axis_survives_a_degenerate_extent() {
        // A zero-width surface would divide by zero.
        assert_eq!(to_axis(10, 0), 0.0);
        assert!(to_axis(10, 1).is_finite());
    }

    #[test]
    fn test_button_names_match_qmp() {
        assert_eq!(button_name(MouseButton::Left), "left");
        assert_eq!(button_name(MouseButton::Right), "right");
        assert_eq!(button_name(MouseButton::Middle), "middle");
    }

    #[test]
    fn test_abs_event_shape() {
        let event = abs_event("x", 16384.0);
        assert_eq!(event["type"], "abs");
        assert_eq!(event["data"]["axis"], "x");
        assert_eq!(event["data"]["value"], 16384.0);
    }

    #[test]
    fn test_button_event_shape() {
        let down = button_event(MouseButton::Left, true);
        assert_eq!(down["type"], "btn");
        assert_eq!(down["data"]["down"], true);
        assert_eq!(down["data"]["button"], "left");
        let up = button_event(MouseButton::Right, false);
        assert_eq!(up["data"]["down"], false);
        assert_eq!(up["data"]["button"], "right");
    }

    #[test]
    fn test_wheel_event_carries_sign_in_the_axis_name() {
        let up = wheel_event("up", 3u32);
        let down = wheel_event("down", 3u32);
        assert_eq!(up["data"]["value"], 3);
        assert_eq!(down["data"]["value"], 3);
        assert_ne!(up["data"]["axis"], down["data"]["axis"]);
    }

    #[test]
    fn test_default_key_delay_matches_the_empirical_finding() {
        assert_eq!(DEFAULT_KEY_DELAY, Duration::from_millis(50));
    }

    #[test]
    fn test_core_to_transport_event_conversion() {
        let core = RemoteInputEvent {
            action: InputAction::KeyPress,
            x: Some(-3),
            y: None,
            button: Some(MouseButton::Middle),
            key: Some("a".into()),
            modifiers: Some(ModifierKeys {
                ctrl: true,
                alt: false,
                shift: true,
                super_key: false,
            }),
            scroll_x: None,
            scroll_y: Some(-1.0),
            monitor_id: Some(2),
        };
        let wire = core_event_to_transport(&core);
        assert_eq!(wire.x, Some(-3));
        assert!(wire.y.is_none());
        assert!(matches!(wire.action, crate::types::InputAction::KeyPress));
        assert!(matches!(
            wire.button,
            Some(crate::types::MouseButton::Middle)
        ));
        let mods = wire.modifiers.unwrap();
        assert!(mods.ctrl && mods.shift && !mods.alt);
        assert_eq!(wire.scroll_y, Some(-1.0));
        assert_eq!(wire.monitor_id, Some(2));
    }

    #[test]
    fn test_all_eight_actions_survive_the_conversion_round_trip() {
        let actions = [
            InputAction::MouseMove,
            InputAction::MouseClick,
            InputAction::MouseDown,
            InputAction::MouseUp,
            InputAction::MouseScroll,
            InputAction::KeyPress,
            InputAction::KeyDown,
            InputAction::KeyUp,
        ];
        assert_eq!(actions.len(), 8);
        for action in actions {
            let core = RemoteInputEvent {
                action,
                x: None,
                y: None,
                button: None,
                key: None,
                modifiers: None,
                scroll_x: None,
                scroll_y: None,
                monitor_id: None,
            };
            let wire = core_event_to_transport(&core);
            let json = serde_json::to_string(&wire).unwrap();
            let back: crate::types::RemoteInputEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back.action, wire.action);
        }
    }
}
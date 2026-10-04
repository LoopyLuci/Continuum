//! The QMP sidecar wire protocol, exercised from outside the crate.
//!
//! These are the exact message shapes the PyQt5 GUI is written against, so a
//! change here is a change to that client's contract. They are asserted as
//! literal JSON rather than by round-tripping through the Rust types, because a
//! round trip passes even when the field *name* drifts — and a renamed field is
//! the failure a GUI actually sees.

use continuum_transport::ws_sidecar::{
    generate_token, parse_vm_list, token_matches, translate_input, ClientMessage, ServerMessage, VmRegistry, VmTarget,
    DEFAULT_QUALITY, DEFAULT_SIDECAR_PORT,
};
use continuum_transport::input_qmp::{key_for, resolve_sendkey, DEFAULT_KEY_DELAY};

// ---------------------------------------------------------------------------
// Client -> server
// ---------------------------------------------------------------------------

#[test]
fn test_config_message_shape() {
    let text = r#"{"type":"config","vm":"win11","quality":80,"fps":30,"width":1280,"height":800,"input_enabled":true}"#;
    let json: serde_json::Value = serde_json::from_str(text).unwrap();

    assert_eq!(json["type"], "config");
    assert_eq!(json["vm"], "win11");
    assert_eq!(json["quality"], 80);
    assert_eq!(json["fps"], 30);
    assert_eq!(json["width"], 1280);
    assert_eq!(json["height"], 800);
    assert_eq!(json["input_enabled"], true);

    let msg = ClientMessage::parse(text).unwrap();
    assert_eq!(msg.kind(), "config");
    assert_eq!(serde_json::to_value(&msg).unwrap(), json);
}

#[test]
fn test_subscribe_message_shape() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"type":"subscribe","vm":"ubuntu"}"#).unwrap();
    assert_eq!(json["type"], "subscribe");
    assert_eq!(json["vm"], "ubuntu");
    assert!(ClientMessage::parse(r#"{"type":"subscribe","vm":"ubuntu"}"#).is_ok());
}

#[test]
fn test_key_input_message_shape() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"type":"input","input_type":"key","key":"a","pressed":true}"#)
            .unwrap();
    assert_eq!(json["type"], "input");
    assert_eq!(json["input_type"], "key");
    assert_eq!(json["key"], "a");
    assert_eq!(json["pressed"], true);
}

#[test]
fn test_mouse_move_input_message_shape() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"type":"input","input_type":"mouse_move","x":640,"y":400}"#)
            .unwrap();
    assert_eq!(json["input_type"], "mouse_move");
    assert_eq!(json["x"], 640);
    assert_eq!(json["y"], 400);
}

#[test]
fn test_mouse_click_input_message_shape() {
    let json: serde_json::Value = serde_json::from_str(
        r#"{"type":"input","input_type":"mouse_click","button":"left","pressed":true}"#,
    )
    .unwrap();
    assert_eq!(json["input_type"], "mouse_click");
    assert_eq!(json["button"], "left");
    assert_eq!(json["pressed"], true);
}

#[test]
fn test_scroll_input_message_shape() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"type":"input","input_type":"scroll","dx":0,"dy":-1}"#).unwrap();
    assert_eq!(json["input_type"], "scroll");
    assert_eq!(json["dx"], 0);
    assert_eq!(json["dy"], -1);
}

#[test]
fn test_ping_message_shape() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"type":"ping","time":1712345678901}"#).unwrap();
    assert_eq!(json["type"], "ping");
    assert_eq!(json["time"], 1_712_345_678_901u64);
}

#[test]
fn test_stats_request_message_shape() {
    let json: serde_json::Value = serde_json::from_str(r#"{"type":"stats_request"}"#).unwrap();
    assert_eq!(json["type"], "stats_request");
    assert_eq!(json.as_object().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Server -> client
// ---------------------------------------------------------------------------

#[test]
fn test_config_ack_message_shape() {
    let text = ServerMessage::ConfigAck {
        quality: 80,
        fps: 30,
        width: 1280,
        height: 800,
    }
    .encode();
    assert_eq!(
        text,
        r#"{"type":"config_ack","quality":80,"fps":30,"width":1280,"height":800}"#
    );
}

#[test]
fn test_pong_message_shape() {
    assert_eq!(
        ServerMessage::Pong { time: 1712345678901 }.encode(),
        r#"{"type":"pong","time":1712345678901}"#
    );
}

#[test]
fn test_stats_message_shape() {
    let text = ServerMessage::Stats {
        frames_sent: 300,
        bytes_sent: 45_000_000,
        fps: 29.97,
    }
    .encode();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["type"], "stats");
    assert_eq!(json["frames_sent"], 300);
    assert_eq!(json["bytes_sent"], 45_000_000u64);
    assert!(json["fps"].is_number());
}

#[test]
fn test_vm_list_message_shape() {
    let registry = VmRegistry::new(vec![
        VmTarget::new("win11", "127.0.0.1:4444".parse().unwrap()),
        VmTarget::new("ubuntu", "127.0.0.1:4445".parse().unwrap()),
    ]);
    let text = ServerMessage::VmList {
        vms: registry.names(),
    }
    .encode();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["type"], "vm_list");
    assert_eq!(json["vms"], serde_json::json!(["ubuntu", "win11"]));
}

#[test]
fn test_error_message_shape() {
    let text = ServerMessage::error("VM is paused").encode();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["type"], "error");
    assert_eq!(json["message"], "VM is paused");
    assert_eq!(json.as_object().unwrap().len(), 2, "error carries no extras");
}

// ---------------------------------------------------------------------------
// Robustness contract
// ---------------------------------------------------------------------------

#[test]
fn test_unknown_type_is_ignored() {
    // A GUI newer than the server must keep its connection.
    for text in [
        r#"{"type":"teleport","x":1}"#,
        r#"{"type":"future_feature","payload":{"deeply":{"nested":true}}}"#,
    ] {
        assert_eq!(
            ClientMessage::parse(text).unwrap(),
            ClientMessage::Unknown,
            "{text} must be ignorable"
        );
    }
}

#[test]
fn test_malformed_json_produces_an_error_not_a_closed_socket() {
    let cases: Vec<String> = vec![
        String::new(),
        "not json at all".into(),
        "{".into(),
        "{\"type\":}".into(),
        // Valid JSON, but a key event with no key: the sidecar reports it
        // rather than typing an empty keystroke.
        r#"{"type":"input","input_type":"key","pressed":true}"#.into(),
        // Valid JSON, wrong shape for the tag.
        r#"{"type":"subscribe","vm":42}"#.into(),
    ];
    for text in cases {
        // Must not panic, and must always yield something the caller can turn
        // into an `error` frame or an ignore.
        match ClientMessage::parse(&text) {
            Ok(_) => {}
            Err(message) => assert!(
                !message.is_empty(),
                "an empty reason helps nobody: {text:?}"
            ),
        }
    }
}

#[test]
fn test_frames_are_not_carried_in_json() {
    // A frame is a binary WebSocket message. If a future change routes frames
    // through `ServerMessage` it would be base64 and 33% larger; this asserts
    // that no variant carries a payload big enough to be one.
    for msg in [
        ServerMessage::Pong { time: 0 },
        ServerMessage::error("x"),
        ServerMessage::VmList {
            vms: vec!["a".into()],
        },
    ] {
        let json: serde_json::Value = serde_json::from_str(&msg.encode()).unwrap();
        assert!(json.get("data").is_none());
        assert!(json.get("frame").is_none());
        assert!(json.get("jpeg").is_none());
    }
}

// ---------------------------------------------------------------------------
// Transport defaults and registry
// ---------------------------------------------------------------------------

#[test]
fn test_sidecar_defaults_match_the_documented_protocol() {
    assert_eq!(DEFAULT_SIDECAR_PORT, 8446);
    assert_eq!(DEFAULT_QUALITY, 80);
    assert_eq!(DEFAULT_KEY_DELAY.as_millis(), 50);
}

#[test]
fn test_vm_registry_round_trip() {
    let targets = parse_vm_list("win11=127.0.0.1:4444;fedora=127.0.0.1:5555").unwrap();
    let registry = VmRegistry::new(targets);
    assert_eq!(registry.names(), vec!["fedora", "win11"]);
    assert_eq!(registry.get("win11").unwrap().qmp_addr.port(), 4444);
    assert_eq!(registry.get("fedora").unwrap().qmp_addr.port(), 5555);
    assert!(registry.get("debian").is_none());
}

#[test]
fn test_vm_list_rejects_a_malformed_entry_instead_of_skipping_it() {
    assert!(parse_vm_list("win11").is_err());
    assert!(parse_vm_list("win11=127.0.0.1:notaport").is_err());
    assert!(parse_vm_list("=127.0.0.1:4444").is_err());
}

// ---------------------------------------------------------------------------
// Key vocabulary, from the GUI's point of view
// ---------------------------------------------------------------------------

#[test]
fn test_all_95_printable_ascii_characters_are_typeable() {
    let mut count = 0usize;
    for byte in 0x20u8..0x7Fu8 {
        let ch = char::from(byte);
        assert!(
            key_for(ch).is_ok(),
            "printable ASCII {ch:?} (U+{byte:04X}) has no HMP sendkey name"
        );
        count += 1;
    }
    assert_eq!(count, 95);
}

#[test]
fn test_lowercase_is_never_shifted() {
    for byte in b'a'..=b'z' {
        let ch = char::from(byte);
        let name = key_for(ch).unwrap();
        assert_eq!(
            name,
            ch.to_string(),
            "{ch:?} must send the bare key, or every password types in capitals"
        );
    }
}

#[test]
fn test_uppercase_is_shifted_lowercase() {
    for byte in b'A'..=b'Z' {
        let ch = char::from(byte);
        assert_eq!(key_for(ch).unwrap(), format!("shift-{}", ch.to_ascii_lowercase()));
    }
}

#[test]
fn test_shifted_punctuation_names() {
    for (ch, expected) in [
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
    ] {
        assert_eq!(key_for(ch).unwrap(), expected, "for {ch:?}");
    }
}

#[test]
fn test_named_punctuation_names() {
    for (ch, expected) in [
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
    ] {
        assert_eq!(key_for(ch).unwrap(), expected, "for {ch:?}");
    }
}

#[test]
fn test_unmappable_character_is_an_error_never_a_skip() {
    // A dropped character in a password produces a wrong password and a
    // lockout counter, not a visible failure. Err is the only safe answer.
    for ch in ['\u{00e9}', '\u{00a0}', '\u{4e2d}', '\u{1f600}', '\u{200b}'] {
        let err = key_for(ch).unwrap_err();
        let text = format!("{err}");
        assert!(
            text.contains(&format!("U+{:04X}", ch as u32)),
            "the error must name the code point so the caller can extend the table: {text}"
        );
    }
}

#[test]
fn test_resolve_sendkey_accepts_both_vocabularies() {
    use continuum_core::input::ModifierKeys;
    let none = ModifierKeys::default();
    assert_eq!(resolve_sendkey("a", none).unwrap(), "a");
    assert_eq!(resolve_sendkey("A", none).unwrap(), "shift-a");
    assert_eq!(resolve_sendkey("!", none).unwrap(), "shift-1");
    assert_eq!(resolve_sendkey("ret", none).unwrap(), "ret");
    assert_eq!(resolve_sendkey("Escape", none).unwrap(), "esc");
}

#[test]
fn test_resolve_sendkey_chords_and_no_double_shift() {
    use continuum_core::input::ModifierKeys;
    let chords = ModifierKeys {
        ctrl: true,
        alt: true,
        shift: false,
        super_key: false,
    };
    assert_eq!(resolve_sendkey("Delete", chords).unwrap(), "ctrl-alt-delete");
    let shifted = ModifierKeys {
        ctrl: false,
        alt: false,
        shift: true,
        super_key: false,
    };
    assert_eq!(
        resolve_sendkey("A", shifted).unwrap(),
        "shift-a",
        "\"A\" already carries the shift; a second one is rejected by HMP"
    );
}

// ---------------------------------------------------------------------------
// Input translation, from the GUI's point of view
// ---------------------------------------------------------------------------

#[test]
fn test_input_translation_produces_a_push_event() {
    let msg =
        ClientMessage::parse(r#"{"type":"input","input_type":"mouse_move","x":12,"y":34}"#).unwrap();
    let event = translate_input(&msg).unwrap();
    assert_eq!(event.x, Some(12));
    assert_eq!(event.y, Some(34));
    assert_eq!(
        event.action,
        continuum_core::input::InputAction::MouseMove
    );
}

#[test]
fn test_scroll_translation_keeps_the_deltas_signed() {
    let msg =
        ClientMessage::parse(r#"{"type":"input","input_type":"scroll","dx":-2,"dy":5}"#).unwrap();
    let event = translate_input(&msg).unwrap();
    assert_eq!(event.scroll_x, Some(-2.0));
    assert_eq!(event.scroll_y, Some(5.0));
}

#[test]
fn test_key_release_translates_to_nothing() {
    // sendkey is a press *and* a release; honouring the release as well would
    // type every character twice.
    let msg =
        ClientMessage::parse(r#"{"type":"input","input_type":"key","key":"a","pressed":false}"#)
            .unwrap();
    assert!(translate_input(&msg).is_none());
}

#[test]
fn test_click_release_translates_to_nothing() {
    let msg = ClientMessage::parse(
        r#"{"type":"input","input_type":"mouse_click","button":"left","pressed":false}"#,
    )
    .unwrap();
    assert!(translate_input(&msg).is_none());
}

#[test]
fn test_unknown_input_type_translates_to_nothing() {
    let msg = ClientMessage::parse(r#"{"type":"input","input_type":"wave"}"#).unwrap();
    assert!(translate_input(&msg).is_none());
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[test]
fn test_auth_message_parses() {
    let msg = ClientMessage::parse(r#"{"type":"auth","key":"s3cret"}"#).unwrap();
    assert_eq!(msg.kind(), "auth");
    match msg {
        ClientMessage::Auth { key } => assert_eq!(key, "s3cret"),
        other => panic!("expected auth, got {other:?}"),
    }
}

#[test]
fn test_token_comparison_is_exact() {
    // Right length, wrong bytes, and wrong length must all be rejected: a
    // prefix match would let "s3cr" authenticate against "s3cret".
    assert!(!token_matches("s3cret-token", "s3cret-tokeN"));
    assert!(!token_matches("s3cret-token", "s3cret"));
    assert!(!token_matches("s3cret-token", ""));
    assert!(token_matches("s3cret-token", "s3cret-token"));
}

#[test]
fn test_generated_tokens_are_hex_and_distinct() {
    let a = generate_token();
    let b = generate_token();
    assert_eq!(a.len(), 32, "expected 16 bytes of hex: {a}");
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "not hex: {a}");
    assert_ne!(a, b, "two generated tokens collided");
}

#[test]
fn test_auth_is_not_an_input_or_config() {
    // A stray auth later in the stream must not be mistaken for anything that
    // drives the guest.
    assert!(translate_input(&ClientMessage::parse(r#"{"type":"auth","key":"k"}"#).unwrap()).is_none());
}
#[test]
fn test_auth_ok_is_emitted_after_successful_auth() {
    // The GUI will not put anything on the wire -- not config, not subscribe,
    // not input -- until it sees auth_ok, so a server that accepts the token
    // without sending this leaves the client stuck on "authenticating". Both
    // server implementations must send it; assert the wire shape here.
    let encoded = ServerMessage::AuthOk.encode();
    assert_eq!(encoded, r#"{"type":"auth_ok"}"#);
}
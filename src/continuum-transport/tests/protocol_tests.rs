use continuum_transport::types::*;

#[test]
fn test_apq_stream_type_roundtrip() {
    assert_eq!(ApqStreamType::from(1), ApqStreamType::Media);
    assert_eq!(ApqStreamType::from(2), ApqStreamType::Intent);
    assert_eq!(ApqStreamType::from(3), ApqStreamType::Clipboard);
    assert_eq!(ApqStreamType::from(4), ApqStreamType::FileTransfer);
    assert_eq!(ApqStreamType::from(5), ApqStreamType::Audio);
    assert_eq!(ApqStreamType::from(99), ApqStreamType::Media);
}

#[test]
fn test_frame_semantics_serialization() {
    let sem = FrameSemantics {
        content_type: ContentType::Jpeg,
        width: 1920,
        height: 1080,
        quality: 85,
        frame_number: 42,
        timestamp: chrono::Utc::now(),
        is_keyframe: true,
        monitor_id: 0,
        encode_time_us: 5000,
    };

    let json = serde_json::to_string(&sem).unwrap();
    let deserialized: FrameSemantics = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.width, 1920);
    assert_eq!(deserialized.height, 1080);
    assert_eq!(deserialized.quality, 85);
    assert_eq!(deserialized.frame_number, 42);
    assert!(deserialized.is_keyframe);
    assert_eq!(deserialized.monitor_id, 0);
    assert_eq!(deserialized.encode_time_us, 5000);
}

#[test]
fn test_pairing_handshake_serialization() {
    let handshake = PairingHandshake {
        pairing_code: "test123".to_string(),
        client_name: "Test Client".to_string(),
        protocol_version: PROTOCOL_VERSION,
        client_e2e_public: Vec::new(),
        pake_encrypted_key: Vec::new(),
    };

    let json = serde_json::to_string(&handshake).unwrap();
    let deserialized: PairingHandshake = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.pairing_code, "test123");
    assert_eq!(deserialized.client_name, "Test Client");
    assert_eq!(deserialized.protocol_version, PROTOCOL_VERSION);
}

#[test]
fn test_remote_input_event_serialization() {
    let event = RemoteInputEvent {
        action: InputAction::MouseMove,
        x: Some(500),
        y: Some(300),
        button: None,
        key: None,
        modifiers: None,
        scroll_x: None,
        scroll_y: None,
        monitor_id: Some(0),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: RemoteInputEvent = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.action, InputAction::MouseMove);
    assert_eq!(deserialized.x, Some(500));
    assert_eq!(deserialized.y, Some(300));
}

#[test]
fn test_modifier_keys_default() {
    let mods = ModifierKeys::default();
    assert!(!mods.ctrl);
    assert!(!mods.alt);
    assert!(!mods.shift);
    assert!(!mods.super_key);
}

#[test]
fn test_permissions_default() {
    let perms = Permissions::default();
    assert!(perms.can_view);
    assert!(perms.can_control);
    assert!(perms.can_clipboard);
    assert!(perms.can_file_transfer);
    assert!(perms.can_audio);
}

#[test]
fn test_permissions_view_only() {
    let perms = Permissions::view_only();
    assert!(perms.can_view);
    assert!(!perms.can_control);
    assert!(!perms.can_clipboard);
    assert!(!perms.can_file_transfer);
    assert!(!perms.can_audio);
}

#[test]
fn test_intent_message_pairing_serialization() {
    let msg = IntentMessage::Pairing(PairingHandshake {
        pairing_code: "abc".to_string(),
        client_name: "test".to_string(),
        protocol_version: PROTOCOL_VERSION,
        client_e2e_public: Vec::new(),
        pake_encrypted_key: Vec::new(),
    });

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"pairing\""));
    assert!(json.contains("\"pairing_code\":\"abc\""));
}

#[test]
fn test_intent_message_input_serialization() {
    let msg = IntentMessage::Input(RemoteInputEvent {
        action: InputAction::KeyPress,
        x: None,
        y: None,
        button: None,
        key: Some("enter".to_string()),
        modifiers: Some(ModifierKeys {
            ctrl: true,
            alt: false,
            shift: false,
            super_key: false,
        }),
        scroll_x: None,
        scroll_y: None,
        monitor_id: None,
    });

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"input\""));
    assert!(json.contains("\"key_press\""));
}

#[test]
fn test_connection_status_display() {
    assert_eq!(
        format!("{}", ConnectionStatus::Disconnected),
        "Disconnected"
    );
    assert_eq!(format!("{}", ConnectionStatus::Connecting), "Connecting");
    assert_eq!(format!("{}", ConnectionStatus::Streaming), "Streaming");
}

#[test]
fn test_content_type_serialization() {
    let jpeg = ContentType::Jpeg;
    let json = serde_json::to_string(&jpeg).unwrap();
    assert_eq!(json, "\"image/jpeg\"");

    let png = ContentType::Png;
    let json = serde_json::to_string(&png).unwrap();
    assert_eq!(json, "\"image/png\"");
}

#[test]
fn test_mouse_button_serialization() {
    assert_eq!(
        serde_json::to_string(&MouseButton::Left).unwrap(),
        "\"left\""
    );
    assert_eq!(
        serde_json::to_string(&MouseButton::Right).unwrap(),
        "\"right\""
    );
    assert_eq!(
        serde_json::to_string(&MouseButton::Middle).unwrap(),
        "\"middle\""
    );
}

#[test]
fn test_intent_response_pairing_result() {
    let response = IntentResponse::PairingResult(PairingResponse {
        accepted: true,
        message: "OK".to_string(),
        session_token: Some("token-123".to_string()),
        permissions: Permissions::default(),
        server_e2e_public: Vec::new(),
        pake_encrypted_key: Vec::new(),
        resume_token: None,
        sas_words: Vec::new(),
    });

    let json = serde_json::to_vec(&response).unwrap();
    let parsed: IntentResponse = serde_json::from_slice(&json).unwrap();

    match parsed {
        IntentResponse::PairingResult(r) => {
            assert!(r.accepted);
            assert_eq!(r.message, "OK");
            assert_eq!(r.session_token, Some("token-123".to_string()));
        }
        _ => panic!("Expected PairingResult"),
    }
}

#[test]
fn test_intent_response_error() {
    let response = IntentResponse::Error(ErrorMessage {
        code: 404,
        message: "Not found".to_string(),
    });

    let json = serde_json::to_vec(&response).unwrap();
    let parsed: IntentResponse = serde_json::from_slice(&json).unwrap();

    match parsed {
        IntentResponse::Error(e) => {
            assert_eq!(e.code, 404);
            assert_eq!(e.message, "Not found");
        }
        _ => panic!("Expected Error"),
    }
}

#[test]
fn test_error_message_display() {
    let err = ErrorMessage {
        code: 500,
        message: "Internal error".to_string(),
    };
    assert_eq!(format!("{}", err), "[500] Internal error");
}

#[test]
fn test_full_protocol_flow() {
    let handshake = PairingHandshake {
        pairing_code: "test-code".to_string(),
        client_name: "Integration Test".to_string(),
        protocol_version: PROTOCOL_VERSION,
        client_e2e_public: Vec::new(),
        pake_encrypted_key: Vec::new(),
    };

    let msg = IntentMessage::Pairing(handshake);
    let json = serde_json::to_vec(&msg).unwrap();
    let parsed: IntentMessage = serde_json::from_slice(&json).unwrap();

    match parsed {
        IntentMessage::Pairing(h) => {
            assert_eq!(h.pairing_code, "test-code");
            assert_eq!(h.client_name, "Integration Test");
        }
        _ => panic!("Expected Pairing message"),
    }
}

#[test]
fn test_input_event_with_all_fields() {
    let event = RemoteInputEvent {
        action: InputAction::KeyPress,
        x: Some(100),
        y: Some(200),
        button: Some(MouseButton::Right),
        key: Some("a".to_string()),
        modifiers: Some(ModifierKeys {
            ctrl: true,
            alt: true,
            shift: true,
            super_key: true,
        }),
        scroll_x: Some(1.5),
        scroll_y: Some(-2.0),
        monitor_id: Some(0),
    };

    let json = serde_json::to_vec(&event).unwrap();
    let parsed: RemoteInputEvent = serde_json::from_slice(&json).unwrap();

    assert_eq!(parsed.action, InputAction::KeyPress);
    assert_eq!(parsed.x, Some(100));
    assert_eq!(parsed.y, Some(200));
    assert_eq!(parsed.button, Some(MouseButton::Right));
    assert_eq!(parsed.key, Some("a".to_string()));
    assert!(parsed.modifiers.as_ref().unwrap().ctrl);
    assert!(parsed.modifiers.as_ref().unwrap().alt);
    assert!(parsed.modifiers.as_ref().unwrap().shift);
    assert!(parsed.modifiers.as_ref().unwrap().super_key);
    assert_eq!(parsed.scroll_x, Some(1.5));
    assert_eq!(parsed.scroll_y, Some(-2.0));
    assert_eq!(parsed.monitor_id, Some(0));
}

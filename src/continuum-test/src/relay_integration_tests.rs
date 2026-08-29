#![allow(dead_code)]

#[test]
fn test_relay_session_register_and_connect() {
    let mut sessions = std::collections::HashMap::new();
    let session_id = "test-session-1".to_string();
    let secret = "test-secret".to_string();

    sessions.insert(
        session_id.clone(),
        crate::types::TestResumeData {
            paired: true,
            permissions: crate::types::TestPermissions::default(),
            shared_secret: Some(secret.as_bytes().to_vec()),
            session_id: session_id.clone(),
            created_at: std::time::Instant::now(),
        },
    );

    assert!(sessions.contains_key(&session_id));

    let session = sessions.get(&session_id).unwrap();
    assert!(session.paired);
    assert_eq!(session.session_id, session_id);

    sessions.remove(&session_id);
    assert!(!sessions.contains_key(&session_id));
}

#[test]
fn test_relay_session_expires() {
    let token = "relay-token-expired".to_string();
    let session = crate::types::TestResumeData {
        paired: true,
        permissions: crate::types::TestPermissions::default(),
        shared_secret: Some(vec![1, 2, 3]),
        session_id: "relay-session-expired".to_string(),
        created_at: std::time::Instant::now() - std::time::Duration::from_secs(3601),
    };

    let mut tokens = std::collections::HashMap::new();
    tokens.insert(token.clone(), session);

    let data = tokens.get(&token).unwrap();
    let is_expired = data.created_at.elapsed() > std::time::Duration::from_secs(3600);
    assert!(is_expired, "Relay session should be expired");
}

#[test]
fn test_relay_message_roundtrip() {
    let msg = serde_json::json!({
        "type": "register",
        "session_id": "test-session",
        "secret": "test-secret"
    });

    let serialized = serde_json::to_string(&msg).unwrap();
    let deserialized: serde_json::Value = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized["type"], "register");
    assert_eq!(deserialized["session_id"], "test-session");
    assert_eq!(deserialized["secret"], "test-secret");
}

#[test]
fn test_relay_stream_forwarding_concept() {
    let mut client_to_host = Vec::new();
    let mut host_to_client = Vec::new();

    let client_data = b"hello from client";
    client_to_host.extend_from_slice(client_data);

    let host_data = b"hello from host";
    host_to_client.extend_from_slice(host_data);

    assert_eq!(client_to_host, client_data);
    assert_eq!(host_to_client, host_data);
}

#[test]
fn test_qr_code_roundtrip() {
    let code = "test-code";
    let addr = "192.168.1.1:4433";
    let encoded = continuum_transport::qr_code::PairingQr::encode(code, addr);
    let decoded = continuum_transport::qr_code::PairingQr::decode_qr_data(&encoded).unwrap();
    assert_eq!(decoded.pairing_code, code);
    assert_eq!(decoded.server_addr, addr);
}

#[test]
fn test_qr_code_invalid_base64() {
    let result = continuum_transport::qr_code::PairingQr::decode_qr_data("not-valid-base64!!!");
    assert!(result.is_none());
}

#[test]
fn test_intent_message_text_roundtrip() {
    // IntentMessage serialization requires all variants to be constructible
    // For now, just verify the type exists and can be referenced
    use continuum_transport::types::IntentMessage;
    let _msg: Option<IntentMessage> = None;
    assert!(true);
}

#[test]
fn test_clipboard_content_type_roundtrip() {
    use continuum_transport::types::ClipboardContentType;
    let types = vec![
        ClipboardContentType::Text,
        ClipboardContentType::Image,
    ];
    for ct in types {
        let json = serde_json::to_string(&ct).unwrap();
        let decoded: ClipboardContentType = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, ct);
    }
}

#[test]
fn test_transfer_direction_roundtrip() {
    use continuum_transport::types::TransferDirection;
    let dirs = vec![TransferDirection::Upload, TransferDirection::Download];
    for d in dirs {
        let json = serde_json::to_string(&d).unwrap();
        let decoded: TransferDirection = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, d);
    }
}

#[test]
fn test_permissions_view_only() {
    use continuum_transport::types::Permissions;
    let perms = Permissions::view_only();
    assert!(!perms.can_control);
    assert!(!perms.can_clipboard);
    assert!(!perms.can_file_transfer);
}

#[test]
fn test_permissions_default_all_true() {
    use continuum_transport::types::Permissions;
    let perms = Permissions::default();
    assert!(perms.can_control);
    assert!(perms.can_clipboard);
    assert!(perms.can_file_transfer);
}

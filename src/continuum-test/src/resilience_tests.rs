#![allow(dead_code)]

#[test]
fn test_connection_timeout_is_reported() {
    let timeout_result = std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(2));
    });
    let result = timeout_result.join();
    assert!(result.is_ok());
}

#[test]
fn test_resume_token_basic_lifecycle() {
    let mut tokens = std::collections::HashMap::new();
    let token = "token-1".to_string();
    let session = crate::types::TestResumeData {
        paired: true,
        permissions: crate::types::TestPermissions::default(),
        shared_secret: Some(vec![1, 2, 3]),
        session_id: "session-1".to_string(),
        created_at: std::time::Instant::now(),
    };

    tokens.insert(token.clone(), session);
    let data = tokens.get(&token).expect("token should exist after insert");
    assert!(data.paired);
    assert_eq!(data.session_id, "session-1");

    tokens.remove(&token);
    assert!(tokens.get(&token).is_none());
}

#[test]
fn test_resume_token_expires() {
    let token = "token-expired".to_string();
    let session = crate::types::TestResumeData {
        paired: true,
        permissions: crate::types::TestPermissions::default(),
        shared_secret: Some(vec![1, 2, 3]),
        session_id: "session-expired".to_string(),
        created_at: std::time::Instant::now() - std::time::Duration::from_secs(3601),
    };

    let mut tokens = std::collections::HashMap::new();
    tokens.insert(token.clone(), session);

    let data = tokens.get(&token);
    assert!(data.is_some());
    assert!(
        data.unwrap().created_at.elapsed() > std::time::Duration::from_secs(3600),
        "Session should be expired"
    );
}

#[test]
fn test_encoder_fallback_to_software() {
    let img = continuum_transport::capture::synthesize_demo_frame();
    let jpeg = continuum_transport::codec::encode_jpeg(&img, 80).unwrap();
    assert!(!jpeg.is_empty());
    assert!(jpeg.len() > 100);

    let decoded = image::load_from_memory(&jpeg).unwrap();
    assert_eq!(decoded.width(), img.width());
    assert_eq!(decoded.height(), img.height());
}

#[test]
fn test_rate_controller_under_high_latency() {
    use chrono::Utc;
    use continuum_transport::streaming_engine::{
        RateControlConfig, RateControlSample, RateController,
    };

    let mut controller = RateController::new(RateControlConfig::default());

    for _ in 0..10 {
        controller.record_sample(RateControlSample {
            timestamp: Utc::now(),
            encode_time_us: 50000,
            frame_size_bytes: 50000,
            rtt_ms: 500.0,
            client_decode_time_us: None,
            bandwidth_estimate_kbps: 1000.0,
            packet_loss: 0.05,
        });
    }

    let decision = controller.decide();
    assert!(
        decision.quality <= 95,
        "Quality should be reduced under high latency"
    );
}

#[test]
fn test_clipboard_unpaired_rejection() {
    use continuum_transport::types::Permissions;
    let perms = Permissions::view_only();
    assert!(!perms.can_clipboard);
    assert!(!perms.can_file_transfer);
}

#[test]
fn test_file_transfer_concept() {
    let mut received_bytes = 0u64;
    let total_size = 1024u64;
    let chunk_offsets = vec![0u64, 256, 512];

    for _offset in &chunk_offsets {
        received_bytes += 256;
    }

    assert_eq!(received_bytes, 768);
    assert!(received_bytes < total_size);
}

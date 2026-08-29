use crate::harness::TestHarness;
use crate::types::*;
use std::time::Duration;
use tokio::time::sleep;

pub async fn run_chaos_disconnect_reconnect(harness: &mut TestHarness) -> TestResult {
    let start = std::time::Instant::now();
    let mut assertions = Vec::new();
    let server_addr = "127.0.0.1:4499";

    let _ = harness.start_server(server_addr).await;

    // Connect
    let connect = harness.connect_client(server_addr, "test").await;
    assertions.push(AssertionResult {
        description: "Initial connection".to_string(),
        passed: connect.status == TestStatus::Passed,
        expected: "PASS".to_string(),
        actual: connect.status.to_string(),
        line: None,
    });

    // Restart server to simulate crash
    harness.stop_server();
    sleep(Duration::from_millis(500)).await;

    let _ = harness.start_server(server_addr).await;
    sleep(Duration::from_millis(1000)).await;

    // Reconnect
    let reconnect = harness.connect_client(server_addr, "test").await;
    assertions.push(AssertionResult {
        description: "Reconnection after server restart".to_string(),
        passed: reconnect.status == TestStatus::Passed,
        expected: "PASS".to_string(),
        actual: reconnect.status.to_string(),
        line: None,
    });

    let all_passed = assertions.iter().all(|a| a.passed);
    TestResult {
        name: "chaos-disconnect-reconnect".to_string(),
        category: TestCategory::Chaos,
        status: if all_passed {
            TestStatus::Passed
        } else {
            TestStatus::Failed
        },
        duration_ms: start.elapsed().as_millis() as u64,
        started_at: chrono::Utc::now(),
        finished_at: Some(chrono::Utc::now()),
        details: format!(
            "{}/{} chaos assertions passed",
            assertions.iter().filter(|a| a.passed).count(),
            assertions.len()
        ),
        metrics: vec![MetricSample {
            name: "reconnect_time_ms".to_string(),
            value: 0.0,
            unit: "ms".to_string(),
            percentile: None,
        }],
        assertions,
    }
}

pub async fn run_chaos_frame_stress(harness: &mut TestHarness) -> TestResult {
    let start = std::time::Instant::now();
    let server_addr = "127.0.0.1:4498";

    let _ = harness.start_server(server_addr).await;
    let _ = harness.connect_client(server_addr, "test").await;

    let frames = harness.stream_frames(50).await;

    let passed = frames.status == TestStatus::Passed;
    TestResult {
        name: "chaos-frame-stress".to_string(),
        category: TestCategory::Chaos,
        status: if passed {
            TestStatus::Passed
        } else {
            TestStatus::Failed
        },
        duration_ms: start.elapsed().as_millis() as u64,
        started_at: chrono::Utc::now(),
        finished_at: Some(chrono::Utc::now()),
        details: format!("Frame stress test: {}", frames.details),
        metrics: vec![MetricSample {
            name: "frames_stressed".to_string(),
            value: frames.metrics.first().map(|m| m.value).unwrap_or(0.0),
            unit: "frames".to_string(),
            percentile: None,
        }],
        assertions: frames.assertions,
    }
}

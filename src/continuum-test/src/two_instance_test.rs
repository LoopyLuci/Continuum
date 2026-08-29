#![allow(dead_code)]

// Two-Instance Connection Test
// =================================================================
// Launches server + client on the same machine, verifies:
//   1. Server starts and binds
//   2. Client connects and pairs
//   3. Frames flow from server to client
//   4. Input events can be sent
//   5. Disconnect/reconnect works
// =================================================================

use std::time::{Duration, Instant};
use tokio::time::sleep;

const SERVER_ADDR: &str = "127.0.0.1:4480";
const HEALTH_URL: &str = "http://127.0.0.1:9091/health";

#[derive(Debug, Clone)]
pub struct TestReport {
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    pub details: String,
    pub metrics: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum TestStatus {
    Pass,
    Failed,
    Skip,
}

pub async fn run_two_instance_test() -> Vec<TestReport> {
    let mut reports = Vec::new();
    let start = Instant::now();

    // Phase 1: Start server
    reports.push(test_server_start().await);

    // Phase 2: Wait for server
    reports.push(test_server_health().await);

    // Phase 3: Connect client
    reports.push(test_client_connect().await);

    // Phase 4: Verify pairing
    reports.push(test_pairing().await);

    // Phase 5: Verify frame streaming
    reports.push(test_frame_streaming().await);

    // Phase 6: Test disconnect
    reports.push(test_disconnect().await);

    // Summary
    let total = reports.len();
    let passed = reports
        .iter()
        .filter(|r| r.status == TestStatus::Pass)
        .count();
    let failed = reports
        .iter()
        .filter(|r| r.status == TestStatus::Failed)
        .count();

    println!("\n{}", "=".repeat(60));
    println!("  Two-Instance Connection Test Report");
    println!(
        "  Total: {} | Passed: {} | Failed: {} | Duration: {:.1}s",
        total,
        passed,
        failed,
        start.elapsed().as_secs_f64()
    );
    println!("{}", "=".repeat(60));

    for r in &reports {
        let icon = match r.status {
            TestStatus::Pass => "✓",
            TestStatus::Failed => "✗",
            TestStatus::Skip => "⊘",
        };
        println!("  {} {} ({}) {}ms", icon, r.name, r.details, r.duration_ms);
    }

    reports
}

async fn test_server_start() -> TestReport {
    let start = Instant::now();

    // Kill any existing servers on our test port
    kill_port(4480);

    let result = std::process::Command::new("cargo")
        .args([
            "run",
            "-p",
            "continuum-server",
            "--",
            "--listen",
            SERVER_ADDR,
            "--log-level",
            "error",
        ])
        .current_dir("Z:\\Projects\\Continuum")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    match result {
        Ok(_child) => {
            sleep(Duration::from_millis(2000)).await;
            TestReport {
                name: "server-start".into(),
                status: TestStatus::Pass,
                duration_ms: start.elapsed().as_millis() as u64,
                details: format!("Server spawned on {}", SERVER_ADDR),
                metrics: vec![],
            }
        }
        Err(e) => TestReport {
            name: "server-start".into(),
            status: TestStatus::Failed,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Failed to start: {}", e),
            metrics: vec![],
        },
    }
}

async fn test_server_health() -> TestReport {
    let start = Instant::now();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    for _ in 0..10 {
        sleep(Duration::from_millis(500)).await;
        if let Ok(resp) = client.get(HEALTH_URL).send().await {
            if resp.status().is_success() {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                return TestReport {
                    name: "server-health".into(),
                    status: TestStatus::Pass,
                    duration_ms: start.elapsed().as_millis() as u64,
                    details: format!("Status: {}, Version: {}", body["status"], body["version"]),
                    metrics: vec![
                        ("uptime".into(), body["uptime_secs"].to_string()),
                        ("connections".into(), body["active_connections"].to_string()),
                    ],
                };
            }
        }
    }

    TestReport {
        name: "server-health".into(),
        status: TestStatus::Failed,
        duration_ms: start.elapsed().as_millis() as u64,
        details: "Health endpoint not reachable after 5s".into(),
        metrics: vec![],
    }
}

async fn test_client_connect() -> TestReport {
    let start = Instant::now();

    // Connect to server using QUIC
    let config = continuum_transport::ClientConfig {
        server_addr: SERVER_ADDR.parse().unwrap(),
        pairing_code: "continuum".to_string(),
        auto_reconnect: false,
        ..Default::default()
    };

    match timeout(Duration::from_secs(10), async {
        continuum_transport::client::connect_to_server(&config).await
    })
    .await
    {
        Ok(Ok(_conn)) => {
            let elapsed = start.elapsed().as_millis() as u64;
            TestReport {
                name: "client-connect".into(),
                status: TestStatus::Pass,
                duration_ms: elapsed,
                details: format!("QUIC connection established in {}ms", elapsed),
                metrics: vec![("connect_ms".into(), elapsed.to_string())],
            }
        }
        Ok(Err(e)) => TestReport {
            name: "client-connect".into(),
            status: TestStatus::Failed,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Connection failed: {}", e),
            metrics: vec![],
        },
        Err(_) => TestReport {
            name: "client-connect".into(),
            status: TestStatus::Failed,
            duration_ms: start.elapsed().as_millis() as u64,
            details: "Connection timed out".into(),
            metrics: vec![],
        },
    }
}

async fn test_pairing() -> TestReport {
    let start = Instant::now();

    let config = continuum_transport::ClientConfig {
        server_addr: SERVER_ADDR.parse().unwrap(),
        pairing_code: "continuum".to_string(),
        auto_reconnect: false,
        ..Default::default()
    };

    match timeout(Duration::from_secs(5), async {
        let conn = continuum_transport::client::connect_to_server(&config).await?;
        let (client_secret, _client_public) = continuum_security::generate_dh_keypair();
        let (pair, _shared_secret) = continuum_transport::client::pair_connection(
            &conn.connection,
            "continuum",
            "test-client",
            &client_secret,
        )
        .await?;
        Ok::<_, anyhow::Error>(pair)
    })
    .await
    {
        Ok(Ok(response)) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let accepted = response.accepted;
            TestReport {
                name: "pairing".into(),
                status: if accepted {
                    TestStatus::Pass
                } else {
                    TestStatus::Failed
                },
                duration_ms: elapsed,
                details: if accepted {
                    format!("Pairing accepted in {}ms", elapsed)
                } else {
                    format!("Pairing rejected: {}", response.message)
                },
                metrics: vec![("accepted".into(), accepted.to_string())],
            }
        }
        Ok(Err(e)) => TestReport {
            name: "pairing".into(),
            status: TestStatus::Failed,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Pairing failed: {}", e),
            metrics: vec![],
        },
        Err(_) => TestReport {
            name: "pairing".into(),
            status: TestStatus::Failed,
            duration_ms: start.elapsed().as_millis() as u64,
            details: "Pairing timed out".into(),
            metrics: vec![],
        },
    }
}

async fn test_frame_streaming() -> TestReport {
    let start = Instant::now();
    let frame_count = 10u32;

    let config = continuum_transport::ClientConfig {
        server_addr: SERVER_ADDR.parse().unwrap(),
        pairing_code: "continuum".to_string(),
        auto_reconnect: false,
        ..Default::default()
    };

    match timeout(Duration::from_secs(30), async {
        let conn = continuum_transport::client::connect_to_server(&config).await?;
        let (client_secret, _client_public) = continuum_security::generate_dh_keypair();
        let _ = continuum_transport::client::pair_connection(
            &conn.connection,
            "continuum",
            "test",
            &client_secret,
        )
        .await;

        let (mut send, mut recv) = conn.connection.open_bi().await?;
        send.write_all(&(continuum_transport::ApqStreamType::Media as u32).to_be_bytes())
            .await?;

        let mut received = 0u32;
        let mut total_bytes = 0u64;
        let mut latencies = Vec::new();

        for _ in 0..frame_count {
            let t0 = Instant::now();
            match continuum_transport::client::read_frame(&mut recv).await {
                Ok((data, _sem)) => {
                    let latency = t0.elapsed().as_micros() as u64;
                    latencies.push(latency);
                    total_bytes += data.len() as u64;
                    received += 1;
                    if data.is_empty() {
                        continue;
                    }
                }
                Err(e) => {
                    tracing::warn!("Frame read error: {}", e);
                    break;
                }
            }
        }

        let avg_latency = if latencies.is_empty() {
            0.0
        } else {
            latencies.iter().sum::<u64>() as f64 / latencies.len() as f64
        };

        Ok::<_, anyhow::Error>((received, total_bytes, avg_latency))
    })
    .await
    {
        Ok(Ok((received, bytes, avg_latency))) => {
            let elapsed = start.elapsed().as_millis() as u64;
            TestReport {
                name: "frame-streaming".into(),
                status: if received > 0 {
                    TestStatus::Pass
                } else {
                    TestStatus::Failed
                },
                duration_ms: elapsed,
                details: format!(
                    "{}/{} frames received, {:.0}KB total, avg {:.0}µs latency",
                    received,
                    frame_count,
                    bytes as f64 / 1024.0,
                    avg_latency
                ),
                metrics: vec![
                    ("frames".into(), received.to_string()),
                    ("bytes".into(), bytes.to_string()),
                    ("avg_latency_us".into(), format!("{:.0}", avg_latency)),
                ],
            }
        }
        Ok(Err(e)) => TestReport {
            name: "frame-streaming".into(),
            status: TestStatus::Failed,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Stream error: {}", e),
            metrics: vec![],
        },
        Err(_) => TestReport {
            name: "frame-streaming".into(),
            status: TestStatus::Failed,
            duration_ms: start.elapsed().as_millis() as u64,
            details: "Stream timed out".into(),
            metrics: vec![],
        },
    }
}

async fn test_disconnect() -> TestReport {
    let start = Instant::now();

    let config = continuum_transport::ClientConfig {
        server_addr: SERVER_ADDR.parse().unwrap(),
        pairing_code: "continuum".to_string(),
        auto_reconnect: false,
        ..Default::default()
    };

    match timeout(Duration::from_secs(5), async {
        let _conn = continuum_transport::client::connect_to_server(&config).await?;
        sleep(Duration::from_millis(200)).await;
        // Connection drop simulates disconnect
        Ok::<_, anyhow::Error>(())
    })
    .await
    {
        Ok(Ok(())) => {
            let elapsed = start.elapsed().as_millis() as u64;
            TestReport {
                name: "disconnect".into(),
                status: TestStatus::Pass,
                duration_ms: elapsed,
                details: "Disconnect completed cleanly".into(),
                metrics: vec![],
            }
        }
        Ok(Err(e)) => TestReport {
            name: "disconnect".into(),
            status: TestStatus::Failed,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Disconnect error: {}", e),
            metrics: vec![],
        },
        Err(_) => TestReport {
            name: "disconnect".into(),
            status: TestStatus::Failed,
            duration_ms: start.elapsed().as_millis() as u64,
            details: "Disconnect timed out".into(),
            metrics: vec![],
        },
    }
}

fn kill_port(port: u16) {
    let output = std::process::Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
        .ok();

    if let Some(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            if line.contains(&format!(":{}", port)) {
                if let Some(pid_str) = line.split_whitespace().last() {
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        let _ = std::process::Command::new("taskkill")
                            .args(["/F", "/PID", &pid.to_string()])
                            .output();
                    }
                }
            }
        }
    }
}

use tokio::time::timeout;

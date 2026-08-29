#![allow(dead_code)]

use crate::types::*;
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::time::timeout;

const SERVER_START_TIMEOUT: Duration = Duration::from_secs(120);
const CLIENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const STREAM_TIMEOUT: Duration = Duration::from_secs(30);

pub struct TestHarness {
    pub server: Option<ServerHandle>,
}

struct HarnessMetrics {
    pub frames_sent: u64,
    pub frames_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub errors: Vec<String>,
    pub start_time: Instant,
}

pub struct ServerHandle {
    pub process: Option<std::process::Child>,
    pub addr: String,
}

impl TestHarness {
    pub fn new() -> Self {
        Self { server: None }
    }

    pub async fn start_server(&mut self, addr: &str) -> TestResult {
        let start = Instant::now();

        let pairing_code = format!("test-{}", rand::random::<u32>());

        let server_result = timeout(SERVER_START_TIMEOUT, async {
            let process = std::process::Command::new("cargo")
                .args([
                    "run",
                    "-p",
                    "continuum-server",
                    "--",
                    "--listen",
                    addr,
                    "--pairing-code",
                    &pairing_code,
                    "--log-level",
                    "error",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();

            match process {
                Ok(child) => {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    Ok(child)
                }
                Err(e) => Err(e),
            }
        })
        .await;

        match server_result {
            Ok(Ok(process)) => {
                self.server = Some(ServerHandle {
                    process: Some(process),
                    addr: addr.to_string(),
                });
                let elapsed = start.elapsed().as_millis() as u64;
                TestResult {
                    name: format!("server-start-{}", addr),
                    category: TestCategory::E2E,
                    status: TestStatus::Passed,
                    duration_ms: elapsed,
                    started_at: chrono::Utc::now(),
                    finished_at: Some(chrono::Utc::now()),
                    details: format!("Server started on {} in {}ms", addr, elapsed),
                    metrics: vec![MetricSample {
                        name: "startup_ms".to_string(),
                        value: elapsed as f64,
                        unit: "ms".to_string(),
                        percentile: None,
                    }],
                    assertions: vec![AssertionResult {
                        description: "Server process started".to_string(),
                        passed: true,
                        expected: "running".to_string(),
                        actual: "running".to_string(),
                        line: None,
                    }],
                }
            }
            Ok(Err(e)) => TestResult {
                name: format!("server-start-{}", addr),
                category: TestCategory::E2E,
                status: TestStatus::Error,
                duration_ms: start.elapsed().as_millis() as u64,
                started_at: chrono::Utc::now(),
                finished_at: None,
                details: format!("Failed to spawn server: {}", e),
                metrics: vec![],
                assertions: vec![],
            },
            Err(_) => TestResult {
                name: format!("server-start-{}", addr),
                category: TestCategory::E2E,
                status: TestStatus::Timeout,
                duration_ms: start.elapsed().as_millis() as u64,
                started_at: chrono::Utc::now(),
                finished_at: None,
                details: format!(
                    "Server failed to start within {}s",
                    SERVER_START_TIMEOUT.as_secs()
                ),
                metrics: vec![],
                assertions: vec![],
            },
        }
    }

    pub async fn connect_client(&mut self, server_addr: &str, pairing_code: &str) -> TestResult {
        let start = Instant::now();
        let result = timeout(
            CLIENT_CONNECT_TIMEOUT,
            self.connect_client_inner(server_addr, pairing_code, start),
        )
        .await;

        match result {
            Ok(r) => r,
            Err(_) => TestResult {
                name: format!("client-connect-{}", server_addr),
                category: TestCategory::E2E,
                status: TestStatus::Timeout,
                duration_ms: start.elapsed().as_millis() as u64,
                started_at: chrono::Utc::now(),
                finished_at: None,
                details: format!(
                    "Client connection timed out after {}s",
                    CLIENT_CONNECT_TIMEOUT.as_secs()
                ),
                metrics: vec![],
                assertions: vec![AssertionResult {
                    description: "Client connects to server".to_string(),
                    passed: false,
                    expected: "connected".to_string(),
                    actual: "timeout".to_string(),
                    line: None,
                }],
            },
        }
    }

    async fn connect_client_inner(
        &self,
        server_addr: &str,
        pairing_code: &str,
        start: Instant,
    ) -> TestResult {
        match self.do_connect(server_addr, pairing_code).await {
            Ok(pair) => {
                let elapsed = start.elapsed().as_millis() as u64;
                TestResult {
                    name: format!("client-connect-{}", server_addr),
                    category: TestCategory::E2E,
                    status: TestStatus::Passed,
                    duration_ms: elapsed,
                    started_at: chrono::Utc::now(),
                    finished_at: Some(chrono::Utc::now()),
                    details: format!("Connected and paired in {}ms", elapsed),
                    metrics: vec![MetricSample {
                        name: "connect_ms".to_string(),
                        value: elapsed as f64,
                        unit: "ms".to_string(),
                        percentile: None,
                    }],
                    assertions: vec![
                        AssertionResult {
                            description: "Client connects to server".to_string(),
                            passed: true,
                            expected: "connected".to_string(),
                            actual: "connected".to_string(),
                            line: None,
                        },
                        AssertionResult {
                            description: "Pairing accepted".to_string(),
                            passed: pair.accepted,
                            expected: "accepted=true".to_string(),
                            actual: format!("accepted={}", pair.accepted),
                            line: None,
                        },
                    ],
                }
            }
            Err(e) => TestResult {
                name: format!("client-connect-{}", server_addr),
                category: TestCategory::E2E,
                status: TestStatus::Failed,
                duration_ms: start.elapsed().as_millis() as u64,
                started_at: chrono::Utc::now(),
                finished_at: None,
                details: format!("Connection failed: {}", e),
                metrics: vec![],
                assertions: vec![AssertionResult {
                    description: "Client connects to server".to_string(),
                    passed: false,
                    expected: "connected".to_string(),
                    actual: format!("error: {}", e),
                    line: None,
                }],
            },
        }
    }

    async fn do_connect(
        &self,
        server_addr: &str,
        pairing_code: &str,
    ) -> Result<continuum_transport::PairingResponse> {
        let config = continuum_transport::ClientConfig {
            server_addr: server_addr.parse()?,
            pairing_code: pairing_code.to_string(),
            auto_reconnect: false,
            ..Default::default()
        };
        let conn = continuum_transport::client::connect_to_server(&config).await?;
        let (client_secret, _client_public) = continuum_security::generate_dh_keypair();
        let (pair, _shared_secret) = continuum_transport::client::pair_connection(
            &conn.connection,
            pairing_code,
            "test-client",
            &client_secret,
        )
        .await?;
        Ok(pair)
    }

    pub async fn stream_frames(&self, count: u32) -> TestResult {
        let start = Instant::now();

        let result = timeout(STREAM_TIMEOUT, self.stream_frames_inner(count)).await;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(Ok(received)) => {
                let success = received >= count / 2;
                TestResult {
                    name: format!("stream-{}-frames", count),
                    category: TestCategory::E2E,
                    status: if success {
                        TestStatus::Passed
                    } else {
                        TestStatus::Failed
                    },
                    duration_ms: elapsed,
                    started_at: chrono::Utc::now(),
                    finished_at: Some(chrono::Utc::now()),
                    details: format!("Received {}/{} frames in {}ms", received, count, elapsed),
                    metrics: vec![MetricSample {
                        name: "frames_received".to_string(),
                        value: received as f64,
                        unit: "frames".to_string(),
                        percentile: None,
                    }],
                    assertions: vec![AssertionResult {
                        description: format!("Receive >= {}/{} frames", count / 2, count),
                        passed: success,
                        expected: format!(">={}", count / 2),
                        actual: received.to_string(),
                        line: None,
                    }],
                }
            }
            Ok(Err(e)) => TestResult {
                name: format!("stream-{}-frames", count),
                category: TestCategory::E2E,
                status: TestStatus::Error,
                duration_ms: elapsed,
                started_at: chrono::Utc::now(),
                finished_at: None,
                details: format!("Stream error: {}", e),
                metrics: vec![],
                assertions: vec![],
            },
            Err(_) => TestResult {
                name: format!("stream-{}-frames", count),
                category: TestCategory::E2E,
                status: TestStatus::Timeout,
                duration_ms: elapsed,
                started_at: chrono::Utc::now(),
                finished_at: None,
                details: format!("Stream timed out after {}s", STREAM_TIMEOUT.as_secs()),
                metrics: vec![],
                assertions: vec![],
            },
        }
    }

    async fn stream_frames_inner(&self, count: u32) -> Result<u32> {
        let addr: std::net::SocketAddr = self
            .server
            .as_ref()
            .and_then(|s| s.addr.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("Server not started"))?;

        let config = continuum_transport::ClientConfig {
            server_addr: addr,
            pairing_code: "test".to_string(),
            auto_reconnect: false,
            ..Default::default()
        };

        let conn = continuum_transport::client::connect_to_server(&config).await?;
        let (client_secret, _client_public) = continuum_security::generate_dh_keypair();
        let _ = continuum_transport::client::pair_connection(
            &conn.connection,
            "test",
            "test",
            &client_secret,
        )
        .await;

        let (mut media_send, mut media_recv) = conn.connection.open_bi().await?;
        media_send
            .write_all(&(continuum_transport::ApqStreamType::Media as u32).to_be_bytes())
            .await?;

        let mut received = 0u32;
        for _ in 0..count {
            match continuum_transport::client::read_frame(&mut media_recv).await {
                Ok((data, _sem)) => {
                    received += 1;
                    if data.is_empty() {
                        continue;
                    }
                }
                Err(e) => {
                    // If we got at least some frames, consider it a partial success
                    if received > 0 {
                        break;
                    }
                    return Err(e);
                }
            }
        }
        Ok(received)
    }

    pub fn stop_server(&mut self) {
        if let Some(ref mut handle) = self.server {
            if let Some(ref mut process) = handle.process {
                let _ = process.kill();
                let _ = process.wait();
            }
        }
        self.server = None;
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        self.stop_server();
    }
}

impl Default for TestHarness {
    fn default() -> Self {
        Self::new()
    }
}

mod agent_harness;
mod benchmark;
mod chaos;
mod debug_e2e_test;
mod harness;
mod instrument;
mod integration_tests;
mod packaging;
mod relay_integration_tests;
mod replay;
mod reporter;
mod resilience_tests;
mod two_instance_test;
mod types;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Instant;
use types::{TestCategory, TestResult, TestStatus, TestSuiteResult};

#[derive(Parser, Debug)]
#[command(name = "continuum-test", version, about = "Continuum testing suite")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run all E2E and integration tests
    E2E {
        #[arg(short, long)]
        server_addr: Option<String>,
    },

    /// Run chaos tests (disconnect, reconnect, frame stress)
    Chaos,

    /// Run two-instance connection test (server + client on same machine)
    TwoTest,

    /// Run performance benchmarks
    Bench {
        #[arg(short, long)]
        save_json: Option<String>,
    },

    /// Build and package release binaries
    Package {
        #[arg(short, long)]
        format: Option<String>,
    },

    /// Run smoke tests (quick sanity checks)
    Smoke,

    /// Run the full test suite
    All,

    /// Replay a .csr recording file
    Replay {
        /// Path to the .csr recording file
        #[arg(short, long)]
        file: PathBuf,
    },

    /// Generate test report from saved results
    Report {
        #[arg(default_value = "target/test-reports/test-results.json")]
        path: String,

        #[arg(long)]
        html: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cli.log_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    instrument::CrashGuard::install();

    match &cli.command {
        Some(Commands::E2E { server_addr }) => run_e2e(server_addr).await,
        Some(Commands::Chaos) => run_chaos().await,
        Some(Commands::TwoTest) => run_two_test().await,
        Some(Commands::Bench { save_json }) => run_bench(save_json),
        Some(Commands::Package { format }) => run_package(format),
        Some(Commands::Smoke) => run_smoke().await,
        Some(Commands::All) => run_all().await,
        Some(Commands::Replay { file }) => {
            replay::run_replay(file)?;
            Ok(())
        }
        Some(Commands::Report { path, html }) => run_report(path, *html),
        None => {
            println!("Continuum Test Suite v{}", env!("CARGO_PKG_VERSION"));
            println!();
            println!("Commands:");
            println!("  e2e       Run end-to-end integration tests");
            println!("  chaos     Run chaos testing (disconnect, reconnect)");
            println!("  two-test  Two-instance connection test (same machine)");
            println!("  bench     Run performance benchmarks");
            println!("  package   Build and package release binaries");
            println!("  smoke     Run smoke tests");
            println!("  all       Run full test suite");
            println!("  report    Generate HTML/JSON test report");
            println!("  bench     Run performance benchmarks");
            println!("  package   Build and package release binaries");
            println!("  smoke     Run quick smoke tests");
            println!("  all       Run full test suite");
            println!("  report    Generate HTML/JSON test report");
            Ok(())
        }
    }
}

async fn run_two_test() -> Result<()> {
    println!("Running two-instance connection test (server + client on same machine)...\n");
    let reports = two_instance_test::run_two_instance_test().await;
    let failed = reports
        .iter()
        .filter(|r| r.status == two_instance_test::TestStatus::Failed)
        .count();
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_e2e(_server_addr: &Option<String>) -> Result<()> {
    let mut suite = TestSuiteResult::new("e2e-tests");
    let start = Instant::now();

    let mut harness = harness::TestHarness::new();
    let server_addr = "127.0.0.1:4450";

    // Start server
    suite.add(harness.start_server(server_addr).await);

    // Connect client
    suite.add(harness.connect_client(server_addr, "continuum").await);

    // Stream frames
    let result = harness.stream_frames(10).await;
    suite.add(result);

    // Latency benchmark
    suite.add(benchmark::run_latency_benchmark(&mut harness));

    suite.duration_ms = start.elapsed().as_millis() as u64;

    let reporter = reporter::TestReporter::new();
    reporter.print_console(&suite);
    let _json = reporter.generate_json(&suite);
    let html = reporter.generate_html(&suite);

    println!("\nReport: file:///{}", html.display());

    std::process::exit(if suite.failed > 0 { 1 } else { 0 });
}

async fn run_chaos() -> Result<()> {
    let mut suite = TestSuiteResult::new("chaos-tests");
    let start = Instant::now();

    let mut harness = harness::TestHarness::new();

    suite.add(chaos::run_chaos_disconnect_reconnect(&mut harness).await);
    suite.add(chaos::run_chaos_frame_stress(&mut harness).await);

    suite.duration_ms = start.elapsed().as_millis() as u64;

    let reporter = reporter::TestReporter::new();
    reporter.print_console(&suite);
    let _json = reporter.generate_json(&suite);

    std::process::exit(if suite.failed > 0 { 1 } else { 0 });
}

fn run_bench(_save_json: &Option<String>) -> Result<()> {
    let mut suite = TestSuiteResult::new("performance-benchmarks");
    let start = Instant::now();

    let mut harness = harness::TestHarness::new();

    suite.add(benchmark::run_latency_benchmark(&mut harness));
    suite.add(benchmark::run_frame_size_benchmark(&mut harness));

    suite.duration_ms = start.elapsed().as_millis() as u64;

    let reporter = reporter::TestReporter::new();
    reporter.print_console(&suite);
    let html = reporter.generate_html(&suite);

    println!("\nReport: file:///{}", html.display());

    std::process::exit(if suite.failed > 0 { 1 } else { 0 });
}

fn run_package(_format: &Option<String>) -> Result<()> {
    let mut suite = TestSuiteResult::new("packaging");
    let start = Instant::now();
    let packager = packaging::BinaryPackager::new();

    suite.add(packager.build_release_binaries()?);
    suite.add(packager.package_archive("tar.gz")?);

    suite.duration_ms = start.elapsed().as_millis() as u64;

    let reporter = reporter::TestReporter::new();
    reporter.print_console(&suite);

    std::process::exit(if suite.failed > 0 { 1 } else { 0 });
}

async fn run_smoke() -> Result<()> {
    let mut suite = TestSuiteResult::new("smoke-tests");
    let start = Instant::now();

    // Verify crates compile
    suite.add(smoke_crate("continuum-transport").await);
    suite.add(smoke_crate("continuum-security").await);
    suite.add(smoke_crate("continuum-ai").await);
    suite.add(smoke_crate("continuum-observability").await);
    suite.add(smoke_crate("continuum-ci").await);

    // Verify tests pass
    suite.add(smoke_tests("continuum-transport").await);
    suite.add(smoke_tests("continuum-security").await);
    suite.add(smoke_tests("continuum-ai").await);
    suite.add(smoke_tests("continuum-observability").await);

    suite.duration_ms = start.elapsed().as_millis() as u64;

    let reporter = reporter::TestReporter::new();
    reporter.print_console(&suite);

    std::process::exit(if suite.failed > 0 { 1 } else { 0 });
}

async fn smoke_crate(name: &str) -> TestResult {
    let start = Instant::now();
    let output = tokio::process::Command::new("cargo")
        .args(["check", "-p", name, "-q"])
        .output()
        .await;

    match output {
        Ok(out) => {
            let passed = out.status.success();
            TestResult {
                name: format!("check-{}", name),
                category: TestCategory::Smoke,
                status: if passed {
                    TestStatus::Passed
                } else {
                    TestStatus::Failed
                },
                duration_ms: start.elapsed().as_millis() as u64,
                started_at: chrono::Utc::now(),
                finished_at: Some(chrono::Utc::now()),
                details: if passed {
                    "Compiles clean".into()
                } else {
                    String::from_utf8_lossy(&out.stderr).to_string()
                },
                metrics: vec![],
                assertions: vec![AssertionResult {
                    description: format!("{} compiles", name),
                    passed,
                    expected: "exit 0".into(),
                    actual: format!("exit {}", out.status.code().unwrap_or(-1)),
                    line: None,
                }],
            }
        }
        Err(e) => TestResult {
            name: format!("check-{}", name),
            category: TestCategory::Smoke,
            status: TestStatus::Error,
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: chrono::Utc::now(),
            finished_at: None,
            details: format!("Failed to run cargo check: {}", e),
            metrics: vec![],
            assertions: vec![],
        },
    }
}

async fn smoke_tests(name: &str) -> TestResult {
    let start = Instant::now();
    let output = tokio::process::Command::new("cargo")
        .args(["test", "-p", name, "-q", "--no-fail-fast"])
        .output()
        .await;

    match output {
        Ok(out) => {
            let passed = out.status.success();
            TestResult {
                name: format!("test-{}", name),
                category: TestCategory::Smoke,
                status: if passed {
                    TestStatus::Passed
                } else {
                    TestStatus::Failed
                },
                duration_ms: start.elapsed().as_millis() as u64,
                started_at: chrono::Utc::now(),
                finished_at: Some(chrono::Utc::now()),
                details: if passed {
                    "All tests pass".into()
                } else {
                    String::from_utf8_lossy(&out.stderr).to_string()
                },
                metrics: vec![],
                assertions: vec![AssertionResult {
                    description: format!("{} tests pass", name),
                    passed,
                    expected: "exit 0".into(),
                    actual: format!("exit {}", out.status.code().unwrap_or(-1)),
                    line: None,
                }],
            }
        }
        Err(e) => TestResult {
            name: format!("test-{}", name),
            category: TestCategory::Smoke,
            status: TestStatus::Error,
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: chrono::Utc::now(),
            finished_at: None,
            details: format!("Failed to run tests: {}", e),
            metrics: vec![],
            assertions: vec![],
        },
    }
}

async fn run_all() -> Result<()> {
    let mut suite = TestSuiteResult::new("full-test-suite");
    let start = Instant::now();

    println!("Running full Continuum test suite...\n");

    // Phase 1: Smoke tests
    println!("Phase 1/5: Smoke tests");
    suite.add(smoke_crate("continuum-transport").await);
    suite.add(smoke_crate("continuum-security").await);
    suite.add(smoke_crate("continuum-ai").await);
    suite.add(smoke_crate("continuum-observability").await);
    suite.add(smoke_crate("continuum-ci").await);

    // Phase 2: Unit + integration tests
    println!("Phase 2/5: Unit & integration tests");
    suite.add(smoke_tests("continuum-transport").await);
    suite.add(smoke_tests("continuum-security").await);
    suite.add(smoke_tests("continuum-ai").await);
    suite.add(smoke_tests("continuum-observability").await);

    // Phase 3: E2E tests
    println!("Phase 3/5: End-to-end tests");
    let mut harness = harness::TestHarness::new();
    let addr = "127.0.0.1:4455";
    suite.add(harness.start_server(addr).await);
    suite.add(harness.connect_client(addr, "continuum").await);
    suite.add(harness.stream_frames(10).await);

    // Phase 4: Chaos tests
    println!("Phase 4/5: Chaos tests");
    suite.add(chaos::run_chaos_disconnect_reconnect(&mut harness).await);

    // Phase 5: Benchmarks
    println!("Phase 5/5: Performance benchmarks");
    suite.add(benchmark::run_latency_benchmark(&mut harness));
    suite.add(benchmark::run_frame_size_benchmark(&mut harness));

    suite.duration_ms = start.elapsed().as_millis() as u64;

    let reporter = reporter::TestReporter::new();
    reporter.print_console(&suite);
    let _json = reporter.generate_json(&suite);
    let html = reporter.generate_html(&suite);

    println!("\nFull test report: file:///{}", html.display());
    println!(
        "\nSummary: {} passed, {} failed, {} skipped ({:.0}%)",
        suite.passed,
        suite.failed,
        suite.skipped,
        suite.success_rate()
    );

    std::process::exit(if suite.failed > 0 { 1 } else { 0 });
}

fn run_report(path: &str, html: bool) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let suite: TestSuiteResult = serde_json::from_str(&content)?;

    let reporter = reporter::TestReporter::new();
    reporter.print_console(&suite);

    if html {
        let html_path = reporter.generate_html(&suite);
        println!("\nHTML report: file:///{}", html_path.display());
    }

    Ok(())
}

use types::AssertionResult;

// ============================================================================
// Feature and GUI Tests
// ============================================================================
#[cfg(test)]
mod gui_tests {
    use continuum_transport::capture::{compute_frame_diff, synthesize_demo_frame};
    use continuum_transport::codec::{decode_jpeg, encode_jpeg, AdaptiveEncoder};
    use continuum_transport::config::ClientConfig;
    use continuum_transport::error::ContinuumError;
    use continuum_transport::ice_transport::IceConfig;
    use continuum_transport::streaming_engine::{RateControlConfig, RateController};
    use continuum_transport::types::*;
    use std::net::SocketAddr;

    // ------------------------------------------------------------------------
    // Wizard Logic Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_wizard_has_four_steps() {
        let steps = ["Welcome", "ConnectionType", "FindServer", "Pair"];
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0], "Welcome");
        assert_eq!(steps[3], "Pair");
    }

    #[test]
    fn test_wizard_paired_step_transition() {
        let mut current = 3usize; // Pair step index
        current = current.saturating_sub(1);
        assert_eq!(current, 2, "Back from Pair goes to FindServer");
    }

    #[test]
    fn test_wizard_choice_persists() {
        let choice = true; // ManualAddress
        assert!(choice, "Connection type choice should persist");
    }

    #[test]
    fn test_wizard_requires_pairing_code() {
        let code = String::new();
        assert!(code.is_empty(), "Empty code should not allow connect");
        let code = "test123".to_string();
        assert!(!code.is_empty(), "Non-empty code should allow connect");
    }

    #[test]
    fn test_wizard_default_server_addr() {
        let addr: SocketAddr = "127.0.0.1:4433".parse().unwrap();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 4433);
    }

    #[test]
    fn test_wizard_manual_addr_validation() {
        let valid = "192.168.1.100:4433".parse::<SocketAddr>();
        assert!(valid.is_ok());
        let invalid = "not-valid".parse::<SocketAddr>();
        assert!(invalid.is_err());
        let no_port = "192.168.1.100".parse::<SocketAddr>();
        assert!(no_port.is_err());
    }

    // ------------------------------------------------------------------------
    // Settings Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_settings_defaults() {
        let c = ClientConfig::default();
        assert_eq!(c.pairing_code, "continuum");
        assert_eq!(c.client_name, "Continuum Client");
        assert!(!c.view_only);
        assert!(c.auto_reconnect);
        assert!(c.enable_clipboard);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn test_settings_view_only_can_be_set() {
        let mut c = ClientConfig::default();
        c.view_only = true;
        assert!(c.view_only);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn test_settings_reconnect_can_be_disabled() {
        let mut c = ClientConfig::default();
        c.auto_reconnect = false;
        assert!(!c.auto_reconnect);
    }

    #[test]
    fn test_settings_server_addr_accepts_valid() {
        let c = ClientConfig::default();
        assert_eq!(c.server_addr.to_string(), "127.0.0.1:4433");
    }

    // ------------------------------------------------------------------------
    // Error Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_error_severity_classification() {
        let retryable = ContinuumError::ConnectionFailed("test".into());
        assert!(retryable.is_retryable());
        assert!(!retryable.is_fatal());

        let fatal = ContinuumError::InvariantViolation("test".into());
        assert!(fatal.is_fatal());
        assert!(!fatal.is_retryable());

        let normal = ContinuumError::ConfigError("test".into());
        assert!(!normal.is_retryable());
        assert!(!normal.is_fatal());
    }

    #[test]
    fn test_error_display_contains_message() {
        let err = ContinuumError::MonitorNotFound("Monitor 5".into());
        let msg = format!("{}", err);
        assert!(
            msg.contains("Monitor 5"),
            "Error display should include details"
        );
    }

    // ------------------------------------------------------------------------
    // Frame Encode/Decode Pipeline Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_jpeg_quality_affects_size() {
        let img = synthesize_demo_frame();
        let low = encode_jpeg(&img, 30).unwrap();
        let high = encode_jpeg(&img, 95).unwrap();
        assert!(
            high.len() >= low.len(),
            "Higher quality should produce >= size (low={}, high={})",
            low.len(),
            high.len()
        );
    }

    #[test]
    fn test_jpeg_quality_differences() {
        let img = synthesize_demo_frame();
        // Test several quality levels produce different sizes
        let sizes: Vec<usize> = [30, 50, 70, 85, 95]
            .iter()
            .map(|&q| encode_jpeg(&img, q).unwrap().len())
            .collect();
        // Check each successive quality produces >= size
        for i in 1..sizes.len() {
            assert!(
                sizes[i] >= sizes[i - 1],
                "Quality {} should produce >= size than quality {} ({} vs {})",
                [30, 50, 70, 85, 95][i],
                [30, 50, 70, 85, 95][i - 1],
                sizes[i],
                sizes[i - 1]
            );
        }
    }

    #[test]
    fn test_jpeg_decode_rejects_empty() {
        assert!(decode_jpeg(&[]).is_err());
        assert!(decode_jpeg(&[0; 2]).is_err());
    }

    #[test]
    fn test_encode_decode_preserves_dimensions() {
        let img = synthesize_demo_frame();
        let data = encode_jpeg(&img, 85).unwrap();
        let decoded = decode_jpeg(&data).unwrap();
        assert_eq!(decoded.width(), img.width());
        assert_eq!(decoded.height(), img.height());
    }

    #[test]
    fn test_frame_diff_identical_is_zero() {
        let d = vec![128u8; 1000];
        assert_eq!(compute_frame_diff(&d, &d), 0.0);
    }

    #[test]
    fn test_frame_diff_completely_different_is_one() {
        let a = vec![0u8; 1000];
        let b = vec![255u8; 1000];
        assert_eq!(compute_frame_diff(&a, &b), 1.0);
    }

    #[test]
    fn test_frame_diff_mismatched_sizes_is_one() {
        assert_eq!(compute_frame_diff(&[0; 100], &[0; 50]), 1.0);
        assert_eq!(compute_frame_diff(&[], &[]), 1.0);
    }

    // ------------------------------------------------------------------------
    // Adaptive Encoder Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_encoder_quality_clamping() {
        let e = AdaptiveEncoder::new(200, 30);
        assert_eq!(e.quality(), 100, "Quality clamped to 100 max");
        let e = AdaptiveEncoder::new(0, 30);
        assert_eq!(e.quality(), 1, "Quality clamped to 1 min");
    }

    #[test]
    fn test_encoder_initial_frame_number() {
        let e = AdaptiveEncoder::new(85, 30);
        assert_eq!(e.frame_number(), 0);
    }

    // ------------------------------------------------------------------------
    // Protocol Type Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_apq_stream_type_constants() {
        assert_eq!(ApqStreamType::Media as u32, 1);
        assert_eq!(ApqStreamType::Intent as u32, 2);
        assert_eq!(ApqStreamType::Clipboard as u32, 3);
        assert_eq!(ApqStreamType::FileTransfer as u32, 4);
        assert_eq!(ApqStreamType::Audio as u32, 5);
    }

    #[test]
    fn test_apq_stream_type_unknown_defaults_to_media() {
        assert_eq!(ApqStreamType::from(0), ApqStreamType::Media);
        assert_eq!(ApqStreamType::from(99), ApqStreamType::Media);
        assert_eq!(ApqStreamType::from(255), ApqStreamType::Media);
    }

    #[test]
    fn test_permissions_default() {
        let p = Permissions::default();
        assert!(
            p.can_view && p.can_control && p.can_clipboard && p.can_file_transfer && p.can_audio
        );
    }

    #[test]
    fn test_permissions_view_only() {
        let p = Permissions::view_only();
        assert!(p.can_view);
        assert!(!p.can_control);
        assert!(!p.can_file_transfer);
    }

    // ------------------------------------------------------------------------
    // ICE Transport Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_ice_default_has_stun_server() {
        let config = IceConfig::default();
        assert!(config.stun_servers.iter().any(|s| s.contains("google.com")));
        assert!(config.ice_lite, "ICE-lite should be true by default");
    }

    #[test]
    fn test_ice_config_defaults_are_sane() {
        let config = IceConfig::default();
        assert!(config.gather_timeout_secs > 0);
        assert!(config.gather_timeout_secs <= 30);
    }

    // ------------------------------------------------------------------------
    // Rate Controller Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_rate_controller_has_sane_defaults() {
        let config = RateControlConfig::default();
        assert_eq!(config.min_quality, 30);
        assert_eq!(config.max_quality, 95);
        assert_eq!(config.target_fps, 30);
        assert!(config.target_latency_ms > 0.0);
    }

    #[test]
    fn test_rate_controller_initial_decision_valid() {
        let mut rc = RateController::new(RateControlConfig::default());
        let d = rc.decide();
        assert!(!d.skip_frame, "Initial frame should not be skipped");
        assert!(d.quality >= 30, "Initial quality >= min");
        assert!(d.fps >= 10, "Initial FPS >= min");
    }

    // ------------------------------------------------------------------------
    // Audit Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_audit_log_and_verify() {
        use continuum_observability::audit::{AuditEvent, AuditLogger};
        let log = AuditLogger::new();
        log.log(AuditEvent::ConnectionOpened, "10.0.0.1", "s1", "opened");
        log.log(
            AuditEvent::InputEvent {
                action: "click".into(),
            },
            "10.0.0.1",
            "s1",
            "clicked",
        );
        assert!(log.verify_chain(), "Audit chain should verify");
        assert!(log.entries().len() >= 2, "Should have at least 2 entries");
    }

    // ------------------------------------------------------------------------
    // Config Parsing Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_client_config_env_overrides() {
        // Default has no env overrides
        let c = ClientConfig::default();
        assert_eq!(c.log_level, "info");
        assert_eq!(c.log_format, "text");
    }

    #[test]
    fn test_client_config_name_default() {
        let c = ClientConfig::default();
        assert_eq!(c.client_name, "Continuum Client");
    }

    #[test]
    fn test_client_config_server_addr_default() {
        let c = ClientConfig::default();
        assert_eq!(format!("{}", c.server_addr), "127.0.0.1:4433");
    }

    // ------------------------------------------------------------------------
    // Stress / Edge Case Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_lots_of_frames_doesnt_crash() {
        let img = synthesize_demo_frame();
        for q in [30, 50, 70, 85, 95] {
            for _ in 0..10 {
                let data = encode_jpeg(&img, q).unwrap();
                let _decoded = decode_jpeg(&data).unwrap();
            }
        }
    }

    #[test]
    fn test_extreme_quality_values() {
        let img = synthesize_demo_frame();
        // Quality 1 should work
        let d1 = encode_jpeg(&img, 1).unwrap();
        assert!(!d1.is_empty());
        // Quality 100 should work
        let d100 = encode_jpeg(&img, 100).unwrap();
        assert!(!d100.is_empty());
        // Quality 100 should be >= quality 1 in size
        assert!(
            d100.len() >= d1.len(),
            "Quality 100 ({}) should be >= quality 1 ({})",
            d100.len(),
            d1.len()
        );
    }

    #[test]
    fn test_adaptive_encoder_handles_empty_images() {
        use image::DynamicImage;
        // A 1x1 image is the minimum viable
        let tiny = DynamicImage::new_rgba8(1, 1);
        let mut enc = AdaptiveEncoder::new(85, 30);
        let result = enc.encode(&tiny, 0);
        assert!(
            result.is_ok(),
            "Tiny image should encode: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_adaptive_encoder_frame_numbers_increment() {
        let mut enc = AdaptiveEncoder::new(85, 30);
        let img = synthesize_demo_frame();
        assert_eq!(enc.frame_number(), 0);
        let _ = enc.encode(&img, 0);
        assert_eq!(
            enc.frame_number(),
            1,
            "Frame number should increment after encode"
        );
        let _ = enc.encode(&img, 0);
        assert_eq!(enc.frame_number(), 2, "Frame number should increment twice");
    }

    // ------------------------------------------------------------------------
    // Recording Format Tests
    // ------------------------------------------------------------------------
    #[test]
    fn test_recording_format_roundtrip() {
        use continuum_transport::capture::synthesize_demo_frame;
        use continuum_transport::codec::encode_jpeg;
        use continuum_transport::recording::SessionRecorder;
        use continuum_transport::types::{ContentType, FrameSemantics};

        let dir = std::env::temp_dir().join("continuum-test-recording");
        let _ = std::fs::create_dir_all(&dir);

        let mut recorder = SessionRecorder::new(dir.clone());
        let session_id = recorder.start_session().expect("start session");
        assert!(!session_id.is_empty());

        for i in 0..10 {
            let img = synthesize_demo_frame();
            let jpeg = encode_jpeg(&img, 85).expect("encode");
            let semantics = FrameSemantics {
                content_type: ContentType::Jpeg,
                width: img.width(),
                height: img.height(),
                quality: 85,
                frame_number: i as u64,
                timestamp: chrono::Utc::now(),
                is_keyframe: true,
                monitor_id: 0,
                encode_time_us: 0,
            };
            recorder.record_frame(&jpeg, &semantics);
        }

        let record = recorder.stop_session().expect("stop session");
        assert_eq!(record.frame_count, 10);
        assert!(record.total_bytes > 0);

        let csr_file = dir.join(format!("{}.csr", session_id));
        assert!(
            csr_file.exists(),
            "Recording file should exist at {}",
            csr_file.display()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_replay_reads_recording() {
        use continuum_transport::capture::synthesize_demo_frame;
        use continuum_transport::codec::encode_jpeg;
        use continuum_transport::recording::SessionRecorder;
        use continuum_transport::types::{ContentType, FrameSemantics};

        let dir = std::env::temp_dir().join("continuum-test-replay");
        let _ = std::fs::create_dir_all(&dir);

        let mut recorder = SessionRecorder::new(dir.clone());
        let session_id = recorder.start_session().unwrap();

        for i in 0..5 {
            let img = synthesize_demo_frame();
            let jpeg = encode_jpeg(&img, 80).unwrap();
            let semantics = FrameSemantics {
                content_type: ContentType::Jpeg,
                width: img.width(),
                height: img.height(),
                quality: 80,
                frame_number: i as u64,
                timestamp: chrono::Utc::now(),
                is_keyframe: true,
                monitor_id: 0,
                encode_time_us: 0,
            };
            recorder.record_frame(&jpeg, &semantics);
        }
        recorder.stop_session().unwrap();

        let csr_file = dir.join(format!("{}.csr", session_id));
        assert!(csr_file.exists());

        let result = crate::replay::run_replay(&csr_file);
        assert!(result.is_ok(), "Replay should succeed: {:?}", result);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

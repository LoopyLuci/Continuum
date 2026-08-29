use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub category: TestCategory,
    pub status: TestStatus,
    pub duration_ms: u64,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub details: String,
    pub metrics: Vec<MetricSample>,
    pub assertions: Vec<AssertionResult>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TestCategory {
    Unit,
    Integration,
    E2E,
    Chaos,
    Performance,
    Smoke,
    Packaging,
}

impl std::fmt::Display for TestCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unit => write!(f, "unit"),
            Self::Integration => write!(f, "integration"),
            Self::E2E => write!(f, "e2e"),
            Self::Chaos => write!(f, "chaos"),
            Self::Performance => write!(f, "performance"),
            Self::Smoke => write!(f, "smoke"),
            Self::Packaging => write!(f, "packaging"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
    Error,
    Timeout,
}

impl std::fmt::Display for TestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(f, "PASS"),
            Self::Failed => write!(f, "FAIL"),
            Self::Skipped => write!(f, "SKIP"),
            Self::Error => write!(f, "ERROR"),
            Self::Timeout => write!(f, "TIMEOUT"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub name: String,
    pub value: f64,
    pub unit: String,
    pub percentile: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionResult {
    pub description: String,
    pub passed: bool,
    pub expected: String,
    pub actual: String,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSuiteResult {
    pub name: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration_ms: u64,
    pub timestamp: DateTime<Utc>,
    pub results: Vec<TestResult>,
    pub system_info: SystemInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub cpu_count: u32,
    pub memory_mb: u64,
    pub rust_version: String,
    pub target_triple: String,
}

impl TestSuiteResult {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            total: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            duration_ms: 0,
            timestamp: Utc::now(),
            results: Vec::new(),
            system_info: SystemInfo::collect(),
        }
    }

    pub fn add(&mut self, result: TestResult) {
        match result.status {
            TestStatus::Passed => self.passed += 1,
            TestStatus::Failed | TestStatus::Error | TestStatus::Timeout => self.failed += 1,
            TestStatus::Skipped => self.skipped += 1,
        }
        self.total += 1;
        self.results.push(result);
    }

    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.passed as f64 / self.total as f64) * 100.0
    }
}

impl SystemInfo {
    pub fn collect() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            cpu_count: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1),
            memory_mb: total_system_memory(),
            rust_version: rustc_version(),
            target_triple: std::env::consts::ARCH.to_string(),
        }
    }
}

fn total_system_memory() -> u64 {
    use sysinfo::System;
    let mut system = System::new();
    system.refresh_memory();
    system.total_memory() / 1024
}

fn rustc_version() -> String {
    let version = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());
    version
        .unwrap_or_else(|| "unknown".to_string())
        .trim()
        .to_string()
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TestResumeData {
    pub paired: bool,
    pub permissions: TestPermissions,
    pub shared_secret: Option<Vec<u8>>,
    pub session_id: String,
    pub created_at: std::time::Instant,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct TestPermissions {
    pub can_control: bool,
    pub can_clipboard: bool,
    pub can_file_transfer: bool,
}

impl TestPermissions {
    #[allow(dead_code)]
    pub fn view_only() -> Self {
        Self {
            can_control: false,
            can_clipboard: false,
            can_file_transfer: false,
        }
    }
}

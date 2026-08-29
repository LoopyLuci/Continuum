#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub static NEXT_BUILD_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_build_id() -> String {
    let id = NEXT_BUILD_ID.fetch_add(1, Ordering::SeqCst);
    format!("build-{:06}", id)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BuildStatus {
    Pending,
    Running,
    Success,
    Failure,
    Cancelled,
    Timeout,
}

impl BuildStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            BuildStatus::Success
                | BuildStatus::Failure
                | BuildStatus::Cancelled
                | BuildStatus::Timeout
        )
    }

    pub fn color_hex(&self) -> &'static str {
        match self {
            BuildStatus::Pending => "#f59e0b",
            BuildStatus::Running => "#3b82f6",
            BuildStatus::Success => "#22c55e",
            BuildStatus::Failure => "#ef4444",
            BuildStatus::Cancelled => "#6b7280",
            BuildStatus::Timeout => "#f97316",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            BuildStatus::Pending => "Pending",
            BuildStatus::Running => "Running",
            BuildStatus::Success => "Success",
            BuildStatus::Failure => "Failure",
            BuildStatus::Cancelled => "Cancelled",
            BuildStatus::Timeout => "Timeout",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRecord {
    pub id: String,
    pub pipeline_name: String,
    pub status: BuildStatus,
    pub trigger: BuildTrigger,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: u64,
    pub stages: Vec<StageRecord>,
    pub branch: String,
    pub commit: String,
    pub artifacts: Vec<ArtifactRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BuildTrigger {
    Manual,
    FileWatch { paths: Vec<String> },
    Schedule,
    GitHook,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRecord {
    pub name: String,
    pub status: BuildStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: u64,
    pub output: Vec<OutputLine>,
    pub exit_code: Option<i32>,
    pub allow_failure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputLine {
    pub stream: StreamType,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamType {
    Stdout,
    Stderr,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct RunningBuild {
    pub id: String,
    pub config: crate::config::CiConfig,
    pub status: Arc<std::sync::RwLock<BuildStatus>>,
    pub stages: Arc<std::sync::RwLock<Vec<StageRecord>>>,
    pub started_at: DateTime<Utc>,
    pub cancel_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl RunningBuild {
    pub fn new(config: crate::config::CiConfig) -> Self {
        let stages: Vec<StageRecord> = config
            .stages
            .iter()
            .map(|s| StageRecord {
                name: s.name.clone(),
                status: BuildStatus::Pending,
                started_at: None,
                finished_at: None,
                duration_ms: 0,
                output: Vec::new(),
                exit_code: None,
                allow_failure: s.allow_failure,
            })
            .collect();

        Self {
            id: next_build_id(),
            config,
            status: Arc::new(std::sync::RwLock::new(BuildStatus::Running)),
            stages: Arc::new(std::sync::RwLock::new(stages)),
            started_at: Utc::now(),
            cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSummary {
    pub total: usize,
    pub success: usize,
    pub failure: usize,
    pub running: usize,
    pub pending: usize,
    pub avg_duration_ms: f64,
}

impl BuildSummary {
    pub fn from_records(records: &[BuildRecord]) -> Self {
        let total = records.len();
        let success = records
            .iter()
            .filter(|r| r.status == BuildStatus::Success)
            .count();
        let failure = records
            .iter()
            .filter(|r| r.status == BuildStatus::Failure)
            .count();
        let running = records
            .iter()
            .filter(|r| r.status == BuildStatus::Running)
            .count();
        let pending = records
            .iter()
            .filter(|r| r.status == BuildStatus::Pending)
            .count();
        let avg_duration_ms = if total > 0 {
            records.iter().map(|r| r.duration_ms as f64).sum::<f64>() / total as f64
        } else {
            0.0
        };

        Self {
            total,
            success,
            failure,
            running,
            pending,
            avg_duration_ms,
        }
    }
}

pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{}m {}s", mins, secs)
    }
}

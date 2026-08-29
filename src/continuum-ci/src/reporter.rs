#![allow(dead_code)]

use crate::config::CiConfig;
use crate::types::{BuildRecord, BuildStatus, BuildSummary};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct Reporter {
    db: Mutex<Connection>,
    data_dir: PathBuf,
}

impl Reporter {
    pub fn new(config: &CiConfig) -> Self {
        let data_dir = config.project.data_dir.clone();
        std::fs::create_dir_all(&data_dir).ok();

        let db_path = data_dir.join("continuum-ci.db");
        let conn = Connection::open(&db_path).expect("Failed to open CI database");
        conn.execute_batch(SCHEMA)
            .expect("Failed to initialize CI database");

        tracing::debug!(path = %db_path.display(), "CI database initialized");

        Self {
            db: Mutex::new(conn),
            data_dir,
        }
    }

    pub fn save_build(&self, record: &BuildRecord) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO builds (id, pipeline_name, status, trigger, started_at, finished_at, duration_ms, branch, commit) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id,
                record.pipeline_name,
                record.status.label(),
                format!("{:?}", record.trigger),
                record.started_at.to_rfc3339(),
                record.finished_at.map(|t| t.to_rfc3339()),
                record.duration_ms as i64,
                record.branch,
                record.commit,
            ],
        )?;

        for stage in &record.stages {
            db.execute(
                "INSERT INTO stages (build_id, name, status, started_at, finished_at, duration_ms, exit_code, allow_failure) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    record.id,
                    stage.name,
                    stage.status.label(),
                    stage.started_at.map(|t| t.to_rfc3339()),
                    stage.finished_at.map(|t| t.to_rfc3339()),
                    stage.duration_ms as i64,
                    stage.exit_code,
                    stage.allow_failure as i32,
                ],
            )?;
        }

        Ok(())
    }

    pub fn list_builds(&self, limit: usize) -> Result<Vec<BuildRecord>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, pipeline_name, status, trigger, started_at, finished_at, duration_ms, branch, commit FROM builds ORDER BY started_at DESC LIMIT ?1",
        )?;

        let records = stmt
            .query_map(params![limit as i64], |row| {
                Ok(BuildRecord {
                    id: row.get(0)?,
                    pipeline_name: row.get(1)?,
                    status: parse_status(row.get::<_, String>(2)?),
                    trigger: crate::types::BuildTrigger::Manual,
                    started_at: row
                        .get::<_, String>(3)?
                        .parse()
                        .unwrap_or_else(|_| Utc::now()),
                    finished_at: row
                        .get::<_, Option<String>>(4)?
                        .and_then(|s| s.parse().ok()),
                    duration_ms: row.get::<_, i64>(5)? as u64,
                    branch: row.get(6)?,
                    commit: row.get(7)?,
                    stages: Vec::new(),
                    artifacts: Vec::new(),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }

    pub fn get_build(&self, id: &str) -> Result<Option<BuildRecord>> {
        self.get_build_full(id)
    }

    fn get_build_full(&self, id: &str) -> Result<Option<BuildRecord>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, pipeline_name, status, trigger, started_at, finished_at, duration_ms, branch, commit FROM builds WHERE id = ?1",
        )?;

        let mut records: Vec<BuildRecord> = stmt
            .query_map(params![id], |row| {
                Ok(BuildRecord {
                    id: row.get(0)?,
                    pipeline_name: row.get(1)?,
                    status: parse_status(row.get::<_, String>(2)?),
                    trigger: crate::types::BuildTrigger::Manual,
                    started_at: row
                        .get::<_, String>(3)?
                        .parse()
                        .unwrap_or_else(|_| Utc::now()),
                    finished_at: row
                        .get::<_, Option<String>>(4)?
                        .and_then(|s| s.parse().ok()),
                    duration_ms: row.get::<_, i64>(5)? as u64,
                    branch: row.get(6)?,
                    commit: row.get(7)?,
                    stages: Vec::new(),
                    artifacts: Vec::new(),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        if let Some(ref mut record) = records.first_mut() {
            let mut stage_stmt = db.prepare(
                "SELECT name, status, started_at, finished_at, duration_ms, exit_code, allow_failure FROM stages WHERE build_id = ?1 ORDER BY started_at",
            )?;

            record.stages = stage_stmt
                .query_map(params![id], |row| {
                    Ok(crate::types::StageRecord {
                        name: row.get(0)?,
                        status: parse_status(row.get::<_, String>(1)?),
                        started_at: row
                            .get::<_, Option<String>>(2)?
                            .and_then(|s| s.parse().ok()),
                        finished_at: row
                            .get::<_, Option<String>>(3)?
                            .and_then(|s| s.parse().ok()),
                        duration_ms: row.get::<_, i64>(4)? as u64,
                        output: Vec::new(),
                        exit_code: row.get(5)?,
                        allow_failure: row.get::<_, i32>(6)? != 0,
                    })
                })?
                .filter_map(|r| r.ok())
                .collect();

            Ok(Some(record.clone()))
        } else {
            Ok(None)
        }
    }

    pub fn summary(&self) -> Result<BuildSummary> {
        let builds = self.list_builds(100)?;
        Ok(BuildSummary::from_records(&builds))
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }
}

fn parse_status(s: String) -> BuildStatus {
    match s.to_lowercase().as_str() {
        "success" => BuildStatus::Success,
        "failure" => BuildStatus::Failure,
        "running" => BuildStatus::Running,
        "pending" => BuildStatus::Pending,
        "cancelled" => BuildStatus::Cancelled,
        "timeout" => BuildStatus::Timeout,
        _ => BuildStatus::Pending,
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS builds (
    id TEXT PRIMARY KEY,
    pipeline_name TEXT NOT NULL,
    status TEXT NOT NULL,
    trigger TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    branch TEXT NOT NULL DEFAULT '',
    commit TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS stages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    build_id TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    exit_code INTEGER,
    allow_failure INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (build_id) REFERENCES builds(id)
);

CREATE INDEX IF NOT EXISTS idx_builds_started_at ON builds(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_stages_build_id ON stages(build_id);
";

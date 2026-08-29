#![allow(dead_code)]

use crate::types::{BuildStatus, OutputLine, StageRecord, StreamType};
use chrono::Utc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::broadcast;

pub struct StageRunner {
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    output_tx: broadcast::Sender<OutputLine>,
}

impl StageRunner {
    pub fn new(
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
        output_tx: broadcast::Sender<OutputLine>,
    ) -> Self {
        Self {
            cancel_flag,
            output_tx,
        }
    }

    pub fn output_rx(&self) -> broadcast::Receiver<OutputLine> {
        self.output_tx.subscribe()
    }

    pub async fn run(
        &self,
        name: &str,
        commands: &[String],
        env: &std::collections::HashMap<String, String>,
        work_dir: &std::path::Path,
        timeout_secs: u64,
    ) -> StageRecord {
        let started_at = Utc::now();
        let start = Instant::now();
        let output = Vec::new();

        self.emit(StreamType::System, format!("[{}] Starting stage", name));

        let mut exit_code = 0i32;

        for cmd_str in commands.iter() {
            if self.cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                self.emit(StreamType::System, "[CANCELLED]".to_string());
                return StageRecord {
                    name: name.to_string(),
                    status: BuildStatus::Cancelled,
                    started_at: Some(started_at),
                    finished_at: Some(Utc::now()),
                    duration_ms: start.elapsed().as_millis() as u64,
                    output,
                    exit_code: Some(-1),
                    allow_failure: false,
                };
            }

            let shell_cmd = if cfg!(target_os = "windows") {
                "pwsh".to_string()
            } else {
                "sh".to_string()
            };
            let shell_arg = if cfg!(target_os = "windows") {
                "-Command".to_string()
            } else {
                "-c".to_string()
            };

            let child = Command::new(&shell_cmd)
                .arg(&shell_arg)
                .arg(cmd_str)
                .current_dir(work_dir)
                .envs(env.iter())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();

            match child {
                Ok(mut child) => {
                    let stdout = child.stdout.take();
                    let stderr = child.stderr.take();

                    let cancel = self.cancel_flag.clone();
                    let tx = self.output_tx.clone();
                    let tx2 = self.output_tx.clone();

                    let stdout_task = tokio::spawn(async move {
                        if let Some(stdout) = stdout {
                            let reader = tokio::io::BufReader::new(stdout);
                            let mut lines = reader.lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                    break;
                                }
                                if tx
                                    .send(OutputLine {
                                        stream: StreamType::Stdout,
                                        text: line,
                                        timestamp: Utc::now(),
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                    });

                    let stderr_task = tokio::spawn(async move {
                        if let Some(stderr) = stderr {
                            let reader = tokio::io::BufReader::new(stderr);
                            let mut lines = reader.lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                if tx2
                                    .send(OutputLine {
                                        stream: StreamType::Stderr,
                                        text: line,
                                        timestamp: Utc::now(),
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                    });

                    let timeout = Duration::from_secs(timeout_secs);
                    let result = tokio::time::timeout(timeout, child.wait()).await;

                    let _ = stdout_task.await;
                    let _ = stderr_task.await;

                    match result {
                        Ok(Ok(status)) => {
                            exit_code = status.code().unwrap_or(0);
                            if exit_code != 0 {
                                self.emit(
                                    StreamType::System,
                                    format!("[FAILED] Exit code: {}", exit_code),
                                );
                            }
                        }
                        Ok(Err(e)) => {
                            self.emit(
                                StreamType::System,
                                format!("[ERROR] Failed to run command: {}", e),
                            );
                            exit_code = -1;
                        }
                        Err(_) => {
                            let _ = child.kill().await;
                            self.emit(StreamType::System, "[TIMEOUT] Stage timed out".to_string());
                            return StageRecord {
                                name: name.to_string(),
                                status: BuildStatus::Timeout,
                                started_at: Some(started_at),
                                finished_at: Some(Utc::now()),
                                duration_ms: start.elapsed().as_millis() as u64,
                                output,
                                exit_code: Some(-1),
                                allow_failure: false,
                            };
                        }
                    }

                    if self.cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                        self.emit(StreamType::System, "[CANCELLED]".to_string());
                        return StageRecord {
                            name: name.to_string(),
                            status: BuildStatus::Cancelled,
                            started_at: Some(started_at),
                            finished_at: Some(Utc::now()),
                            duration_ms: start.elapsed().as_millis() as u64,
                            output,
                            exit_code: Some(-1),
                            allow_failure: false,
                        };
                    }
                }
                Err(e) => {
                    self.emit(
                        StreamType::System,
                        format!("[ERROR] Failed to spawn process: {}", e),
                    );
                    exit_code = -2;
                }
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        let status = if exit_code == 0 {
            BuildStatus::Success
        } else {
            BuildStatus::Failure
        };

        self.emit(
            StreamType::System,
            format!(
                "[{}] Completed in {}",
                status.label(),
                crate::types::format_duration(elapsed)
            ),
        );

        StageRecord {
            name: name.to_string(),
            status,
            started_at: Some(started_at),
            finished_at: Some(Utc::now()),
            duration_ms: elapsed,
            output,
            exit_code: Some(exit_code),
            allow_failure: false,
        }
    }

    fn emit(&self, stream: StreamType, text: String) {
        let _ = self.output_tx.send(OutputLine {
            stream,
            text,
            timestamp: Utc::now(),
        });
    }
}

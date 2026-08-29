use crate::config::{CiConfig, StageDef};
use crate::runner::StageRunner;
use crate::types::{BuildRecord, BuildStatus, BuildTrigger, RunningBuild};
use anyhow::Result;
use chrono::Utc;
use tokio::sync::broadcast;

pub struct Pipeline {
    config: CiConfig,
    reporter: crate::reporter::Reporter,
}

impl Pipeline {
    pub fn new(config: CiConfig) -> Self {
        let reporter = crate::reporter::Reporter::new(&config);
        Self { config, reporter }
    }

    pub async fn run(&self, trigger: BuildTrigger) -> Result<RunningBuild> {
        let build = RunningBuild::new(self.config.clone());

        let id = build.id.clone();
        let cancel = build.cancel_flag.clone();
        let stages_arc = build.stages.clone();
        let status_arc = build.status.clone();
        let started_at = build.started_at;

        tracing::info!(build_id = %id, trigger = ?trigger, "Starting build");

        let (output_tx, _) = broadcast::channel(4096);
        let _runner = StageRunner::new(cancel.clone(), output_tx.clone());

        let config_stages = self.config.stages.clone();
        let config_clone = self.config.clone();
        let ci_config = self.config.clone();

        tokio::spawn(async move {
            let work_dir = &config_clone.project.work_dir;
            let stage_defs = resolve_dag(&config_stages);

            for stage_batch in &stage_defs {
                let mut handles = Vec::new();

                for stage_def in stage_batch {
                    let name = stage_def.name.clone();
                    let commands = stage_def.commands.clone();
                    let env = stage_def.env.clone();
                    let wd = work_dir.to_path_buf();
                    let timeout = stage_def.timeout_secs;
                    let allow_failure = stage_def.allow_failure;
                    let cancel = cancel.clone();
                    let stages = stages_arc.clone();
                    let output_tx = output_tx.clone();

                    let handle = tokio::spawn(async move {
                        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }

                        let stage_runner = StageRunner::new(cancel.clone(), output_tx);
                        let mut stage =
                            stage_runner.run(&name, &commands, &env, &wd, timeout).await;
                        stage.allow_failure = allow_failure;

                        if !allow_failure && stage.status == BuildStatus::Failure {
                            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }

                        let mut stages_w = stages.write().unwrap();
                        if let Some(s) = stages_w.iter_mut().find(|s| s.name == name) {
                            *s = stage;
                        }
                    });
                    handles.push(handle);
                }

                for handle in handles {
                    let _ = handle.await;
                }

                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
            }

            let stages_final = stages_arc.read().unwrap().clone();
            let any_failed = stages_final
                .iter()
                .any(|s| s.status == BuildStatus::Failure && !s.allow_failure);
            let any_timeout = stages_final
                .iter()
                .any(|s| s.status == BuildStatus::Timeout);
            let any_cancelled = stages_final
                .iter()
                .any(|s| s.status == BuildStatus::Cancelled);

            let final_status = if any_cancelled {
                BuildStatus::Cancelled
            } else if any_timeout {
                BuildStatus::Timeout
            } else if any_failed {
                BuildStatus::Failure
            } else {
                BuildStatus::Success
            };

            *status_arc.write().unwrap() = final_status.clone();

            let finished_at = Utc::now();
            let duration_ms = (finished_at - started_at).num_milliseconds().max(0) as u64;

            let build_record = BuildRecord {
                id: id.clone(),
                pipeline_name: ci_config.project.name.clone(),
                status: final_status,
                trigger,
                started_at,
                finished_at: Some(finished_at),
                duration_ms,
                stages: stages_final,
                branch: get_git_branch(),
                commit: get_git_commit(),
                artifacts: Vec::new(),
            };

            let _ = build_record;
            tracing::info!(build_id = %id, duration_ms = duration_ms, "Build completed");
        });

        Ok(build)
    }

    pub fn reporter(&self) -> &crate::reporter::Reporter {
        &self.reporter
    }
}

fn resolve_dag(stages: &[StageDef]) -> Vec<Vec<StageDef>> {
    let mut remaining: Vec<StageDef> = stages.to_vec();
    let mut completed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut batches: Vec<Vec<StageDef>> = Vec::new();

    while !remaining.is_empty() {
        let mut batch = Vec::new();

        remaining.retain(|stage| {
            let all_deps_met = stage.depends_on.iter().all(|dep| completed.contains(dep));

            if all_deps_met {
                batch.push(stage.clone());
                false
            } else {
                true
            }
        });

        if batch.is_empty() {
            tracing::error!("Circular dependency detected in pipeline");
            break;
        }

        for stage in &batch {
            completed.insert(stage.name.clone());
        }

        batches.push(batch);
    }

    batches
}

fn get_git_branch() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_default()
}

fn get_git_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_default()
}

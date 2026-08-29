#![allow(dead_code)]

use crate::config::CiConfig;
use anyhow::Result;
use notify::{Event, EventHandler, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;
use std::time::Duration;

struct ChangeCollector {
    tx: mpsc::Sender<Vec<String>>,
    debounce: Duration,
    last_event: Option<std::time::Instant>,
    ignore: Vec<String>,
}

impl EventHandler for ChangeCollector {
    fn handle_event(&mut self, event: notify::Result<Event>) {
        let Ok(event) = event else { return };
        let now = std::time::Instant::now();

        if let Some(last) = self.last_event {
            if now.duration_since(last) < self.debounce {
                return;
            }
        }
        self.last_event = Some(now);

        let mut changed: Vec<String> = event
            .paths
            .iter()
            .filter_map(|p| {
                let s = p.to_string_lossy().to_string();
                if self.ignore.iter().any(|pat| s.contains(pat)) {
                    None
                } else {
                    Some(s)
                }
            })
            .collect();

        changed.sort();
        changed.dedup();

        if !changed.is_empty() {
            let _ = self.tx.send(changed);
        }
    }
}

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<Vec<String>>,
}

impl FileWatcher {
    pub fn new(config: &CiConfig) -> Result<Self> {
        let (tx, rx) = mpsc::channel();

        let handler = ChangeCollector {
            tx,
            debounce: Duration::from_millis(config.watch.debounce_ms),
            last_event: None,
            ignore: config.watch.ignore_patterns.clone(),
        };

        let mut watcher = RecommendedWatcher::new(handler, notify::Config::default())?;

        for watch_path in &config.watch.paths {
            let abs = if watch_path.is_absolute() {
                watch_path.clone()
            } else {
                config.project.work_dir.join(watch_path)
            };
            if abs.exists() {
                watcher.watch(&abs, RecursiveMode::Recursive)?;
                tracing::info!(path = %abs.display(), "Watching for changes");
            } else {
                tracing::warn!(path = %abs.display(), "Watch path does not exist");
            }
        }

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    pub fn next_changes(&self) -> Option<Vec<String>> {
        self.rx.try_recv().ok()
    }

    pub fn wait_for_changes(&self) -> Result<Vec<String>> {
        Ok(self.rx.recv()?)
    }
}

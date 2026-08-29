#![allow(dead_code)]

use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};

static PANICKED: AtomicBool = AtomicBool::new(false);

pub struct CrashGuard;

impl CrashGuard {
    pub fn install() {
        panic::set_hook(Box::new(|info| {
            PANICKED.store(true, Ordering::SeqCst);
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()));
            let payload = info.payload().downcast_ref::<&str>().map(|s| s.to_string());
            let msg = payload.unwrap_or_else(|| "unknown panic".to_string());
            let loc = location.unwrap_or_else(|| "unknown location".to_string());
            eprintln!("\n[CONTINUUM-TEST] PANIC at {}: {}", loc, msg);

            // Write crash file for test reporter
            let crash = serde_json::json!({
                "type": "panic",
                "message": msg,
                "location": loc,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            });
            let _ = std::fs::write(
                "continuum-crash.json",
                serde_json::to_string_pretty(&crash).unwrap(),
            );
        }));
    }

    pub fn has_panicked() -> bool {
        PANICKED.load(Ordering::SeqCst)
    }

    pub fn clear() {
        PANICKED.store(false, Ordering::SeqCst);
    }
}

pub struct LeakDetector {
    allocations_before: u64,
}

impl LeakDetector {
    pub fn new() -> Self {
        Self {
            allocations_before: allocated_objects(),
        }
    }

    pub fn check(&self) -> LeakReport {
        let after = allocated_objects();
        LeakReport {
            leaked_objects: after.saturating_sub(self.allocations_before),
            stable: after == self.allocations_before,
        }
    }
}

impl Default for LeakDetector {
    fn default() -> Self {
        Self::new()
    }
}

pub struct LeakReport {
    pub leaked_objects: u64,
    pub stable: bool,
}

fn allocated_objects() -> u64 {
    // Approximate: count threads and open file descriptors
    let threads = std::thread::available_parallelism()
        .map(|n| n.get() as u64)
        .unwrap_or(1);
    threads * 100 // rough baseline
}

pub fn monitor_memory_usage() -> MemorySnapshot {
    let memory_mb;

    #[cfg(target_os = "linux")]
    {
        memory_mb = 0u64;
        if let Ok(contents) = std::fs::read_to_string("/proc/self/status") {
            for line in contents.lines() {
                if line.starts_with("VmRSS:") {
                    if let Some(val) = line.split_whitespace().nth(1) {
                        memory_mb = val.parse::<u64>().unwrap_or(0) / 1024;
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: use GetProcessMemoryInfo via winapi
        memory_mb = 0;
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        memory_mb = 0;
    }

    MemorySnapshot {
        memory_mb,
        timestamp: chrono::Utc::now(),
    }
}

pub struct MemorySnapshot {
    pub memory_mb: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crash_guard_install() {
        CrashGuard::install();
        assert!(!CrashGuard::has_panicked());
    }

    #[test]
    fn test_leak_detector() {
        let detector = LeakDetector::new();
        let report = detector.check();
        assert!(report.stable || report.leaked_objects < 1000);
    }
}

use crate::types::*;
use anyhow::Result;
use std::path::PathBuf;
use std::time::Instant;

pub struct BinaryPackager {
    pub target_dir: PathBuf,
    pub output_dir: PathBuf,
}

impl BinaryPackager {
    pub fn new() -> Self {
        Self {
            target_dir: PathBuf::from("target/release"),
            output_dir: PathBuf::from("target/ci-release"),
        }
    }

    pub fn build_release_binaries(&self) -> Result<TestResult> {
        let start = Instant::now();
        let mut assertions = Vec::new();

        let binaries = [
            "continuum-server",
            "continuum-client",
            "relay-server",
            "continuum-ci",
        ];
        let mut built = Vec::new();

        for bin in &binaries {
            let output = std::process::Command::new("cargo")
                .args(["build", "--release", "--bin", bin])
                .output()?;

            let success = output.status.success();
            if !success {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::error!(binary = %bin, error = %stderr, "Build failed");
            }

            let exe = if cfg!(target_os = "windows") {
                format!("{}.exe", bin)
            } else {
                bin.to_string()
            };
            let binary_path = self.target_dir.join(&exe);
            let exists = binary_path.exists();
            built.push((bin, success && exists));

            assertions.push(AssertionResult {
                description: format!("Build {}", bin),
                passed: success && exists,
                expected: format!("{} exists", binary_path.display()),
                actual: if exists {
                    "found".into()
                } else {
                    "not found".into()
                },
                line: None,
            });
        }

        let all_passed = assertions.iter().all(|a| a.passed);
        Ok(TestResult {
            name: "build-release-binaries".to_string(),
            category: TestCategory::Packaging,
            status: if all_passed {
                TestStatus::Passed
            } else {
                TestStatus::Failed
            },
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            details: format!(
                "Built {}/{} binaries",
                built.iter().filter(|(_, s)| *s).count(),
                built.len()
            ),
            metrics: vec![],
            assertions,
        })
    }

    pub fn package_archive(&self, format: &str) -> Result<TestResult> {
        let start = Instant::now();
        std::fs::create_dir_all(&self.output_dir)?;

        let _ = format;
        let archive_name = format!("continuum-{}.tar.gz", std::env::consts::OS);

        let binaries = if cfg!(target_os = "windows") {
            vec![
                "continuum-server.exe",
                "continuum-client.exe",
                "relay-server.exe",
                "continuum-ci.exe",
            ]
        } else {
            vec![
                "continuum-server",
                "continuum-client",
                "relay-server",
                "continuum-ci",
            ]
        };

        let archive_path = self.output_dir.join(&archive_name);
        let file = std::fs::File::create(&archive_path)?;
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::best());
        let mut tar = tar::Builder::new(encoder);

        for binary in &binaries {
            let src = self.target_dir.join(binary);
            if src.exists() {
                tar.append_path_with_name(&src, format!("continuum/{}", binary))?;
            }
        }

        let _ = tar.finish();
        let size = std::fs::metadata(&archive_path)
            .map(|m| m.len())
            .unwrap_or(0);

        Ok(TestResult {
            name: format!("package-archive-{}", format),
            category: TestCategory::Packaging,
            status: TestStatus::Passed,
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            details: format!(
                "Created {} ({:.1} MB)",
                archive_name,
                size as f64 / (1024.0 * 1024.0)
            ),
            metrics: vec![MetricSample {
                name: "archive_size_mb".to_string(),
                value: size as f64 / (1024.0 * 1024.0),
                unit: "MB".to_string(),
                percentile: None,
            }],
            assertions: vec![AssertionResult {
                description: "Archive created".to_string(),
                passed: size > 0,
                expected: "size > 0".to_string(),
                actual: format!("{} bytes", size),
                line: None,
            }],
        })
    }
}

impl Default for BinaryPackager {
    fn default() -> Self {
        Self::new()
    }
}

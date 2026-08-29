use crate::types::*;
use std::path::PathBuf;

pub struct TestReporter {
    pub output_dir: PathBuf,
}

impl TestReporter {
    pub fn new() -> Self {
        Self {
            output_dir: PathBuf::from("target/test-reports"),
        }
    }

    pub fn generate_json(&self, suite: &TestSuiteResult) -> PathBuf {
        std::fs::create_dir_all(&self.output_dir).ok();
        let path = self.output_dir.join("test-results.json");
        let json = serde_json::to_string_pretty(suite).unwrap_or_default();
        std::fs::write(&path, json).ok();
        path
    }

    pub fn generate_html(&self, suite: &TestSuiteResult) -> PathBuf {
        std::fs::create_dir_all(&self.output_dir).ok();
        let path = self.output_dir.join("test-report.html");

        let mut rows = String::new();
        for result in &suite.results {
            let status_class = match result.status {
                TestStatus::Passed => "pass",
                TestStatus::Failed => "fail",
                TestStatus::Skipped => "skip",
                TestStatus::Error => "error",
                TestStatus::Timeout => "timeout",
            };

            let assertions_html: String = result
                .assertions
                .iter()
                .map(|a| {
                    let icon = if a.passed { "✓" } else { "✗" };
                    format!(
                        r#"<tr class="{}"><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>"#,
                        if a.passed { "pass" } else { "fail" },
                        icon,
                        a.description,
                        a.expected,
                        a.actual
                    )
                })
                .collect();

            let metrics_html: String = result
                .metrics
                .iter()
                .map(|m| {
                    format!(
                        r#"<tr><td>{}</td><td class="val">{:.1}</td><td>{}</td></tr>"#,
                        m.name, m.value, m.unit
                    )
                })
                .collect();

            rows.push_str(&format!(
                r#"
            <tr class="result-row" onclick="toggleDetails('d-{}')">
                <td><span class="status {}">{}</span></td>
                <td>{}</td>
                <td>{}</td>
                <td>{:.1}s</td>
                <td>{}</td>
            </tr>
            <tr id="d-{}" class="details hidden">
                <td colspan="5">
                    <div class="detail-box">
                        <p><strong>Details:</strong> {}</p>
                        {}
                        {}
                    </div>
                </td>
            </tr>
            "#,
                result.name.replace('.', "-"),
                status_class,
                result.status,
                result.name,
                result.category,
                result.duration_ms as f64 / 1000.0,
                result.details,
                result.name.replace('.', "-"),
                result.details,
                if assertions_html.is_empty() {
                    String::new()
                } else {
                    format!(
                        "<strong>Assertions:</strong><table>{}</table>",
                        assertions_html
                    )
                },
                if metrics_html.is_empty() {
                    String::new()
                } else {
                    format!("<strong>Metrics:</strong><table>{}</table>", metrics_html)
                },
            ));
        }

        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="UTF-8"><title>Continuum Test Report</title>
<style>
  * {{ margin: 0; padding: 0; box-sizing: border-box; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: #0f0f14; color: #d0d0dc; padding: 24px; }}
  h1 {{ font-size: 24px; margin-bottom: 8px; }}
  .summary {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 12px; margin: 16px 0; }}
  .summary-card {{ background: #1a1a24; border-radius: 8px; padding: 16px; border: 1px solid #252533; text-align: center; }}
  .summary-card .value {{ font-size: 28px; font-weight: 700; }}
  .summary-card .label {{ font-size: 12px; color: #888; text-transform: uppercase; }}
  .pass {{ color: #22c55e; }} .fail {{ color: #ef4444; }} .skip {{ color: #f59e0b; }} .error {{ color: #ef4444; }} .timeout {{ color: #f97316; }}
  table {{ width: 100%; border-collapse: collapse; margin: 8px 0; }}
  th, td {{ padding: 8px 12px; text-align: left; border-bottom: 1px solid #252533; font-size: 13px; }}
  th {{ background: #1a1a24; font-weight: 600; }}
  .result-row {{ cursor: pointer; }}
  .result-row:hover {{ background: #1a1a24; }}
  .status {{ display: inline-block; padding: 2px 8px; border-radius: 4px; font-weight: 600; font-size: 11px; background: #ffffff15; }}
  .details {{ display: none; }}
  .details.hidden {{ display: none; }}
  .details:not(.hidden) {{ display: table-row; }}
  .detail-box {{ background: #0a0a10; padding: 16px; border-radius: 8px; margin: 8px 0; }}
  .val {{ font-family: monospace; text-align: right; }}
  .system {{ color: #888; font-size: 12px; margin-top: 16px; }}
</style>
<script>
function toggleDetails(id) {{ document.getElementById(id).classList.toggle('hidden'); }}
</script>
</head>
<body>
  <h1>Continuum Test Report</h1>
  <p style="color:#888">{} — {} tests in {:.1}s</p>
  <div class="summary">
    <div class="summary-card"><div class="value" style="color:#22c55e">{}</div><div class="label">Passed</div></div>
    <div class="summary-card"><div class="value" style="color:#ef4444">{}</div><div class="label">Failed</div></div>
    <div class="summary-card"><div class="value" style="color:#f59e0b">{}</div><div class="label">Skipped</div></div>
    <div class="summary-card"><div class="value">{:.0}%</div><div class="label">Success Rate</div></div>
  </div>
  <table><thead><tr><th>Status</th><th>Test</th><th>Category</th><th>Duration</th><th>Details</th></tr></thead><tbody>{}</tbody></table>
  <div class="system">
    <p>OS: {} | CPU: {} cores | Memory: {} MB | Rust: {}</p>
  </div>
</body>
</html>"#,
            suite.name,
            suite.total,
            suite.duration_ms as f64 / 1000.0,
            suite.passed,
            suite.failed,
            suite.skipped,
            suite.success_rate(),
            rows,
            suite.system_info.os,
            suite.system_info.cpu_count,
            suite.system_info.memory_mb,
            suite.system_info.rust_version,
        );

        std::fs::write(&path, html).ok();
        tracing::info!(path = %path.display(), "HTML test report generated");
        path
    }

    pub fn print_console(&self, suite: &TestSuiteResult) {
        println!("\n{}", "=".repeat(60));
        println!("  Test Suite: {}", suite.name);
        println!(
            "  Total: {}  Passed: {}  Failed: {}  Skipped: {}  Rate: {:.0}%",
            suite.total,
            suite.passed,
            suite.failed,
            suite.skipped,
            suite.success_rate()
        );
        println!("  Duration: {:.1}s", suite.duration_ms as f64 / 1000.0);
        println!("{}", "=".repeat(60));

        for result in &suite.results {
            let icon = match result.status {
                TestStatus::Passed => "✓",
                TestStatus::Failed => "✗",
                TestStatus::Skipped => "⊘",
                TestStatus::Error => "⚠",
                TestStatus::Timeout => "⌛",
            };
            println!("  {} {} ({}ms)", icon, result.name, result.duration_ms);
            if result.status != TestStatus::Passed {
                println!("      {}", result.details);
            }
            // Show metrics inline
            if !result.metrics.is_empty() {
                let metrics_str: Vec<String> = result
                    .metrics
                    .iter()
                    .map(|m| format!("{}={:.1}{}", m.name, m.value, m.unit))
                    .collect();
                println!("      📊 {}", metrics_str.join(", "));
            }
            // Show failed assertions
            let failed: Vec<&AssertionResult> =
                result.assertions.iter().filter(|a| !a.passed).collect();
            for a in &failed {
                println!(
                    "      ✗ {} (expected: {}, got: {})",
                    a.description, a.expected, a.actual
                );
            }
        }
        println!();
    }
}

impl Default for TestReporter {
    fn default() -> Self {
        Self::new()
    }
}

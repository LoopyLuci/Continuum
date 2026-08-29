use crate::harness::TestHarness;
use crate::types::*;
use std::time::Instant;

pub fn run_latency_benchmark(_harness: &mut TestHarness) -> TestResult {
    let start = Instant::now();
    let mut samples = Vec::new();

    // In-process benchmark: measure encode + decode roundtrip
    let img = continuum_transport::capture::synthesize_demo_frame();
    let mut encoder = continuum_transport::codec::AdaptiveEncoder::new(85, 30);

    for _ in 0..100 {
        let t0 = Instant::now();
        if let Ok((data, _)) = encoder.encode(&img, 0) {
            if !data.is_empty() {
                if let Ok(_decoded) = continuum_transport::codec::decode_jpeg(&data) {
                    samples.push(t0.elapsed().as_micros() as f64);
                }
            }
        }
    }

    if samples.is_empty() {
        return TestResult {
            name: "benchmark-encode-decode-latency".to_string(),
            category: TestCategory::Performance,
            status: TestStatus::Error,
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: chrono::Utc::now(),
            finished_at: None,
            details: "No valid samples collected".to_string(),
            metrics: vec![],
            assertions: vec![],
        };
    }

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = samples.iter().sum();
    let avg = total / samples.len() as f64;
    let p50 = samples[samples.len() / 2];
    let p95_idx = (samples.len() as f64 * 0.95).ceil() as usize - 1;
    let p95 = samples[p95_idx.min(samples.len() - 1)];
    let p99_idx = (samples.len() as f64 * 0.99).ceil() as usize - 1;
    let p99 = samples[p99_idx.min(samples.len() - 1)];
    let max = samples[samples.len() - 1];
    let mut fps_estimate = 0.0f64;
    if avg > 0.0 {
        fps_estimate = 1_000_000.0 / avg;
    }

    TestResult {
        name: "benchmark-encode-decode-latency".to_string(),
        category: TestCategory::Performance,
        status: TestStatus::Passed,
        duration_ms: start.elapsed().as_millis() as u64,
        started_at: chrono::Utc::now(),
        finished_at: Some(chrono::Utc::now()),
        details: format!("Encode+decode pipeline: avg={:.0}µs p50={:.0}µs p95={:.0}µs p99={:.0}µs max={:.0}µs (est. {:.0} FPS)", avg, p50, p95, p99, max, fps_estimate),
        metrics: vec![
            MetricSample { name: "encode_decode_avg_us".to_string(), value: avg, unit: "µs".to_string(), percentile: None },
            MetricSample { name: "encode_decode_p50_us".to_string(), value: p50, unit: "µs".to_string(), percentile: Some(50.0) },
            MetricSample { name: "encode_decode_p95_us".to_string(), value: p95, unit: "µs".to_string(), percentile: Some(95.0) },
            MetricSample { name: "encode_decode_p99_us".to_string(), value: p99, unit: "µs".to_string(), percentile: Some(99.0) },
            MetricSample { name: "encode_decode_max_us".to_string(), value: max, unit: "µs".to_string(), percentile: None },
            MetricSample { name: "estimated_fps".to_string(), value: fps_estimate, unit: "FPS".to_string(), percentile: None },
        ],
        assertions: vec![
            AssertionResult {
                description: "Encode+decode latency < 50ms".to_string(),
                passed: avg < 50_000.0,
                expected: "avg < 50000 µs".to_string(),
                actual: format!("{:.0} µs", avg),
                line: None,
            },
            AssertionResult {
                description: "Estimated FPS >= 20".to_string(),
                passed: fps_estimate >= 20.0,
                expected: ">= 20 FPS".to_string(),
                actual: format!("{:.0} FPS", fps_estimate),
                line: None,
            },
        ],
    }
}

pub fn run_frame_size_benchmark(_harness: &mut TestHarness) -> TestResult {
    let start = Instant::now();
    let img = continuum_transport::capture::synthesize_demo_frame();

    let qualities = [30u8, 50, 70, 85, 95];
    let mut results = Vec::new();

    for &q in &qualities {
        let mut total = 0u64;
        let mut count = 0u64;
        for _ in 0..10 {
            if let Ok(data) = continuum_transport::codec::encode_jpeg(&img, q) {
                total += data.len() as u64;
                count += 1;
            }
        }
        if let Some(avg) = total.checked_div(count) {
            results.push((q, avg));
        }
    }

    let passed = !results.is_empty();
    TestResult {
        name: "benchmark-frame-sizes".to_string(),
        category: TestCategory::Performance,
        status: if passed {
            TestStatus::Passed
        } else {
            TestStatus::Error
        },
        duration_ms: start.elapsed().as_millis() as u64,
        started_at: chrono::Utc::now(),
        finished_at: Some(chrono::Utc::now()),
        details: format!(
            "Frame sizes: {}",
            results
                .iter()
                .map(|(q, s)| format!("q{}={}KB", q, s / 1024))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        metrics: results
            .iter()
            .map(|(q, s)| MetricSample {
                name: format!("frame_size_q{}", q),
                value: *s as f64 / 1024.0,
                unit: "KB".to_string(),
                percentile: None,
            })
            .collect(),
        assertions: vec![AssertionResult {
            description: "Frame size measurement completed".to_string(),
            passed,
            expected: "samples > 0".to_string(),
            actual: format!("{} quality levels", results.len()),
            line: None,
        }],
    }
}

use std::time::Instant;

pub struct PipelineBenchmark {
    pub capture_time_us: Vec<u64>,
    pub encode_time_us: Vec<u64>,
    pub encrypt_time_us: Vec<u64>,
    pub decrypt_time_us: Vec<u64>,
    pub decode_time_us: Vec<u64>,
    pub roundtrip_us: Vec<u64>,
    pub frame_sizes: Vec<u64>,
    pub fps_samples: Vec<f64>,
    pub start: Instant,
}

impl PipelineBenchmark {
    pub fn new() -> Self {
        Self {
            capture_time_us: Vec::new(),
            encode_time_us: Vec::new(),
            encrypt_time_us: Vec::new(),
            decrypt_time_us: Vec::new(),
            decode_time_us: Vec::new(),
            roundtrip_us: Vec::new(),
            frame_sizes: Vec::new(),
            fps_samples: Vec::new(),
            start: Instant::now(),
        }
    }

    pub fn record_capture(&mut self, us: u64) {
        self.capture_time_us.push(us);
    }
    pub fn record_encode(&mut self, us: u64, bytes: u64) {
        self.encode_time_us.push(us);
        self.frame_sizes.push(bytes);
    }
    pub fn record_encrypt(&mut self, us: u64) {
        self.encrypt_time_us.push(us);
    }
    pub fn record_decrypt(&mut self, us: u64) {
        self.decrypt_time_us.push(us);
    }
    pub fn record_decode(&mut self, us: u64) {
        self.decode_time_us.push(us);
    }
    pub fn record_frame(&mut self) {
        let elapsed = self.start.elapsed().as_secs_f64();
        self.fps_samples.push(1.0 / elapsed.max(0.001));
        self.start = Instant::now();
    }

    pub fn record_roundtrip(&mut self, us: u64) {
        self.roundtrip_us.push(us);
    }

    pub fn stats(&self) -> BenchmarkResult {
        fn percentile(data: &[u64], pct: f64) -> f64 {
            if data.is_empty() {
                return 0.0;
            }
            let mut sorted = data.to_vec();
            sorted.sort();
            let idx = (sorted.len() as f64 * pct / 100.0).ceil() as usize - 1;
            *sorted.get(idx.min(sorted.len() - 1)).unwrap_or(&0) as f64
        }

        fn avg(data: &[u64]) -> f64 {
            if data.is_empty() {
                0.0
            } else {
                data.iter().sum::<u64>() as f64 / data.len() as f64
            }
        }

        fn max_val(data: &[u64]) -> u64 {
            *data.iter().max().unwrap_or(&0)
        }

        BenchmarkResult {
            num_samples: self.encode_time_us.len(),
            capture_avg_us: avg(&self.capture_time_us),
            capture_p99_us: percentile(&self.capture_time_us, 99.0),
            encode_avg_us: avg(&self.encode_time_us),
            encode_p50_us: percentile(&self.encode_time_us, 50.0),
            encode_p95_us: percentile(&self.encode_time_us, 95.0),
            encode_p99_us: percentile(&self.encode_time_us, 99.0),
            encode_max_us: max_val(&self.encode_time_us) as f64,
            encrypt_avg_us: avg(&self.encrypt_time_us),
            decrypt_avg_us: avg(&self.decrypt_time_us),
            decode_avg_us: avg(&self.decode_time_us),
            roundtrip_avg_us: avg(&self.roundtrip_us),
            roundtrip_p95_us: percentile(&self.roundtrip_us, 95.0),
            frame_size_avg: avg(&self.frame_sizes) as u64,
            frame_size_max: max_val(&self.frame_sizes),
            avg_fps: if self.fps_samples.is_empty() {
                0.0
            } else {
                self.fps_samples.iter().sum::<f64>() / self.fps_samples.len() as f64
            },
        }
    }

    pub fn print_report(&self) {
        let s = self.stats();
        let divider = "─".repeat(60);

        println!("\n{}", divider);
        println!("  Continuum Pipeline Benchmark Report");
        println!("  Samples: {}", s.num_samples);
        println!("{}", divider);
        println!("  Phase            Avg       P50       P95       P99       Max");
        println!("{}", divider);
        print_row("Capture", s.capture_avg_us, 0.0, 0.0, s.capture_p99_us, 0.0);
        print_row(
            "Encode",
            s.encode_avg_us,
            s.encode_p50_us,
            s.encode_p95_us,
            s.encode_p99_us,
            s.encode_max_us,
        );
        print_row("Encrypt", s.encrypt_avg_us, 0.0, 0.0, 0.0, 0.0);
        print_row("Decrypt", s.decrypt_avg_us, 0.0, 0.0, 0.0, 0.0);
        print_row("Decode", s.decode_avg_us, 0.0, 0.0, 0.0, 0.0);
        print_row(
            "Roundtrip",
            s.roundtrip_avg_us,
            0.0,
            s.roundtrip_p95_us,
            0.0,
            0.0,
        );
        println!("{}", divider);
        println!(
            "  Frame size:  avg={} KB, max={} KB",
            s.frame_size_avg / 1024,
            s.frame_size_max / 1024
        );
        println!("  Average FPS: {:.1}", s.avg_fps);
        println!("{}", divider);
    }
}

fn print_row(label: &str, avg: f64, p50: f64, p95: f64, p99: f64, max: f64) {
    let fmt = |v: f64| -> String {
        if v == 0.0 {
            "    —    ".to_string()
        } else {
            format!("{:>8.1}", v)
        }
    };
    println!(
        "  {:<15} {} {} {} {} {} μs",
        label,
        fmt(avg),
        fmt(p50),
        fmt(p95),
        fmt(p99),
        fmt(max)
    );
}

#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub num_samples: usize,
    pub capture_avg_us: f64,
    pub capture_p99_us: f64,
    pub encode_avg_us: f64,
    pub encode_p50_us: f64,
    pub encode_p95_us: f64,
    pub encode_p99_us: f64,
    pub encode_max_us: f64,
    pub encrypt_avg_us: f64,
    pub decrypt_avg_us: f64,
    pub decode_avg_us: f64,
    pub roundtrip_avg_us: f64,
    pub roundtrip_p95_us: f64,
    pub frame_size_avg: u64,
    pub frame_size_max: u64,
    pub avg_fps: f64,
}

impl Default for PipelineBenchmark {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benchmark_tracking() {
        let mut bm = PipelineBenchmark::new();
        for i in 0..100 {
            bm.record_capture(500 + i * 2);
            bm.record_encode(3000 + i * 5, 50000 + i * 100);
            bm.record_encrypt(100);
            bm.record_decrypt(80);
            bm.record_decode(200);
            bm.record_frame();
        }
        let stats = bm.stats();
        assert_eq!(stats.num_samples, 100);
        assert!(stats.encode_avg_us > 3000.0);
        assert!(stats.frame_size_avg > 50000);
    }

    #[test]
    fn test_empty_benchmark() {
        let bm = PipelineBenchmark::new();
        let stats = bm.stats();
        assert_eq!(stats.num_samples, 0);
    }

    #[test]
    fn test_benchmark_record_roundtrip() {
        let mut bm = PipelineBenchmark::new();
        bm.record_roundtrip(1000);
        bm.record_roundtrip(2000);
        let stats = bm.stats();
        assert_eq!(stats.roundtrip_avg_us, 1500.0);
    }

    #[test]
    fn test_benchmark_frame_resets_timer() {
        let mut bm = PipelineBenchmark::new();
        bm.record_frame();
        bm.record_frame();
        assert!(bm.fps_samples.len() >= 2);
    }

    #[test]
    fn test_benchmark_stats_empty() {
        let bm = PipelineBenchmark::new();
        let stats = bm.stats();
        assert_eq!(stats.capture_avg_us, 0.0);
        assert_eq!(stats.encode_avg_us, 0.0);
        assert_eq!(stats.encrypt_avg_us, 0.0);
        assert_eq!(stats.decrypt_avg_us, 0.0);
        assert_eq!(stats.decode_avg_us, 0.0);
        assert_eq!(stats.frame_size_avg, 0);
        assert_eq!(stats.avg_fps, 0.0);
    }
}

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

pub struct Performance {
    report: Option<PathBuf>,
    frames_ms: Vec<f64>,
}

impl Default for Performance {
    fn default() -> Self {
        Self {
            report: std::env::var_os("CORTEX_PERF_REPORT").map(PathBuf::from),
            frames_ms: Vec::new(),
        }
    }
}

impl Performance {
    pub fn record(&mut self, context: &egui::Context, elapsed: Duration) {
        if self.report.is_none() {
            return;
        }
        if self.frames_ms.len() < 100_000 {
            self.frames_ms.push(elapsed.as_secs_f64() * 1000.0);
        }
        context.request_repaint();
    }
}

impl Drop for Performance {
    fn drop(&mut self) {
        let Some(path) = &self.report else {
            return;
        };
        if self.frames_ms.is_empty() {
            return;
        }
        self.frames_ms.sort_by(f64::total_cmp);
        let percentile =
            |percent: usize| self.frames_ms[((self.frames_ms.len() - 1) * percent) / 100];
        let report = serde_json::json!({
            "metric": "CPU time per complete workspace UI frame; excludes GPU presentation",
            "frames": self.frames_ms.len(),
            "p50_ms": percentile(50), "p95_ms": percentile(95), "p99_ms": percentile(99),
            "max_ms": self.frames_ms.last(),
            "frames_over_16_7_ms": self.frames_ms.iter().filter(|&&ms| ms > 16.7).count(),
        });
        if let Err(error) = std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()) {
            eprintln!("cannot save frame timings: {error}");
        }
    }
}

// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use rand::Rng;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const MAX_LATENCY_SAMPLES: usize = 10_000;

#[derive(Debug, Clone)]
pub enum OperationType {
    OneShot,
    SessionCreate,
    SessionDestroy,
    NodeTune,
}

impl std::fmt::Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OneShot => write!(f, "OneShot"),
            Self::SessionCreate => write!(f, "SessionCreate"),
            Self::SessionDestroy => write!(f, "SessionDestroy"),
            Self::NodeTune => write!(f, "NodeTune"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OperationResult {
    pub op_type: OperationType,
    pub latency: Duration,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug)]
struct OperationStats {
    count: usize,
    success_count: usize,
    latency_sum: Duration,
    min_latency: Option<Duration>,
    max_latency: Option<Duration>,
    latency_samples: Vec<Duration>,
    errors: HashMap<String, usize>,
}

impl OperationStats {
    fn new() -> Self {
        Self {
            count: 0,
            success_count: 0,
            latency_sum: Duration::ZERO,
            min_latency: None,
            max_latency: None,
            latency_samples: Vec::new(),
            errors: HashMap::new(),
        }
    }

    fn record(&mut self, result: &OperationResult) {
        self.count += 1;
        if result.success {
            self.success_count += 1;
            self.latency_sum += result.latency;

            self.min_latency =
                Some(self.min_latency.map_or(result.latency, |d| d.min(result.latency)));
            self.max_latency =
                Some(self.max_latency.map_or(result.latency, |d| d.max(result.latency)));

            // Reservoir sampling to keep a bounded latency sample set for percentiles.
            if self.latency_samples.len() < MAX_LATENCY_SAMPLES {
                self.latency_samples.push(result.latency);
            } else {
                // Cast acceptable: indices are bounded and this is only for sampling.
                #[allow(clippy::cast_possible_truncation)]
                let j = rand::rng().random_range(0..self.success_count);
                if j < MAX_LATENCY_SAMPLES {
                    self.latency_samples[j] = result.latency;
                }
            }
        } else if let Some(err) = &result.error {
            *self.errors.entry(err.clone()).or_insert(0) += 1;
        }
    }

    fn success_rate(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)] // Intentional for percentage calculation
            {
                (self.success_count as f64 / self.count as f64) * 100.0
            }
        }
    }

    fn percentile(&self, p: f64) -> Option<Duration> {
        if self.latency_samples.is_empty() {
            return None;
        }
        let mut sorted = self.latency_samples.clone();
        sorted.sort();
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        // Intentional: computing percentile index requires float arithmetic, result is always positive and within bounds
        let idx = ((p / 100.0) * sorted.len() as f64).ceil() as usize - 1;
        sorted.get(idx.min(sorted.len() - 1)).copied()
    }

    fn mean(&self) -> Option<Duration> {
        if self.success_count == 0 {
            return None;
        }
        // Safe cast: We're computing statistics, and having more than u32::MAX samples is unrealistic
        // In practice, we'd run out of memory long before reaching 4 billion samples
        #[allow(clippy::cast_possible_truncation)]
        let count = self.success_count as u32;
        Some(self.latency_sum / count)
    }

    const fn min(&self) -> Option<Duration> {
        self.min_latency
    }

    const fn max(&self) -> Option<Duration> {
        self.max_latency
    }
}

#[derive(Clone)]
pub struct MetricsCollector {
    start_time: Instant,
    stats: Arc<Mutex<HashMap<String, OperationStats>>>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self { start_time: Instant::now(), stats: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub async fn record(&self, result: OperationResult) {
        let mut stats = self.stats.lock().await;
        let key = result.op_type.to_string();
        stats.entry(key).or_insert_with(OperationStats::new).record(&result);
    }

    pub async fn get_snapshot(&self) -> MetricsSnapshot {
        let elapsed = self.start_time.elapsed();

        let mut total_ops = 0;
        let mut total_success = 0;
        let mut all_latencies = Vec::new();
        let mut all_errors: HashMap<String, usize> = HashMap::new();

        // Release lock early to avoid holding it during expensive calculations
        {
            let stats = self.stats.lock().await;
            for stat in stats.values() {
                total_ops += stat.count;
                total_success += stat.success_count;
                all_latencies.extend(&stat.latency_samples);
                for (err, count) in &stat.errors {
                    *all_errors.entry(err.clone()).or_insert(0) += *count;
                }
            }
        }

        all_latencies.sort();

        // Precision loss is acceptable for throughput/success rate display metrics
        // f64 mantissa (52 bits) can precisely represent integers up to 2^53 (~9 quadrillion)
        // which is far beyond realistic operation counts in a load test
        #[allow(clippy::cast_precision_loss)]
        let throughput = total_ops as f64 / elapsed.as_secs_f64();

        #[allow(clippy::cast_precision_loss)]
        let success_rate =
            if total_ops > 0 { (total_success as f64 / total_ops as f64) * 100.0 } else { 0.0 };

        let total_failures = total_ops.saturating_sub(total_success);
        let top_error = all_errors.into_iter().max_by_key(|(_, c)| *c);

        MetricsSnapshot {
            elapsed,
            total_ops,
            total_success,
            total_failures,
            throughput,
            success_rate,
            p50: Self::calc_percentile(&all_latencies, 50.0),
            p95: Self::calc_percentile(&all_latencies, 95.0),
            p99: Self::calc_percentile(&all_latencies, 99.0),
            top_error,
        }
    }

    fn calc_percentile(sorted_latencies: &[Duration], p: f64) -> Option<Duration> {
        if sorted_latencies.is_empty() {
            return None;
        }
        // Percentile calculation: cast to f64, compute position, cast back to usize
        // Precision loss acceptable: computing percentile index, not exact arithmetic
        // Sign loss acceptable: ceil() always returns positive for our inputs (0-100 percentile)
        // Truncation acceptable: result is bounded by sorted_latencies.len()
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation
        )]
        let idx = ((p / 100.0) * sorted_latencies.len() as f64).ceil() as usize - 1;
        sorted_latencies.get(idx.min(sorted_latencies.len() - 1)).copied()
    }

    pub async fn finalize(self) -> FinalMetrics {
        let elapsed = self.start_time.elapsed();

        let mut by_operation = HashMap::new();

        // Release lock early to avoid holding it unnecessarily
        {
            let stats = self.stats.lock().await;
            for (op_type, stat) in stats.iter() {
                by_operation.insert(
                    op_type.clone(),
                    OperationMetrics {
                        count: stat.count,
                        success_count: stat.success_count,
                        success_rate: stat.success_rate(),
                        min: stat.min(),
                        max: stat.max(),
                        mean: stat.mean(),
                        p50: stat.percentile(50.0),
                        p95: stat.percentile(95.0),
                        p99: stat.percentile(99.0),
                        errors: stat.errors.clone(),
                    },
                );
            }
        }

        FinalMetrics { elapsed, by_operation }
    }
}

#[derive(Debug)]
pub struct MetricsSnapshot {
    pub elapsed: Duration,
    pub total_ops: usize,
    #[allow(dead_code)]
    pub total_success: usize,
    pub total_failures: usize,
    pub throughput: f64,
    pub success_rate: f64,
    pub p50: Option<Duration>,
    pub p95: Option<Duration>,
    pub p99: Option<Duration>,
    pub top_error: Option<(String, usize)>,
}

impl std::fmt::Display for MetricsSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Helper to format duration as milliseconds, showing 0 if None
        let fmt_ms = |d: Option<Duration>| d.map_or(0, |dur| dur.as_millis());
        let top_err = self.top_error.as_ref().map(|(e, c)| {
            let mut s = e.as_str();
            if s.len() > 80 {
                s = &e[..80];
            }
            format!("{c}x {s}")
        });

        write!(
            f,
            "[{:>4}s] Ops: {:>6} | Success: {:>5.1}% | Fail: {:>6} | p50: {:>4}ms p95: {:>4}ms p99: {:>4}ms | Rate: {:>6.1}/s{}",
            self.elapsed.as_secs(),
            self.total_ops,
            self.success_rate,
            self.total_failures,
            fmt_ms(self.p50),
            fmt_ms(self.p95),
            fmt_ms(self.p99),
            self.throughput,
            top_err.as_ref().map_or(String::new(), |s| format!(" | Err: {s}")),
        )
    }
}

#[derive(Debug)]
pub struct OperationMetrics {
    pub count: usize,
    pub success_count: usize,
    pub success_rate: f64,
    pub min: Option<Duration>,
    pub max: Option<Duration>,
    pub mean: Option<Duration>,
    pub p50: Option<Duration>,
    pub p95: Option<Duration>,
    pub p99: Option<Duration>,
    pub errors: HashMap<String, usize>,
}

pub struct FinalMetrics {
    pub elapsed: Duration,
    pub by_operation: HashMap<String, OperationMetrics>,
}

impl FinalMetrics {
    pub fn print_summary(&self) {
        println!("Total Duration: {:.2}s", self.elapsed.as_secs_f64());
        println!();

        if self.by_operation.is_empty() {
            println!("No operations recorded.");
            return;
        }

        for (op_type, metrics) in &self.by_operation {
            println!("Operation: {op_type}");
            println!("  Total:        {}", metrics.count);
            println!("  Success:      {} ({:.2}%)", metrics.success_count, metrics.success_rate);
            println!("  Failed:       {}", metrics.count - metrics.success_count);

            if metrics.success_count > 0 {
                println!("  Latency:");
                println!("    Min:  {:>6}ms", metrics.min.map_or(0, |d| d.as_millis()));
                println!("    Max:  {:>6}ms", metrics.max.map_or(0, |d| d.as_millis()));
                println!("    Mean: {:>6}ms", metrics.mean.map_or(0, |d| d.as_millis()));
                println!("    p50:  {:>6}ms", metrics.p50.map_or(0, |d| d.as_millis()));
                println!("    p95:  {:>6}ms", metrics.p95.map_or(0, |d| d.as_millis()));
                println!("    p99:  {:>6}ms", metrics.p99.map_or(0, |d| d.as_millis()));
                #[allow(clippy::cast_precision_loss)] // Intentional for throughput calculation
                {
                    println!(
                        "  Throughput:   {:.2} ops/sec",
                        metrics.success_count as f64 / self.elapsed.as_secs_f64()
                    );
                }
            }

            if !metrics.errors.is_empty() {
                println!("  Errors:");
                for (err, count) in &metrics.errors {
                    println!("    [{count}x] {err}");
                }
            }

            println!();
        }
    }

    pub fn as_report(&self) -> FinalMetricsReport {
        let mut by_operation = HashMap::new();
        for (op_type, m) in &self.by_operation {
            let error_count = m.errors.values().copied().sum::<usize>();
            let elapsed_secs = self.elapsed.as_secs_f64();
            // Precision loss is acceptable: this is a display metric and counts won't reach f64's
            // integer precision limits in realistic load tests.
            #[allow(clippy::cast_precision_loss)]
            let throughput_success_per_sec =
                if elapsed_secs > 0.0 { m.success_count as f64 / elapsed_secs } else { 0.0 };

            by_operation.insert(
                op_type.clone(),
                OperationMetricsReport {
                    count: m.count,
                    success_count: m.success_count,
                    success_rate: m.success_rate,
                    error_count,
                    min_ms: duration_ms_opt(m.min),
                    max_ms: duration_ms_opt(m.max),
                    mean_ms: duration_ms_opt(m.mean),
                    p50_ms: duration_ms_opt(m.p50),
                    p95_ms: duration_ms_opt(m.p95),
                    p99_ms: duration_ms_opt(m.p99),
                    throughput_success_per_sec,
                    errors: m.errors.clone(),
                },
            );
        }

        FinalMetricsReport { elapsed_secs: self.elapsed.as_secs_f64(), by_operation }
    }

    pub fn as_csv(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        out.push_str("operation,count,success_count,success_rate,error_count,min_ms,max_ms,mean_ms,p50_ms,p95_ms,p99_ms,throughput_success_per_sec\n");

        let elapsed = self.elapsed.as_secs_f64();
        for (op_type, m) in &self.by_operation {
            let error_count = m.errors.values().copied().sum::<usize>();
            // Precision loss is acceptable: this is a display metric and counts won't reach f64's
            // integer precision limits in realistic load tests.
            #[allow(clippy::cast_precision_loss)]
            let throughput_success_per_sec =
                if elapsed > 0.0 { m.success_count as f64 / elapsed } else { 0.0 };

            let _ = writeln!(
                out,
                "{},{},{},{:.4},{},{},{},{},{},{},{},{:.6}",
                op_type,
                m.count,
                m.success_count,
                m.success_rate,
                error_count,
                duration_ms_opt(m.min).unwrap_or(0),
                duration_ms_opt(m.max).unwrap_or(0),
                duration_ms_opt(m.mean).unwrap_or(0),
                duration_ms_opt(m.p50).unwrap_or(0),
                duration_ms_opt(m.p95).unwrap_or(0),
                duration_ms_opt(m.p99).unwrap_or(0),
                throughput_success_per_sec,
            );
        }

        out
    }
}

fn duration_ms_saturated(dur: Duration) -> u64 {
    u64::try_from(dur.as_millis()).unwrap_or(u64::MAX)
}

fn duration_ms_opt(dur: Option<Duration>) -> Option<u64> {
    dur.map(duration_ms_saturated)
}

#[derive(Debug, Serialize)]
pub struct FinalMetricsReport {
    pub elapsed_secs: f64,
    pub by_operation: HashMap<String, OperationMetricsReport>,
}

#[derive(Debug, Serialize)]
pub struct OperationMetricsReport {
    pub count: usize,
    pub success_count: usize,
    pub success_rate: f64,
    pub error_count: usize,
    pub min_ms: Option<u64>,
    pub max_ms: Option<u64>,
    pub mean_ms: Option<u64>,
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
    pub p99_ms: Option<u64>,
    pub throughput_success_per_sec: f64,
    pub errors: HashMap<String, usize>,
}

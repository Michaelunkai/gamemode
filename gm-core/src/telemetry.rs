//! Rich machine telemetry: real GPU load, RAM, CPU, and per-process tables.
//!
//! `monitor.rs` answers "which process is the game". This module answers "what
//! is the machine doing right now", which is what the dashboard draws. Both are
//! refreshed on the same cadence so a single dashboard frame sees one coherent
//! sample.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::win::perf;

/// How many historical samples each series keeps.
const HISTORY: usize = 120;

/// A point-in-time reading of the whole machine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MachineStats {
    /// 0-100 total system CPU.
    pub cpu_percent: f32,
    /// 0-100 real GPU utilisation, from the `\GPU Engine` counters.
    pub gpu_percent: f32,
    /// Number of GPU engines that reported data.
    pub gpu_engines: u32,
    /// Physical RAM used, in bytes.
    pub ram_used: u64,
    /// Physical RAM available, in bytes.
    pub ram_available: u64,
    /// Total physical RAM, in bytes.
    pub ram_total: u64,
    /// Logical CPU count of this machine.
    pub cpu_count: u32,
    /// CPU model name read from the registry.
    pub cpu_name: String,
}

impl MachineStats {
    /// RAM in use as a percentage of the installed total.
    pub fn ram_percent(&self) -> f32 {
        if self.ram_total == 0 {
            return 0.0;
        }
        ((self.ram_used as f64 / self.ram_total as f64) * 100.0) as f32
    }

    /// RAM in use expressed in gigabytes.
    pub fn ram_used_gb(&self) -> f64 {
        self.ram_used as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    pub fn ram_total_gb(&self) -> f64 {
        self.ram_total as f64 / (1024.0 * 1024.0 * 1024.0)
    }
}

/// A rolling history of one machine statistic, used for the sparklines.
#[derive(Debug, Clone, Default)]
pub struct Series {
    pub values: VecDeque<f32>,
}

impl Series {
    pub fn push(&mut self, v: f32) {
        if self.values.len() == HISTORY {
            self.values.pop_front();
        }
        self.values.push_back(v);
    }

    pub fn as_vec(&self) -> Vec<f32> {
        self.values.iter().copied().collect()
    }

    pub fn newest(&self) -> f32 {
        self.values.back().copied().unwrap_or(0.0)
    }
}

/// One row of the top-processes table.
#[derive(Debug, Clone)]
pub struct ProcRow {
    pub pid: u32,
    pub exe: String,
    pub cpu_percent: f32,
    pub ram_mb: f64,
    pub is_game: bool,
    pub demoted: bool,
}

/// Live telemetry plus its history, refreshed together.
#[derive(Debug)]
pub struct Telemetry {
    last: Instant,
    pub stats: MachineStats,
    cpu_hist: Series,
    gpu_hist: Series,
    ram_hist: Series,
    rows: Vec<ProcRow>,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl Telemetry {
    pub fn new() -> Self {
        Telemetry {
            last: Instant::now(),
            stats: MachineStats::default(),
            cpu_hist: Series::default(),
            gpu_hist: Series::default(),
            ram_hist: Series::default(),
            rows: Vec::new(),
        }
    }

    /// True when at least 500 ms passed since the last read.
    pub fn needs_refresh(&self, now: Instant, min_ms: u64) -> bool {
        now.duration_since(self.last) >= Duration::from_millis(min_ms)
    }

    /// Read the machine once and fold it into the history. Rate-limited by the
    /// caller, not here.
    pub fn refresh(&mut self) {
        let gpu = perf::gpu_utilization();
        let (total, available) = perf::ram_bytes();

        let used = total.saturating_sub(available);
        let cpu = perf::system_cpu_percent();

        self.stats = MachineStats {
            cpu_percent: cpu,
            gpu_percent: gpu.utilization,
            gpu_engines: gpu.engines,
            ram_used: used,
            ram_available: available,
            ram_total: total,
            cpu_count: crate::win::process::logical_cpu_count(),
            cpu_name: crate::session::cpu_brand(),
        };

        self.cpu_hist.push(cpu);
        self.gpu_hist.push(gpu.utilization);
        self.ram_hist.push(self.stats.ram_percent());
        self.last = Instant::now();
    }

    /// Push a sorted top-process table computed from the latest process sample.
    pub fn set_rows(&mut self, mut rows: Vec<ProcRow>) {
        rows.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows.truncate(8);
        self.rows = rows;
    }

    pub fn stats(&self) -> &MachineStats {
        &self.stats
    }

    pub fn cpu_history(&self) -> Vec<f32> {
        self.cpu_hist.as_vec()
    }

    pub fn gpu_history(&self) -> Vec<f32> {
        self.gpu_hist.as_vec()
    }

    pub fn ram_history(&self) -> Vec<f32> {
        self.ram_hist.as_vec()
    }

    /// The three histories merged into one list for the dashboard sparklines.
    pub fn combined_history(&self) -> Vec<(f32, f32, f32)> {
        let cpu = self.cpu_hist.as_vec();
        let gpu = self.gpu_hist.as_vec();
        let ram = self.ram_hist.as_vec();
        let n = cpu.len().min(gpu.len()).min(ram.len());
        (0..n).map(|i| (cpu[i], gpu[i], ram[i])).collect()
    }

    pub fn rows(&self) -> &[ProcRow] {
        &self.rows
    }

    /// Heaviest CPU consumers first, game pinned to the top.
    pub fn top_cpu_rows(&self) -> Vec<(String, f32, f64, bool, bool)> {
        let mut rows: Vec<&ProcRow> = self.rows.iter().collect();
        rows.sort_by(|a, b| {
            b.is_game
                .cmp(&a.is_game)
                .then(b.cpu_percent.total_cmp(&a.cpu_percent))
        });
        rows.iter()
            .take(6)
            .map(|r| (r.exe.clone(), r.cpu_percent, r.ram_mb, r.is_game, r.demoted))
            .collect()
    }

    /// Heaviest RAM consumers first, game pinned to the top.
    pub fn top_ram_rows(&self) -> Vec<(String, f32, f64, bool, bool)> {
        let mut rows: Vec<&ProcRow> = self.rows.iter().collect();
        rows.sort_by(|a, b| {
            b.is_game
                .cmp(&a.is_game)
                .then(b.ram_mb.total_cmp(&a.ram_mb))
        });
        rows.iter()
            .take(6)
            .map(|r| (r.exe.clone(), r.cpu_percent, r.ram_mb, r.is_game, r.demoted))
            .collect()
    }
}

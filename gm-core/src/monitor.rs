//! Process sampling and game detection.
//!
//! The monitor takes cheap snapshots of the process table and computes per-process
//! CPU deltas between samples. Detection then decides whether a game is running
//! using the strategy selected in the configuration.

#![allow(dead_code)]

use std::collections::HashMap;
use std::time::Instant;

use crate::config::{Config, DetectMode};
use crate::win::{process, system};

/// One process observation, with CPU usage derived from the previous sample.
#[derive(Debug, Clone)]
pub struct ProcessSample {
    pub pid: u32,
    pub ppid: u32,
    pub exe: String,
    pub path: String,
    /// Percent of one CPU core over the last interval (can exceed 100 for multithreaded work).
    pub cpu_percent: f32,
    /// Percent of total machine CPU.
    pub cpu_percent_total: f32,
    pub working_set: u64,
    pub private_bytes: u64,
    /// Priority class as currently set on the live process.
    pub priority_class: u32,
    pub threads: u32,
    /// True when the executable matches a configured (enabled) game entry.
    pub is_configured_game: bool,
    pub is_whitelisted: bool,
    /// True when the process owns the foreground window.
    pub is_foreground: bool,
    pub started_at: Option<Instant>,
}

impl ProcessSample {
    pub fn ram_mb(&self) -> f64 {
        self.working_set as f64 / (1024.0 * 1024.0)
    }
}

/// The result of one detection pass.
#[derive(Debug, Clone, Default)]
pub struct GameDetection {
    /// Pid of the process judged to be the game, when any.
    pub pid: Option<u32>,
    pub exe: String,
    pub path: String,
    pub title: String,
    /// Why the decision was made, for the log and the UI.
    pub reason: String,
    /// All candidates seen this round, highest priority first.
    pub candidates: Vec<u32>,
    /// True when a fullscreen app is in the foreground.
    pub fullscreen: bool,
    /// Total system CPU load, percent.
    pub system_cpu: f32,
}

impl GameDetection {
    pub fn found(&self) -> bool {
        self.pid.is_some()
    }
}

/// Rolling state needed to compute CPU deltas.
#[derive(Debug, Clone, Copy)]
struct CpuMark {
    cpu_100ns: u64,
    at: Instant,
}

/// The process monitor.
pub struct Monitor {
    marks: HashMap<u32, CpuMark>,
    last_snapshot: Instant,
    /// Cached system CPU baseline.
    last_system: Option<(process::SystemTimes, Instant)>,
    /// Executables launched by the user's launcher, remembered so that we can
    /// recognise a child process as the actual game even if it was not listed.
    launch_children: HashMap<u32, String>,
    pub cpu_count: u32,
    pub total_ram: u64,
}

impl Monitor {
    pub fn new() -> Self {
        Monitor {
            marks: HashMap::with_capacity(512),
            last_snapshot: Instant::now(),
            last_system: None,
            launch_children: HashMap::new(),
            cpu_count: process::logical_cpu_count(),
            total_ram: process::total_ram_bytes(),
        }
    }

    /// Sample every process, computing CPU usage since the previous call.
    pub fn sample(&mut self, cfg: &Config) -> Vec<ProcessSample> {
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_snapshot).as_secs_f64();
        let interval = if elapsed < 0.05 { 0.05 } else { elapsed };
        self.last_snapshot = now;

        let list = match process::snapshot_processes() {
            Ok(l) => l,
            Err(e) => {
                log::warn!("process snapshot failed: {e}");
                return Vec::new();
            }
        };

        let fg_pid = system::window_pid(system::foreground_window());
        let mut out = Vec::with_capacity(list.len());
        let mut seen: Vec<u32> = Vec::with_capacity(list.len());

        for info in list {
            if info.pid == 0 || info.pid == 4 {
                continue; // System Idle / System pseudo-processes
            }
            seen.push(info.pid);
            let Some(counters) = process::process_counters(info.pid) else {
                continue;
            };

            let cpu_percent = match self.marks.get(&info.pid) {
                Some(prev) => {
                    let delta = counters.cpu_time_100ns.saturating_sub(prev.cpu_100ns) as f64;
                    let wall = now.saturating_duration_since(prev.at).as_secs_f64().max(0.001);
                    // delta is in 100ns units; percent of one core:
                    (delta / 10_000_000.0 / wall * 100.0) as f32
                }
                None => 0.0,
            };
            self.marks.insert(
                info.pid,
                CpuMark {
                    cpu_100ns: counters.cpu_time_100ns,
                    at: now,
                },
            );

            let exe_lc = info.exe.to_ascii_lowercase();
            let is_configured_game = cfg.is_game_name(&exe_lc);
            let is_whitelisted = cfg.is_whitelisted(&exe_lc);

            out.push(ProcessSample {
                pid: info.pid,
                ppid: info.ppid,
                exe: info.exe.clone(),
                path: String::new(), // filled lazily when needed
                cpu_percent: cpu_percent.max(0.0),
                cpu_percent_total: (cpu_percent / self.cpu_count as f32).max(0.0),
                working_set: counters.working_set,
                private_bytes: counters.private_bytes,
                priority_class: process::get_priority_class(info.pid).unwrap_or(0),
                threads: info.threads,
                is_configured_game,
                is_whitelisted,
                is_foreground: info.pid == fg_pid,
                started_at: None,
            });
        }

        // Drop marks for processes that have exited so the map cannot grow.
        self.marks.retain(|pid, _| seen.contains(pid));
        let _ = interval;
        out.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    /// Compute overall system CPU load, percent, since the previous call.
    pub fn system_cpu(&mut self) -> f32 {
        let now = Instant::now();
        let t = process::system_times();
        let result = match self.last_system {
            Some((prev, at)) => {
                let wall = now.saturating_duration_since(at).as_secs_f64().max(0.001);
                let idle = t.idle.saturating_sub(prev.idle) as f64;
                let kernel = t.kernel.saturating_sub(prev.kernel) as f64;
                let user = t.user.saturating_sub(prev.user) as f64;
                let total = kernel + user; // kernel includes idle
                if total <= 0.0 {
                    0.0
                } else {
                    let busy = (total - idle).max(0.0);
                    let pct = busy / total * 100.0;
                    let _ = wall;
                    pct as f32
                }
            }
            None => 0.0,
        };
        self.last_system = Some((t, now));
        result.clamp(0.0, 100.0)
    }

    /// Decide whether a game is running, honouring the configured strategy.
    pub fn detect(&mut self, samples: &[ProcessSample], cfg: &Config) -> GameDetection {
        let fullscreen = system::is_fullscreen_app_active();
        let system_cpu = self.system_cpu();
        let fg_pid = system::window_pid(system::foreground_window());

        let mut det = GameDetection {
            fullscreen,
            system_cpu,
            ..Default::default()
        };

        // Candidates ranked by: configured game, foreground, fullscreen owner, CPU load.
        let mut best: Option<&ProcessSample> = None;
        let mut best_score: i64 = i64::MIN;

        for s in samples {
            if s.is_whitelisted && !s.is_configured_game {
                continue;
            }
            let is_known = s.is_configured_game;
            let is_fg = s.pid == fg_pid;
            let is_fs_owner = fullscreen && is_fg;
            let heavy = s.cpu_percent >= cfg.cpu_detect_threshold;

            let qualifies = match cfg.detect_mode {
                DetectMode::ListOnly => is_known,
                DetectMode::ForegroundOnly => is_fs_owner,
                DetectMode::Smart => is_known || is_fs_owner || heavy,
            };
            if !qualifies {
                continue;
            }
            det.candidates.push(s.pid);

            let mut score: i64 = 0;
            if is_known {
                score += 1_000;
            }
            if is_fs_owner {
                score += 500;
            }
            if is_fg {
                score += 250;
            }
            score += s.cpu_percent.min(400.0) as i64;
            score += (s.working_set / (64 * 1024 * 1024)) as i64; // 64 MB blocks

            if score > best_score {
                best_score = score;
                best = Some(s);
            }
        }

        if let Some(b) = best {
            det.pid = Some(b.pid);
            det.exe = b.exe.clone();
            det.path = b.path.clone();
            det.title = system::window_title(system::foreground_window());
            det.reason = if b.is_configured_game {
                format!("{} is on the game list", b.exe)
            } else if fullscreen && b.is_foreground {
                format!("{} owns the fullscreen foreground window", b.exe)
            } else {
                format!(
                    "{} is using {:.0}% of a core (threshold {:.0}%)",
                    b.exe, b.cpu_percent.max(b.cpu_percent_total), cfg.cpu_detect_threshold
                )
            };
        } else {
            det.reason = match cfg.detect_mode {
                DetectMode::ListOnly => "no listed game is running".into(),
                DetectMode::ForegroundOnly => "no fullscreen game in the foreground".into(),
                DetectMode::Smart => "no game detected".into(),
            };
        }
        det
    }

    /// Remember which child processes belong to a launcher so children of a
    /// game (e.g. `game.exe` spawning `game-win64-shipping.exe`) stay boosted.
    pub fn note_launcher_children(&mut self, parent: u32, exe: &str) {
        self.launch_children.insert(parent, exe.to_string());
    }

    /// True when `pid` is a known child of the recorded launcher tree.
    pub fn is_launcher_child(&self, pid: u32, samples: &[ProcessSample]) -> bool {
        let mut cur = pid;
        for _ in 0..8 {
            if self.launch_children.contains_key(&cur) {
                return true;
            }
            match samples.iter().find(|s| s.pid == cur) {
                Some(s) if s.ppid != 0 => cur = s.ppid,
                _ => return false,
            }
        }
        false
    }
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Quick helper used by the GUI/tray to answer "is a game running right now?".
pub fn quick_probe(cfg: &Config) -> GameDetection {
    let mut m = Monitor::new();
    // Two samples are needed before CPU deltas are meaningful.
    let _ = m.sample(cfg);
    process::sleep_ms(120);
    let s = m.sample(cfg);
    m.detect(&s, cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_reports_many_processes() {
        let cfg = Config::default();
        let mut m = Monitor::new();
        let first = m.sample(&cfg);
        assert!(first.len() > 5, "expected several processes, got {}", first.len());
        // Every sample must carry a plausible executable name.
        assert!(first.iter().all(|s| !s.exe.is_empty()));
    }

    #[test]
    fn cpu_delta_becomes_visible_after_a_busy_interval() {
        let cfg = Config::default();
        let mut m = Monitor::new();
        let _ = m.sample(&cfg);
        // Burn CPU so the next sample can observe a non-zero delta.
        let start = Instant::now();
        let mut acc: u64 = 0;
        while start.elapsed().as_millis() < 250 {
            acc = acc.wrapping_mul(1664525).wrapping_add(1013904223);
        }
        std::hint::black_box(acc);
        let s2 = m.sample(&cfg);
        let me = s2.iter().find(|s| s.pid == process::current_pid());
        assert!(me.is_some(), "own process must be sampled");
        assert!(
            me.unwrap().cpu_percent > 0.0,
            "a busy process must show CPU usage"
        );
    }

    #[test]
    fn system_cpu_is_in_range() {
        let cfg = Config::default();
        let mut m = Monitor::new();
        let _ = m.sample(&cfg);
        process::sleep_ms(200);
        let _ = m.sample(&cfg);
        let cpu = m.system_cpu();
        assert!((0.0..=100.0).contains(&cpu), "cpu out of range: {cpu}");
    }

    #[test]
    fn detection_finds_a_configured_game_when_present() {
        // Pretend our own executable is a game and verify the detector picks it up.
        let mut cfg = Config::default();
        let me = crate::prelude::exe_name().to_ascii_lowercase();
        cfg.games.push(crate::config::GameEntry::new(&me));
        let mut m = Monitor::new();
        let _ = m.sample(&cfg);
        process::sleep_ms(120);
        let samples = m.sample(&cfg);
        let det = m.detect(&samples, &cfg);
        assert!(det.found(), "own exe registered as a game must be detected");
        assert!(det.reason.contains("game list"), "reason: {}", det.reason);
    }

    #[test]
    fn system_processes_are_never_selected() {
        let cfg = Config::default();
        let mut m = Monitor::new();
        let samples = m.sample(&cfg);
        let det = m.detect(&samples, &cfg);
        if let Some(pid) = det.pid {
            assert_ne!(pid, 0);
            assert_ne!(pid, 4);
        }
    }

    #[test]
    fn list_only_mode_ignores_heavy_unknown_processes() {
        let mut cfg = Config::default();
        cfg.detect_mode = DetectMode::ListOnly;
        cfg.games.clear();
        let mut m = Monitor::new();
        let _ = m.sample(&cfg);
        process::sleep_ms(150);
        let samples = m.sample(&cfg);
        let det = m.detect(&samples, &cfg);
        assert!(!det.found(), "no listed game should mean no detection");
    }

    #[test]
    fn cpu_count_and_ram_are_plausible() {
        let m = Monitor::new();
        assert!(m.cpu_count >= 1);
        assert!(m.total_ram > 1024 * 1024 * 1024);
    }
}

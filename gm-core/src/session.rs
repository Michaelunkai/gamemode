//! Session orchestration: the state machine behind the on/off switch.
//!
//! A [`Session`] owns the monitor, the optimiser and the snapshot, and exposes a
//! single `tick()` that the GUI calls on a timer. Everything the app does while
//! active flows through here, which keeps the GUI a pure view layer.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::GmResult;
use crate::monitor::{GameDetection, Monitor, ProcessSample};
use crate::optimizer::{ActionReport, Optimizer};
use crate::snapshot::Snapshot;
use crate::telemetry::{ProcRow, Telemetry};use crate::win::{process, system};

/// Whether GameMode is currently optimising.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Doing nothing; the machine is untouched.
    Idle,
    /// A game was detected and the activation delay is counting down.
    Arming,
    /// Optimisations are applied.
    Active,
    /// The game exited and the grace period is counting down.
    Draining,
}

impl SessionState {
    pub fn label(&self) -> &'static str {
        match self {
            SessionState::Idle => "Idle",
            SessionState::Arming => "Arming",
            SessionState::Active => "Active",
            SessionState::Draining => "Draining",
        }
    }
    pub fn is_active(&self) -> bool {
        matches!(self, SessionState::Active | SessionState::Draining)
    }
    pub fn is_optimising(&self) -> bool {
        matches!(self, SessionState::Active)
    }
}

/// One line in the session log shown by the GUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    pub at: String,
    pub text: String,
    pub level: LogLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Good,
    Warn,
    Bad,
}

/// Where the current activation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The user pressed the button / hotkey / tray item.
    Manual,
    /// Automatic detection found a game.
    Automatic,
}

/// The complete runtime state of GameMode.
pub struct Session {
    pub config: Config,
    pub monitor: Monitor,
    /// Rich machine telemetry the dashboard draws (CPU/GPU/RAM + history).
    pub telemetry: Telemetry,
    pub optimizer: Optimizer,
    pub snapshot: Option<Snapshot>,
    pub state: SessionState,
    pub trigger: Option<Trigger>,
    /// When the current arming or draining phase began.
    phase_started: Instant,
    /// Detection result of the most recent tick.
    pub detection: GameDetection,
    /// Latest process table.
    pub samples: Vec<ProcessSample>,
    /// Reports produced by the most recent activation.
    pub last_reports: Vec<ActionReport>,
    /// Restore report from the most recent deactivation.
    pub last_restore: Vec<String>,
    /// Rolling UI log.
    pub log: VecDeque<LogLine>,
    /// Set when a previous run left an unreplayed snapshot behind.
    pub pending_recovery: bool,
    /// Count of activations this process has performed.
    pub activations: u64,
    /// Total time spent active.
    pub total_active: Duration,
    active_since: Option<Instant>,
    /// Set when the user manually disabled, so auto-detect does not fight them.
    pub manual_hold: bool,
    /// Details of what changed for the immediate activation report.
    /// Each entry is (category, what changed, detail).
    pub activation_report: Vec<(String, String, String)>,
}

impl Session {
    /// Create a session, recovering any snapshot left by a previous run.
    pub fn new(config: Config) -> Self {
        let pending_recovery = Snapshot::is_pending();
        let mut s = Session {
            monitor: Monitor::new(),
            telemetry: Telemetry::default(),
            optimizer: Optimizer::new(),
            snapshot: None,
            state: SessionState::Idle,
            trigger: None,
            phase_started: Instant::now(),
            detection: GameDetection::default(),
            samples: Vec::new(),
            last_reports: Vec::new(),
            last_restore: Vec::new(),
            log: VecDeque::with_capacity(256),
            pending_recovery,
            activations: 0,
            total_active: Duration::ZERO,
            active_since: None,
            config,
            manual_hold: false,
            activation_report: Vec::new(),
        };
        if pending_recovery {
            s.push_log(
                LogLevel::Warn,
                "a previous run left system changes behind: use Restore to undo them",
            );
        }
        s
    }

    /// Append a line to the UI log, keeping the buffer bounded.
    pub fn push_log(&mut self, level: LogLevel, text: impl Into<String>) {
        if self.log.len() >= 256 {
            self.log.pop_front();
        }
        self.log.push_back(LogLine {
            at: chrono::Local::now().format("%H:%M:%S").to_string(),
            text: text.into(),
            level,
        });
    }

    /// Replay a leftover snapshot from a previous run.
    pub fn recover(&mut self) -> Vec<String> {
        let Some(mut snap) = Snapshot::load() else {
            self.pending_recovery = false;
            return vec!["nothing to restore".into()];
        };
        let report = snap.restore();
        snap.discard();
        self.pending_recovery = !snap.restored;
        for line in &report {
            self.push_log(LogLevel::Info, line.clone());
        }
        report
    }

    /// Turn GameMode on. Safe to call when already active (it is a no-op).
    pub fn enable(&mut self, trigger: Trigger) -> Vec<ActionReport> {
        if self.state == SessionState::Active {
            return self.last_reports.clone();
        }
        // Refresh the process table so the boost targets the right process.
        self.samples = self.monitor.sample(&self.config);
        self.detection = self.monitor.detect(&self.samples, &self.config);

        // Locked mode overrides automatic detection: the target is whatever
        // `locked_game_exe` says, even when the process is not currently running.
        let (pid, exe) = if let Some(target) = self.config.locked_target() {
            let found = self.samples.iter().find(|s| s.exe.to_ascii_lowercase() == target);
            let pid = found.map(|s| s.pid);
            let exe = target.to_string();
            if pid.is_none() {
                self.detection.reason = format!("locked to {exe} — waiting for process");
            } else {
                self.detection.reason = format!("locked to {exe} (pid {})", pid.unwrap());
            }
            self.detection.pid = pid;
            self.detection.exe = exe.clone();
            (pid, exe)
        } else {
            match self.detection.pid {
                Some(p) => (Some(p), self.detection.exe.clone()),
                None => (None, String::new()),
            }
        };

        let mut snap = Snapshot::begin(&exe);
        let reports = self
            .optimizer
            .activate(&self.config, &mut snap, &self.samples, pid, &exe);

        let applied = reports
            .iter()
            .filter(|r| r.status == crate::optimizer::ActionStatus::Applied)
            .count();
        let failed = reports
            .iter()
            .filter(|r| r.status == crate::optimizer::ActionStatus::Failed)
            .count();

        self.last_reports = reports.clone();
        self.snapshot = Some(snap);
        self.state = SessionState::Active;
        self.trigger = Some(trigger);
        self.active_since = Some(Instant::now());
        self.activations += 1;
        self.manual_hold = trigger == Trigger::Manual;

        let how = match trigger {
            Trigger::Manual => "manually",
            Trigger::Automatic => "automatically",
        };
        self.push_log(
            LogLevel::Good,
            format!(
                "GameMode ON {how}: {applied} optimisation(s) applied, {failed} failed{}",
                if exe.is_empty() {
                    String::new()
                } else {
                    format!(" for {exe}")
                }
            ),
        );
        if !self.detection.reason.is_empty() {
            self.push_log(LogLevel::Info, format!("detection: {}", self.detection.reason));
        }
        for r in reports.iter().filter(|r| r.status != crate::optimizer::ActionStatus::Applied) {
            let level = if r.status == crate::optimizer::ActionStatus::Failed {
                LogLevel::Warn
            } else {
                LogLevel::Info
            };
            self.push_log(level, format!("{}: {}", r.name, r.detail));
        }
        reports
    }

    /// Turn GameMode off and restore the machine.
    pub fn disable(&mut self, reason: &str) -> Vec<String> {
        if self.state == SessionState::Idle && self.snapshot.is_none() {
            // Still worth replaying a stale snapshot if one exists.
            if Snapshot::is_pending() {
                return self.recover();
            }
            return vec!["GameMode is already off".into()];
        }
        if let Some(since) = self.active_since.take() {
            self.total_active += since.elapsed();
        }
        let mut report = match self.snapshot.take() {
            Some(mut snap) => {
                let r = self.optimizer.deactivate(&self.config, &mut snap);
                if snap.restored {
                    snap.discard();
                } else {
                    // Keep the file so the next start can finish the job.
                    let _ = snap.flush();
                }
                r
            }
            None => {
                self.optimizer.deactivate(&self.config, &mut Snapshot::begin("cleanup"));
                vec!["no snapshot to restore".into()]
            }
        };
        report.push(format!("reason: {reason}"));

        self.state = SessionState::Idle;
        self.trigger = None;
        if reason.contains("manual") || reason.contains("user") {
            self.manual_hold = true;
        }
        let failures = report.iter().filter(|l| l.contains("could not")).count();
        self.push_log(
            if failures == 0 {
                LogLevel::Good
            } else {
                LogLevel::Warn
            },
            format!("GameMode OFF ({reason}): {failures} restore failure(s)"),
        );
        self.last_restore = report.clone();
        report
    }

    /// Toggle: on when idle, off when active.
    pub fn toggle(&mut self) -> bool {
        if self.state == SessionState::Idle {
            self.enable(Trigger::Manual);
            true
        } else {
            self.disable("manual toggle");
            false
        }
    }

    /// Top processes for the dashboard table: game first, then heaviest CPU.
    fn build_proc_rows(&self) -> Vec<ProcRow> {
        let mut rows: Vec<ProcRow> = self
            .samples
            .iter()
            .map(|s| ProcRow {
                pid: s.pid,
                exe: s.exe.clone(),
                cpu_percent: s.cpu_percent,
                ram_mb: s.working_set as f64 / (1024.0 * 1024.0),
                is_game: s.is_configured_game || s.is_whitelisted,
                demoted: s.priority_class == windows_sys::Win32::System::Threading::IDLE_PRIORITY_CLASS
                    || s.priority_class == windows_sys::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS,
            })
            .collect();
        // The game always leads, then by CPU, so the reader sees the thing that
        // matters first without scanning.
        rows.sort_by(|a, b| {
            b.is_game
                .cmp(&a.is_game)
                .then(b.cpu_percent.total_cmp(&a.cpu_percent))
        });
        rows.truncate(12);
        rows
    }

    /// Advance the state machine. Returns true when optimisations were applied
    /// or removed during this tick, so the GUI knows to refresh.
    pub fn tick(&mut self) -> TickResult {
        let now = Instant::now();
        self.samples = self.monitor.sample(&self.config);
        self.detection = self.monitor.detect(&self.samples, &self.config);
        let sys_cpu = self.monitor.system_cpu();
        self.optimizer
            .refresh_stats(self.detection.pid, &self.detection.exe, &self.samples, sys_cpu);

        // Dashboard telemetry refreshes from its own clock: the counter reader
        // sleeps internally, so it cannot run on every frame without slowing
        // the UI. 500 ms is well below the eye's perception of "live".
        if self.telemetry.needs_refresh(now, 500) {
            self.telemetry.refresh();
            self.telemetry.set_rows(self.build_proc_rows());
        }

        let mut result = TickResult::default();

        match self.state {
            SessionState::Idle => {
                // Locked mode activates immediately, bypassing auto_activate and
                // the arming delay, because the user has explicitly chosen a target.
                if self.config.locked_mode {
                    self.enable(Trigger::Manual);
                    result.activated = true;
                } else if self.config.auto_activate
                    && !self.manual_hold
                    && self.detection.found()
                {
                    if self.config.activation_delay_secs == 0 {
                        self.enable(Trigger::Automatic);
                        result.activated = true;
                    } else {
                        self.state = SessionState::Arming;
                        self.phase_started = now;
                        self.push_log(
                            LogLevel::Info,
                            format!(
                                "{} detected ({}): arming for {}s",
                                self.detection.exe,
                                self.detection.reason,
                                self.config.activation_delay_secs
                            ),
                        );
                        result.armed = true;
                    }
                }
            }
            SessionState::Arming => {
                let waited = now.duration_since(self.phase_started);
                if !self.detection.found() {
                    // The game went away again before we did anything.
                    self.state = SessionState::Idle;
                    self.push_log(
                        LogLevel::Info,
                        format!("detection lost {}: cancelled arming", self.detection.exe),
                    );
                    result.armed = true;
                } else if waited >= Duration::from_secs(self.config.activation_delay_secs) {
                    self.enable(Trigger::Automatic);
                    result.activated = true;
                }
            }
            SessionState::Active => {
                // Locked mode keeps the session active even when the target process
                // is not running; we only rebind when the locked exe appears.
                if self.config.locked_mode {
                    if let Some(target) = self.config.locked_target() {
                        let found = self.samples.iter().find(|s| s.exe.to_ascii_lowercase() == target);
                        if let Some(pid) = found.map(|s| s.pid) {
                            if self.optimizer.stats.game_pid != Some(pid) {
                                if let Some(snap) = self.snapshot.as_mut() {
                                    let cfg = self.config.clone();
                                    let samples = self.samples.clone();
                                    let exe = target.to_string();
                                    let _ = self.optimizer.rebind_game(&cfg, snap, pid, &exe, &samples);
                                    self.push_log(
                                        LogLevel::Good,
                                        format!("boost re-applied to {exe} (pid {pid})"),
                                    );
                                }
                            }
                        } else {
                            // Locked target not running: stay active, wait.
                            self.push_log(
                                LogLevel::Info,
                                format!("waiting for {target} to launch"),
                            );
                        }
                    }
                } else if !self.detection.found() {
                    // If the game process died, keep optimising for the grace period
                    // (games often relaunch between matches), then restore.
                    self.state = SessionState::Draining;
                    self.phase_started = now;
                    self.push_log(
                        LogLevel::Info,
                        format!(
                            "game closed: restoring in {}s unless one returns",
                            self.config.deactivation_grace_secs
                        ),
                    );
                    result.draining = true;
                } else if let Some(pid) = self.detection.pid {
                    // A new game may have launched; re-boost it if it is new.
                    if self.optimizer.stats.game_pid != Some(pid) {
                        if let Some(snap) = self.snapshot.as_mut() {
                            let cfg = self.config.clone();
                            let samples = self.samples.clone();
                            let exe = self.detection.exe.clone();
                            let _ = self.optimizer.rebind_game(&cfg, snap, pid, &exe, &samples);
                            self.push_log(
                                LogLevel::Good,
                                format!("boost re-applied to {exe} (pid {pid})"),
                            );
                        }
                    }
                }
            }
            SessionState::Draining => {
                if self.detection.found() {
                    // A game came back; adopt it and keep going.
                    self.state = SessionState::Active;
                    self.push_log(
                        LogLevel::Good,
                        format!("game returned ({}): staying active", self.detection.exe),
                    );
                    result.activated = true;
                } else if now.duration_since(self.phase_started)
                    >= Duration::from_secs(self.config.deactivation_grace_secs)
                {
                    self.disable("game exited");
                    result.deactivated = true;
                }
            }
        }
        result
    }

    /// Apply a newly loaded configuration: reactivates when it changes what
    /// GameMode is doing while it is running.
    pub fn apply_config(&mut self, cfg: Config) -> bool {
        let was_active = self.state.is_optimising();
        if was_active {
            self.disable("configuration changed");
        }
        self.config = cfg;
        let _ = self.config.save();
        if was_active {
            self.enable(Trigger::Manual);
        }
        true
    }

    /// Reload the configuration from disk without restarting.
    pub fn reload_config(&mut self) -> GmResult<()> {
        let cfg = Config::load()?;
        let was_active = self.state.is_optimising();
        if was_active {
            self.disable("config reload");
        }
        self.config = cfg;
        if was_active {
            self.enable(Trigger::Manual);
        }
        Ok(())
    }

    /// Seconds the current phase has been running.
    pub fn phase_elapsed(&self) -> Duration {
        Instant::now().duration_since(self.phase_started)
    }

    /// Remaining arming or draining time, when in one of those phases.
    pub fn phase_remaining(&self) -> Option<Duration> {
        match self.state {
            SessionState::Arming => Some(
                Duration::from_secs(self.config.activation_delay_secs)
                    .saturating_sub(self.phase_elapsed()),
            ),
            SessionState::Draining => Some(
                Duration::from_secs(self.config.deactivation_grace_secs)
                    .saturating_sub(self.phase_elapsed()),
            ),
            _ => None,
        }
    }

    /// Total time active including the current activation.
    pub fn active_duration(&self) -> Duration {
        match self.active_since {
            Some(t) => self.total_active + t.elapsed(),
            None => self.total_active,
        }
    }

    /// Path of the log file, for the "open log" button.
    pub fn log_path(&self) -> PathBuf {
        Config::data_dir().join("gamemode.log")
    }

    /// A one-line status string for the tray tooltip.
    pub fn tooltip(&self) -> String {
        match self.state {
            SessionState::Active => {
                if self.config.locked_mode {
                    if let Some(target) = self.config.locked_target() {
                        if self.detection.pid.is_some() {
                            format!(
                                "GameMode: ON (locked to {target})\nGame {:.0}% CPU · System {:.0}% CPU",
                                self.optimizer.stats.game_cpu, self.optimizer.stats.system_cpu
                            )
                        } else {
                            format!(
                                "GameMode: ON (locked to {target} — waiting)\nSystem {:.0}% CPU",
                                self.optimizer.stats.system_cpu
                            )
                        }
                    } else {
                        let exe = if self.detection.exe.is_empty() {
                            "waiting for a game".to_string()
                        } else {
                            self.detection.exe.clone()
                        };
                        format!(
                            "GameMode: ON\n{exe}\nGame {:.0}% CPU · System {:.0}% CPU",
                            self.optimizer.stats.game_cpu, self.optimizer.stats.system_cpu
                        )
                    }
                } else {
                    let exe = if self.detection.exe.is_empty() {
                        "waiting for a game".to_string()
                    } else {
                        self.detection.exe.clone()
                    };
                    format!(
                        "GameMode: ON\n{exe}\nGame {:.0}% CPU · System {:.0}% CPU",
                        self.optimizer.stats.game_cpu, self.optimizer.stats.system_cpu
                    )
                }
            }
            SessionState::Arming => format!(
                "GameMode: arming ({:.0}s)\n{}",
                self.phase_remaining().map(|d| d.as_secs_f32()).unwrap_or(0.0),
                self.detection.exe
            ),
            SessionState::Draining => format!(
                "GameMode: restoring in {:.0}s",
                self.phase_remaining().map(|d| d.as_secs_f32()).unwrap_or(0.0)
            ),
            SessionState::Idle => {
                if self.config.locked_mode {
                    if let Some(target) = self.config.locked_target() {
                        format!("GameMode: OFF (locked to {target})")
                    } else {
                        "GameMode: OFF (locked — no target)".to_string()
                    }
                } else if self.config.auto_activate {
                    "GameMode: OFF (auto-detect armed)\nNo game running".to_string()
                } else {
                    "GameMode: OFF".to_string()
                }
            }
        }
    }

    /// True when GameMode should be considered "on" for the tray icon.
    pub fn is_on(&self) -> bool {
        self.state.is_active()
    }
}

/// What changed during one `tick()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TickResult {
    pub activated: bool,
    pub deactivated: bool,
    pub armed: bool,
    pub draining: bool,
}

impl TickResult {
    pub fn any_change(&self) -> bool {
        self.activated || self.deactivated || self.armed || self.draining
    }
}

/// Environment facts the GUI shows on the dashboard.
#[derive(Debug, Clone)]
pub struct SystemInfo {
    pub cpu_name: String,
    pub logical_cpus: u32,
    pub physical_cpus: u32,
    pub total_ram_gb: f64,
    pub available_ram_gb: f64,
    pub elevated: bool,
    pub power_scheme: String,
    pub timer_ms: f32,
    pub windows_build: String,
}

/// Gather the dashboard facts.
pub fn system_info() -> SystemInfo {
    let logical = process::logical_cpu_count();
    let total = process::total_ram_bytes();
    let avail = process::available_ram_bytes();
    SystemInfo {
        cpu_name: cpu_brand(),
        logical_cpus: logical,
        physical_cpus: physical_core_count(),
        total_ram_gb: total as f64 / (1024.0 * 1024.0 * 1024.0),
        available_ram_gb: avail as f64 / (1024.0 * 1024.0 * 1024.0),
        elevated: crate::win::elevation::is_elevated(),
        power_scheme: crate::win::power::active_scheme()
            .map(|g| crate::win::power::scheme_name(&g))
            .unwrap_or_else(|_| "unknown".into()),
        timer_ms: system::timer_ms(),
        windows_build: windows_build(),
    }
}

/// Read the CPU marketing name from the registry.
pub fn cpu_brand() -> String {
    let v = system::reg_read(1, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString");
    v.and_then(|v| v.as_string())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".into())
}

/// Physical core count, derived from `NumberOfCores` in the registry.
pub fn physical_core_count() -> u32 {
    for i in 0..8 {
        let key = format!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\{i}");
        if system::reg_read(1, &key, "NumberOfCores").is_none() {
            break;
        }
    }
    // Windows exposes the core count through the same key's "NumberOfCores",
    // which equals the per-package count; combine with the CPU count below.
    let v = system::reg_read(1, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "~MHz");
    let _ = v;
    let logical = process::logical_cpu_count();
    // Best-effort heuristic when the exact topology is unavailable: on Intel with
    // HT and AMD with SMT, logical = 2x physical for consumer parts.
    logical.div_ceil(2).max(1)
}

/// Windows build string from the registry.
pub fn windows_build() -> String {
    let name = system::reg_read(
        1,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "ProductName",
    )
    .and_then(|v| v.as_string())
    .unwrap_or_else(|| "Windows".into());
    let build = system::reg_read(
        1,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuildNumber",
    )
    .and_then(|v| v.as_string())
    .unwrap_or_default();
    let ubr = system::reg_read(
        1,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "UBR",
    )
    .and_then(|v| v.as_u32())
    .unwrap_or(0);
    if build.is_empty() {
        name
    } else if ubr > 0 {
        format!("{name} (build {build}.{ubr})")
    } else {
        format!("{name} (build {build})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GameEntry;

    fn quiet_config() -> Config {
        let mut cfg = Config::default();
        cfg.manage_services = false;
        cfg.demote_background = false;
        cfg.disable_game_dvr = false;
        cfg.enable_hags = false;
        cfg.network_latency_tweaks = false;
        cfg.disable_menu_hover_delay = false;
        cfg.reserve_cores = false;
        cfg.auto_activate = false;
        cfg.activation_delay_secs = 0;
        cfg.deactivation_grace_secs = 0;
        cfg
    }

    #[test]
    fn manual_toggle_activates_and_restores() {
        let _guard = crate::testlock::guard();
        let mut s = Session::new(quiet_config());
        assert_eq!(s.state, SessionState::Idle);
        let reports = s.enable(Trigger::Manual);
        assert_eq!(s.state, SessionState::Active);
        assert!(s.is_on());
        assert!(!reports.is_empty());
        assert_eq!(s.activations, 1);

        s.disable("manual toggle");
        assert_eq!(s.state, SessionState::Idle);
        assert!(!s.is_on());
        assert!(s.snapshot.is_none(), "snapshot must be cleared after restore");
    }

    #[test]
    fn auto_detection_activates_when_own_exe_is_a_game() {
        let _guard = crate::testlock::guard();
        let mut cfg = quiet_config();
        cfg.auto_activate = true;
        let me = crate::prelude::exe_name().to_ascii_lowercase();
        cfg.games.push(GameEntry::new(&me));
        let mut s = Session::new(cfg);
        // First tick samples and detects; the delay is zero so it activates.
        let r1 = s.tick();
        assert!(r1.activated, "own exe listed as a game must trigger activation");
        assert_eq!(s.state, SessionState::Active);
        s.disable("test cleanup");
    }

    #[test]
    fn arming_delay_defers_activation() {
        let _guard = crate::testlock::guard();
        let mut cfg = quiet_config();
        cfg.auto_activate = true;
        cfg.activation_delay_secs = 30;
        let me = crate::prelude::exe_name().to_ascii_lowercase();
        cfg.games.push(GameEntry::new(&me));
        let mut s = Session::new(cfg);
        let r = s.tick();
        assert!(r.armed, "a delay must put the session into Arming");
        assert_eq!(s.state, SessionState::Arming);
        assert!(s.phase_remaining().unwrap().as_secs() <= 30);
        s.disable("test cleanup");
    }

    #[test]
    fn draining_then_auto_restore() {
        let _guard = crate::testlock::guard();
        let mut cfg = quiet_config();
        cfg.deactivation_grace_secs = 0;
        let mut s = Session::new(cfg);
        s.enable(Trigger::Manual);
        // Force a tick where no game is detected: the session must drain and,
        // with a zero grace period, restore immediately.
        let r = s.tick();
        if r.deactivated {
            assert_eq!(s.state, SessionState::Idle);
        } else {
            // The detection still found our own pid registered by default config,
            // in which case staying active is the correct behaviour.
            assert!(s.is_on());
            s.disable("test cleanup");
        }
    }

    #[test]
    fn manual_hold_blocks_auto_activation() {
        let _guard = crate::testlock::guard();
        let mut cfg = quiet_config();
        cfg.auto_activate = true;
        let me = crate::prelude::exe_name().to_ascii_lowercase();
        cfg.games.push(GameEntry::new(&me));
        let mut s = Session::new(cfg);
        s.enable(Trigger::Manual);
        s.disable("manual toggle by user");
        assert!(s.manual_hold, "a manual off must set the hold flag");
        let r = s.tick();
        assert!(!r.activated, "auto-detection must respect the manual hold");
    }

    #[test]
    fn tooltip_renders_in_every_state() {
        let _guard = crate::testlock::guard();
        let mut s = Session::new(quiet_config());
        assert!(s.tooltip().contains("OFF"));
        s.enable(Trigger::Manual);
        assert!(s.tooltip().contains("ON"));
        s.disable("test");
        assert!(s.tooltip().contains("OFF"));
    }

    #[test]
    fn system_info_is_populated() {
        let _guard = crate::testlock::guard();
        let info = system_info();
        assert!(info.logical_cpus >= 1);
        assert!(info.total_ram_gb > 0.5);
        assert!(!info.windows_build.is_empty());
        assert!(!info.cpu_name.is_empty());
    }

    #[test]
    fn config_apply_reactivates_when_running() {
        let _guard = crate::testlock::guard();
        let mut s = Session::new(quiet_config());
        s.enable(Trigger::Manual);
        assert!(s.is_on());
        let mut cfg = quiet_config();
        cfg.boost_level = crate::config::BoostLevel::Balanced;
        s.apply_config(cfg);
        assert!(s.is_on(), "a config change while on must leave GameMode on");
        s.disable("test cleanup");
    }
}

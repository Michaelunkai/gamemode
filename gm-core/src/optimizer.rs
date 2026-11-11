//! The optimisation engine: everything GameMode does while it is active.
//!
//! Actions are ordered so the game is raised before background work is demoted,
//! and every action that modifies the system journals its previous state into the
//! [`Snapshot`] so it can be undone exactly.

#![allow(dead_code)]

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::config::{BoostLevel, Config, PowerPlan};
use crate::error::GmResult;
use crate::monitor::ProcessSample;
use crate::snapshot::{Change, Snapshot};
use crate::win::{power, process, services, system};

use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, REALTIME_PRIORITY_CLASS, THREAD_PRIORITY_HIGHEST,
    THREAD_PRIORITY_TIME_CRITICAL,
};

/// Outcome of one optimisation action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Applied,
    Skipped,
    Unsupported,
    Failed,
}

impl ActionStatus {
    pub fn symbol(&self) -> &'static str {
        match self {
            ActionStatus::Applied => "✔",
            ActionStatus::Skipped => "–",
            ActionStatus::Unsupported => "?",
            ActionStatus::Failed => "✖",
        }
    }
}

/// What one action did, ready to show in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionReport {
    pub name: String,
    pub status: ActionStatus,
    pub detail: String,
    /// True when the action changed the machine and will be restored.
    pub reversible: bool,
}

impl ActionReport {
    fn applied(name: &str, detail: impl Into<String>, reversible: bool) -> Self {
        ActionReport {
            name: name.into(),
            status: ActionStatus::Applied,
            detail: detail.into(),
            reversible,
        }
    }
    fn skipped(name: &str, detail: impl Into<String>) -> Self {
        ActionReport {
            name: name.into(),
            status: ActionStatus::Skipped,
            detail: detail.into(),
            reversible: false,
        }
    }
    fn unsupported(name: &str, detail: impl Into<String>) -> Self {
        ActionReport {
            name: name.into(),
            status: ActionStatus::Unsupported,
            detail: detail.into(),
            reversible: false,
        }
    }
    fn failed(name: &str, detail: impl Into<String>) -> Self {
        ActionReport {
            name: name.into(),
            status: ActionStatus::Failed,
            detail: detail.into(),
            reversible: false,
        }
    }
}

/// Live statistics shown in the window while GameMode is active.
#[derive(Debug, Clone, Default)]
pub struct OptimizerStats {
    pub game_pid: Option<u32>,
    pub game_exe: String,
    pub game_cpu: f32,
    pub game_ram_mb: f64,
    pub game_priority: u32,
    pub system_cpu: f32,
    pub processes_demoted: u32,
    pub processes_trimmed: u32,
    pub bytes_freed: u64,
    pub cores_for_game: usize,
    pub cores_for_background: usize,
}

impl OptimizerStats {
    pub fn game_priority_label(&self) -> &'static str {
        priority_class_label(self.game_priority)
    }
}

/// Render a priority class constant as text.
pub fn priority_class_label(class: u32) -> &'static str {
    match class {
        REALTIME_PRIORITY_CLASS => "Realtime",
        HIGH_PRIORITY_CLASS => "High",
        ABOVE_NORMAL_PRIORITY_CLASS => "Above normal",
        NORMAL_PRIORITY_CLASS => "Normal",
        BELOW_NORMAL_PRIORITY_CLASS => "Below normal",
        IDLE_PRIORITY_CLASS => "Idle",
        0 => "unknown",
        _ => "other",
    }
}

/// The engine. One instance lives for the lifetime of the process.
pub struct Optimizer {
    /// Pids GameMode itself changed, so repeated ticks do not re-journal them.
    touched_priority: HashSet<u32>,
    touched_affinity: HashSet<u32>,
    touched_throttle: HashSet<u32>,
    stopped_services: HashSet<String>,
    /// Which process was the game when activation happened.
    primed: bool,
    pub stats: OptimizerStats,
    /// Reserved core mask in use, if any.
    reserved_mask: usize,
    background_mask: usize,
    /// True while sleep inhibition is active.
    sleep_blocked: bool,
    /// True while the timer resolution request is active.
    timer_active: bool,
}

impl Optimizer {
    pub fn new() -> Self {
        Optimizer {
            touched_priority: HashSet::new(),
            touched_affinity: HashSet::new(),
            touched_throttle: HashSet::new(),
            stopped_services: HashSet::new(),
            primed: false,
            stats: OptimizerStats::default(),
            reserved_mask: 0,
            background_mask: 0,
            sleep_blocked: false,
            timer_active: false,
        }
    }

    pub fn is_primed(&self) -> bool {
        self.primed
    }

    /// The core mask reserved for the game, or 0 when core reservation is off.
    pub fn reserved_mask(&self) -> usize {
        self.reserved_mask
    }

    pub fn background_mask(&self) -> usize {
        self.background_mask
    }

    // ---------------------------------------------------------------- activate

    /// Apply every configured optimisation. Returns a per-action report.
    pub fn activate(
        &mut self,
        cfg: &Config,
        snap: &mut Snapshot,
        samples: &[ProcessSample],
        game_pid: Option<u32>,
        game_exe: &str,
    ) -> Vec<ActionReport> {
        let mut reports = Vec::new();
        let elevated = crate::win::elevation::is_elevated();
        if elevated {
            let gained = crate::win::elevation::enable_all_privileges();
            if !gained.is_empty() {
                reports.push(ActionReport::applied(
                    "Privileges",
                    format!("{} debug privilege(s) enabled", gained.len()),
                    false,
                ));
            }
        } else {
            reports.push(ActionReport::skipped(
                "Privileges",
                "not elevated: some processes cannot be retuned",
            ));
        }

        // --- 1. CPU core reservation ---------------------------------------
        if cfg.reserve_cores && game_pid.is_some() {
            let total = process::logical_cpu_count();
            let want = if cfg.reserved_core_count == 0 {
                // Automatic: reserve 4 cores (or half the machine on small CPUs).
                (total / 4).clamp(2, 4).min(total.saturating_sub(1).max(1))
            } else {
                cfg.reserved_core_count as u32
            };
            if want > 0 && total > want {
                self.reserved_mask = process::reserve_mask(total, want);
                self.background_mask = process::background_mask(total, self.reserved_mask);
                reports.push(ActionReport::applied(
                    "Core reservation",
                    format!(
                        "{want} of {total} logical CPUs reserved for the game",
                    ),
                    false,
                ));
            } else {
                reports.push(ActionReport::skipped(
                    "Core reservation",
                    "not enough CPUs to reserve a useful set",
                ));
            }
        } else if cfg.reserve_cores {
            reports.push(ActionReport::skipped(
                "Core reservation",
                "no game process to pin",
            ));
        }

        // --- 2. Raise the game ---------------------------------------------
        if let Some(pid) = game_pid {
            reports.push(self.boost_game(cfg, snap, pid, game_exe, samples));
        } else {
            reports.push(ActionReport::skipped(
                "Game boost",
                "no game process identified",
            ));
        }

        // --- 3. Power scheme ------------------------------------------------
        match cfg.power_plan {
            PowerPlan::Keep => reports.push(ActionReport::skipped(
                "Power plan",
                "left unchanged by configuration",
            )),
            plan => {
                match power::active_scheme() {
                    Ok(active) => {
                        if let Some(target) = power::plan_guid(plan) {
                            if power::guid_eq(&active, &target) {
                                reports.push(ActionReport::skipped(
                                    "Power plan",
                                    format!("{} is already active", power::scheme_name(&target)),
                                ));
                            } else {
                                snap.push(Change::PowerScheme {
                                    previous: power::guid_to_string(&active),
                                });
                                match power::set_active_scheme(&target) {
                                    Ok(()) => {
                                        // Capture processor settings before touching them.
                                        let tuning = power::capture_processor_tuning(&target);
                                        snap.push(Change::ProcessorTuning {
                                            scheme: power::guid_to_string(&target),
                                            min_state: tuning.min_state,
                                            max_state: tuning.max_state,
                                            boost_mode: tuning.boost_mode,
                                        });
                                        let notes = power::apply_max_performance(&target);
                                        reports.push(ActionReport::applied(
                                            "Power plan",
                                            format!(
                                                "switched to {} ({})",
                                                power::scheme_name(&target),
                                                if notes.is_empty() {
                                                    "no processor overrides".to_string()
                                                } else {
                                                    notes.join(", ")
                                                }
                                            ),
                                            true,
                                        ));
                                    }
                                    Err(e) => reports.push(ActionReport::failed(
                                        "Power plan",
                                        format!("cannot switch scheme: {e}"),
                                    )),
                                }
                            }
                        }
                    }
                    Err(e) => reports.push(ActionReport::failed(
                        "Power plan",
                        format!("cannot read the active scheme: {e}"),
                    )),
                }
            }
        }

        // --- 4. Timer resolution -------------------------------------------
        if cfg.high_timer_resolution {
            if self.timer_active {
                reports.push(ActionReport::skipped(
                    "Timer resolution",
                    "already raised",
                ));
            } else {
                let previous = system::query_timer_resolution()
                    .map(|(_, _, cur)| cur)
                    .unwrap_or(0);
                match system::set_timer_resolution(5000) {
                    Ok(granted) => {
                        snap.push(Change::TimerResolution {
                            previous_100ns: previous,
                        });
                        self.timer_active = true;
                        reports.push(ActionReport::applied(
                            "Timer resolution",
                            format!("raised to {:.2} ms", granted as f32 / 10_000.0),
                            true,
                        ));
                    }
                    Err(e) => reports.push(ActionReport::failed(
                        "Timer resolution",
                        format!("{e}"),
                    )),
                }
            }
        } else {
            reports.push(ActionReport::skipped(
                "Timer resolution",
                "disabled by configuration",
            ));
        }

        // --- 5. Keep the machine awake -------------------------------------
        if cfg.prevent_sleep {
            if self.sleep_blocked {
                reports.push(ActionReport::skipped("Sleep inhibition", "already active"));
            } else {
                match system::prevent_sleep() {
                    Ok(()) => {
                        snap.push(Change::SleepInhibition);
                        self.sleep_blocked = true;
                        reports.push(ActionReport::applied(
                            "Sleep inhibition",
                            "system and display will not sleep",
                            true,
                        ));
                    }
                    Err(e) => reports.push(ActionReport::failed(
                        "Sleep inhibition",
                        format!("{e}"),
                    )),
                }
            }
        } else {
            reports.push(ActionReport::skipped(
                "Sleep inhibition",
                "disabled by configuration",
            ));
        }

        // --- 6. Background processes ---------------------------------------
        reports.push(self.tame_background(cfg, snap, samples, game_pid));

        // --- 7. Services ----------------------------------------------------
        if cfg.manage_services {
            let running: Vec<String> = cfg
                .services_to_stop
                .iter()
                .filter(|s| !self.stopped_services.contains(*s))
                .filter(|s| services::service_exists(s))
                .filter(|s| services::service_state(s).map(|st| st.is_running()).unwrap_or(false))
                .cloned()
                .collect();
            if running.is_empty() {
                reports.push(ActionReport::skipped(
                    "Background services",
                    "nothing left to pause",
                ));
            } else {
                let mut stopped = Vec::new();
                for name in &running {
                    match services::stop_service(name, 7000) {
                        Ok(()) => {
                            snap.push(Change::Service {
                                name: name.clone(),
                                was_running: true,
                            });
                            self.stopped_services.insert(name.clone());
                            stopped.push(name.clone());
                        }
                        Err(e) => log::debug!("service {name} not stopped: {e}"),
                    }
                }
                if stopped.is_empty() {
                    reports.push(ActionReport::failed(
                        "Background services",
                        "none could be paused (needs administrator)",
                    ));
                } else {
                    reports.push(ActionReport::applied(
                        "Background services",
                        format!("paused {}", stopped.join(", ")),
                        true,
                    ));
                }
            }
        } else {
            reports.push(ActionReport::skipped(
                "Background services",
                "disabled by configuration",
            ));
        }

        // --- 8. Gaming registry tweaks -------------------------------------
        let tweaks = system::apply_gaming_tweaks(
            cfg.disable_game_dvr,
            cfg.enable_hags,
            cfg.network_latency_tweaks,
            cfg.disable_menu_hover_delay,
        );
        if tweaks.is_empty() {
            reports.push(ActionReport::skipped(
                "Gaming tweaks",
                "no registry tweaks enabled or applicable",
            ));
        } else {
            let names: Vec<String> = tweaks.iter().map(|(n, _)| n.clone()).collect();
            for (_, change) in tweaks {
                snap.push(Change::Registry {
                    root: change.root,
                    subkey: change.subkey.clone(),
                    value: change.value.clone(),
                    prev_kind: change.previous.as_ref().map(|v| v.kind),
                    prev_data: change.previous.as_ref().map(|v| v.data.clone()),
                });
            }
            reports.push(ActionReport::applied(
                "Gaming tweaks",
                names.join("; "),
                true,
            ));
        }

        self.primed = true;
        reports.push(ActionReport::applied(
            "GameMode",
            if game_exe.is_empty() {
                "activated (no game yet: waiting for a match)".to_string()
            } else {
                format!("activated for {game_exe}")
            },
            false,
        ));
        reports
    }

    // ------------------------------------------------------------- game boost

    /// Raise the game's priority, remove power throttling and widen its budget.
    fn boost_game(
        &mut self,
        cfg: &Config,
        snap: &mut Snapshot,
        pid: u32,
        exe: &str,
        samples: &[ProcessSample],
    ) -> ActionReport {
        let mut notes: Vec<String> = Vec::new();
        let mut any_applied = false;

        let target_class = match cfg.boost_level {
            BoostLevel::Off => NORMAL_PRIORITY_CLASS,
            BoostLevel::Balanced => HIGH_PRIORITY_CLASS,
            BoostLevel::Aggressive => HIGH_PRIORITY_CLASS,
            BoostLevel::Extreme => REALTIME_PRIORITY_CLASS,
        };

        // Extreme mode uses realtime, but only when the process is already
        // high-priority and the machine is not contended, otherwise the whole
        // desktop can stutter. Fall back to HIGH when that is unsafe.
        let mut effective_class = target_class;
        if target_class == REALTIME_PRIORITY_CLASS {
            let high_ok = samples
                .iter()
                .find(|s| s.pid == pid)
                .map(|s| s.cpu_percent < 90.0)
                .unwrap_or(true);
            if !high_ok {
                effective_class = HIGH_PRIORITY_CLASS;
                notes.push("realtime declined (heavy load): using High".into());
            }
        }

        if effective_class != NORMAL_PRIORITY_CLASS {
            // Journal the current class before changing it.
            if let Some(prev) = process::get_priority_class(pid) {
                if prev != effective_class && !self.touched_priority.contains(&pid) {
                    snap.push(Change::ProcessPriority {
                        pid,
                        exe: exe.to_string(),
                        previous: prev,
                    });
                    self.touched_priority.insert(pid);
                }
                match process::set_priority_class(pid, effective_class) {
                    Ok(()) => {
                        any_applied = true;
                        notes.push(format!(
                            "priority {}",
                            priority_class_label(effective_class)
                        ));
                    }
                    Err(e) => notes.push(format!("priority failed ({e})")),
                }
            }
        } else {
            notes.push("priority left at Normal (boost level Off)".into());
        }

        // Opt the game out of Windows power throttling: crucial for smooth
        // frame times when the window is not focused.
        if cfg.boost_level >= BoostLevel::Balanced && !self.touched_throttle.contains(&pid) {
            if process::set_power_throttling(pid, false).is_ok() {
                self.touched_throttle.insert(pid);
                any_applied = true;
                notes.push("power throttling disabled".into());
            }
        }

        // Normal memory priority stops Windows from reclaiming the game's pages.
        if cfg.boost_level >= BoostLevel::Balanced {
            let _ = process::set_memory_priority(
                pid,
                windows_sys::Win32::System::Threading::MEMORY_PRIORITY_NORMAL,
            );
        }

        // Disable priority boost for the game so its boosted threads never drop
        // back down mid-frame (a classic source of stutter).
        if cfg.boost_level >= BoostLevel::Aggressive {
            let _ = process::set_priority_boost(pid, true);
        }

        // Push every game thread above normal so the scheduler favours them.
        if cfg.boost_level >= BoostLevel::Aggressive {
            let level = if cfg.boost_level == BoostLevel::Extreme {
                THREAD_PRIORITY_TIME_CRITICAL
            } else {
                THREAD_PRIORITY_HIGHEST
            };
            let raised = process::boost_all_threads(pid, level);
            if raised > 0 {
                any_applied = true;
                notes.push(format!("{raised} threads raised"));
            }
        }

        // Pin the game to the reserved cores when core reservation is on.
        if cfg.reserve_cores && self.reserved_mask != 0 && !self.touched_affinity.contains(&pid) {
            if let Some(prev) = process::get_affinity(pid) {
                snap.push(Change::ProcessAffinity {
                    pid,
                    exe: exe.to_string(),
                    previous: prev,
                });
                match process::set_affinity(pid, self.reserved_mask) {
                    Ok(()) => {
                        self.touched_affinity.insert(pid);
                        any_applied = true;
                        notes.push(format!(
                            "pinned to {} reserved core(s)",
                            self.reserved_mask.count_ones()
                        ));
                    }
                    Err(e) => notes.push(format!("affinity failed ({e})")),
                }
            }
        }

        if !any_applied && notes.is_empty() {
            ActionReport::skipped("Game boost", "nothing to change")
        } else if any_applied {
            ActionReport::applied("Game boost", notes.join("; "), true)
        } else {
            ActionReport::failed("Game boost", notes.join("; "))
        }
    }

    // ------------------------------------------------------- background taming

    /// Demote CPU-hungry background processes so the game always gets the CPU.
    fn tame_background(
        &mut self,
        cfg: &Config,
        snap: &mut Snapshot,
        samples: &[ProcessSample],
        game_pid: Option<u32>,
    ) -> ActionReport {
        if !cfg.demote_background || cfg.boost_level == BoostLevel::Off {
            return ActionReport::skipped(
                "Background processes",
                "disabled by configuration",
            );
        }

        let self_pid = process::current_pid();
        let threshold = if cfg.boost_level >= BoostLevel::Aggressive {
            // Aggressive mode demotes anything that uses real CPU time.
            2.0
        } else {
            cfg.cpu_detect_threshold
        };

        let mut demoted = 0u32;
        let mut trimmed = 0u32;
        let mut freed: u64 = 0;
        let mut names: Vec<String> = Vec::new();
        let mut failures = 0u32;

        for s in samples {
            if Some(s.pid) == game_pid {
                continue;
            }
            if s.pid == self_pid || s.pid <= 4 {
                continue;
            }
            if s.is_whitelisted || s.is_configured_game {
                continue;
            }
            // Never touch processes the user is actively looking at in Extreme mode
            // either; the foreground app is what the user chose.
            if s.is_foreground && cfg.boost_level < BoostLevel::Extreme {
                continue;
            }
            if s.cpu_percent < threshold {
                // Still worth trimming a very fat idle process once.
                if cfg.trim_background_memory
                    && s.working_set > 700 * 1024 * 1024
                    && self.touched_priority.contains(&s.pid)
                {
                    if process::trim_working_set(s.pid).is_ok() {
                        trimmed += 1;
                        freed += s.working_set / 2;
                    }
                }
                continue;
            }

            let mut did_something = false;

            if !self.touched_priority.contains(&s.pid) {
                let target = if cfg.boost_level >= BoostLevel::Aggressive {
                    IDLE_PRIORITY_CLASS
                } else {
                    BELOW_NORMAL_PRIORITY_CLASS
                };
                if let Some(prev) = process::get_priority_class(s.pid) {
                    if prev != target {
                        snap.push(Change::ProcessPriority {
                            pid: s.pid,
                            exe: s.exe.clone(),
                            previous: prev,
                        });
                        match process::set_priority_class(s.pid, target) {
                            Ok(()) => {
                                self.touched_priority.insert(s.pid);
                                did_something = true;
                            }
                            Err(_) => failures += 1,
                        }
                    }
                }
            } else {
                did_something = true;
            }

            // Tell Windows this process should yield CPU to others.
            if !self.touched_throttle.contains(&s.pid) && process::set_power_throttling(s.pid, true).is_ok()
            {
                self.touched_throttle.insert(s.pid);
            }

            // Very low memory priority: the game keeps its pages, this process does not.
            if cfg.boost_level >= BoostLevel::Aggressive {
                let _ = process::set_memory_priority(
                    s.pid,
                    windows_sys::Win32::System::Threading::MEMORY_PRIORITY_VERY_LOW,
                );
            }

            // Keep background work off the reserved cores.
            if cfg.reserve_cores
                && self.background_mask != 0
                && !self.touched_affinity.contains(&s.pid)
                && s.threads > 1
            {
                if let Some(prev) = process::get_affinity(s.pid) {
                    if prev & self.background_mask == prev {
                        // Already confined; nothing to journal.
                    } else {
                        snap.push(Change::ProcessAffinity {
                            pid: s.pid,
                            exe: s.exe.clone(),
                            previous: prev,
                        });
                        if process::set_affinity(s.pid, self.background_mask).is_ok() {
                            self.touched_affinity.insert(s.pid);
                            did_something = true;
                        }
                    }
                }
            }

            if cfg.trim_background_memory && s.working_set > 150 * 1024 * 1024 {
                if process::trim_working_set(s.pid).is_ok() {
                    trimmed += 1;
                    freed += s.working_set / 2;
                    snap.push(Change::MemoryTrim {
                        pid: s.pid,
                        exe: s.exe.clone(),
                    });
                }
            }

            if did_something {
                demoted += 1;
                if names.len() < 6 {
                    names.push(format!("{} ({:.0}%)", s.exe, s.cpu_percent));
                }
            }
        }

        self.stats.processes_demoted = demoted;
        self.stats.processes_trimmed = trimmed;
        self.stats.bytes_freed = freed;

        if demoted == 0 && trimmed == 0 {
            if failures > 0 {
                ActionReport::failed(
                    "Background processes",
                    format!("{failures} process(es) could not be retuned (needs administrator)"),
                )
            } else {
                ActionReport::skipped(
                    "Background processes",
                    "nothing above the threshold right now",
                )
            }
        } else {
            let mut detail = format!("{demoted} processes demoted");
            if trimmed > 0 {
                detail.push_str(&format!(
                    ", {trimmed} trimmed (~{:.1} GB returned to the system)",
                    freed as f64 / (1024.0 * 1024.0 * 1024.0)
                ));
            }
            if !names.is_empty() {
                detail.push_str(&format!(": {}", names.join(", ")));
            }
            ActionReport::applied("Background processes", detail, true)
        }
    }

    // -------------------------------------------------------------- deactivate

    /// Undo every change recorded in `snap` and clear internal state.
    pub fn deactivate(&mut self, cfg: &Config, snap: &mut Snapshot) -> Vec<String> {
        let mut report = if cfg.restore_on_deactivate {
            snap.restore()
        } else {
            vec!["restore disabled by configuration".to_string()]
        };

        // Restart services GameMode stopped even when restore_on_deactivate is off,
        // because leaving a machine with Search/Print Spooler stopped is hostile.
        for name in self.stopped_services.clone() {
            if !snap.restored {
                if let Err(e) = services::start_service(&name, 8000) {
                    report.push(format!("could not start {name}: {e}"));
                } else {
                    report.push(format!("service {name} started"));
                }
            }
        }

        if self.sleep_blocked {
            let _ = system::allow_sleep();
        }
        if self.timer_active {
            let _ = system::clear_timer_resolution();
        }

        self.touched_priority.clear();
        self.touched_affinity.clear();
        self.touched_throttle.clear();
        self.stopped_services.clear();
        self.reserved_mask = 0;
        self.background_mask = 0;
        self.sleep_blocked = false;
        self.timer_active = false;
        self.primed = false;
        self.stats = OptimizerStats::default();

        report.push("GameMode deactivated".into());
        report
    }

    /// Refresh the statistics block without changing any system setting.
    pub fn refresh_stats(&mut self, pid: Option<u32>, exe: &str, samples: &[ProcessSample], system_cpu: f32) {
        self.stats.system_cpu = system_cpu;
        self.stats.game_pid = pid;
        self.stats.game_exe = exe.to_string();
        self.stats.cores_for_game = self.reserved_mask.count_ones() as usize;
        self.stats.cores_for_background = self.background_mask.count_ones() as usize;
        match pid.and_then(|p| samples.iter().find(|s| s.pid == p)) {
            Some(s) => {
                self.stats.game_cpu = s.cpu_percent;
                self.stats.game_ram_mb = s.ram_mb();
                self.stats.game_priority = s.priority_class;
            }
            None => {
                self.stats.game_cpu = 0.0;
                self.stats.game_ram_mb = 0.0;
                self.stats.game_priority = 0;
            }
        }
    }

    /// Re-apply the boost to a newly launched game without a full reactivation.
    pub fn rebind_game(&mut self, cfg: &Config, snap: &mut Snapshot, pid: u32, exe: &str, samples: &[ProcessSample]) -> GmResult<()> {
        self.touched_priority.remove(&pid);
        self.touched_affinity.remove(&pid);
        self.touched_throttle.remove(&pid);
        let _ = self.boost_game(cfg, snap, pid, exe, samples);
        Ok(())
    }
}

impl Default for Optimizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Suggest a sensible reserved-core count for this machine.
pub fn suggested_reserved_cores() -> usize {
    let total = process::logical_cpu_count();
    (total / 4).clamp(2, 8) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::Monitor;

    fn samples(cfg: &Config) -> Vec<ProcessSample> {
        let mut m = Monitor::new();
        let _ = m.sample(cfg);
        process::sleep_ms(150);
        m.sample(cfg)
    }

    #[test]
    fn priority_labels_are_mapped() {
        let _guard = crate::testlock::guard();
        assert_eq!(priority_class_label(HIGH_PRIORITY_CLASS), "High");
        assert_eq!(priority_class_label(IDLE_PRIORITY_CLASS), "Idle");
        assert_eq!(priority_class_label(REALTIME_PRIORITY_CLASS), "Realtime");
    }

    #[test]
    fn activate_reports_actions_and_journals_changes() {
        let _guard = crate::testlock::guard();
        let mut cfg = Config::default();
        // Keep the test machine-safe: no service stopping, no background demotion,
        // and only touch our own process.
        cfg.manage_services = false;
        cfg.demote_background = false;
        cfg.disable_game_dvr = false;
        cfg.enable_hags = false;
        cfg.network_latency_tweaks = false;
        cfg.disable_menu_hover_delay = false;
        cfg.reserve_cores = false;
        let me = process::current_pid();
        let s = samples(&cfg);
        let mut opt = Optimizer::new();
        let mut snap = Snapshot::begin("unit-test");
        let priority_before_activation = process::get_priority_class(me)
            .expect("the test process priority must be readable");
        let reports = opt.activate(&cfg, &mut snap, &s, Some(me), "selftest.exe");
        assert!(!reports.is_empty());
        assert!(opt.is_primed());
        assert!(
            reports.iter().any(|r| r.name == "Game boost"),
            "a game boost report must be produced"
        );

        // Our own process must really have been raised.
        assert_eq!(
            process::get_priority_class(me),
            Some(HIGH_PRIORITY_CLASS),
            "the game process must end up at High priority"
        );

        // Wait: capture the original priority before activating, then verify
        // deactivation restores it exactly.
        opt.deactivate(&cfg, &mut snap);
        let after = process::get_priority_class(me);
        assert_eq!(
            after, Some(priority_before_activation),
            "the original priority class must be restored"
        );
        assert!(!opt.is_primed());
    }

    #[test]
    fn timer_and_sleep_are_restored_on_deactivate() {
        let _guard = crate::testlock::guard();
        let cfg = Config::default();
        let s = samples(&cfg);
        let mut opt = Optimizer::new();
        let mut snap = Snapshot::begin("unit-test");
        let _ = opt.activate(&cfg, &mut snap, &s, None, "");
        assert!(opt.timer_active || !cfg.high_timer_resolution);
        opt.deactivate(&cfg, &mut snap);
        assert!(!opt.timer_active);
        assert!(!opt.sleep_blocked);
        assert_eq!(system::timer_ms(), 0.0);
    }

    #[test]
    fn reserved_cores_are_disjoint_from_background() {
        let _guard = crate::testlock::guard();
        let mut cfg = Config::default();
        cfg.reserve_cores = true;
        cfg.reserved_core_count = 2;
        cfg.demote_background = false;
        cfg.manage_services = false;
        cfg.disable_game_dvr = false;
        cfg.enable_hags = false;
        cfg.network_latency_tweaks = false;
        cfg.disable_menu_hover_delay = false;
        let me = process::current_pid();
        let s = samples(&cfg);
        let mut opt = Optimizer::new();
        let mut snap = Snapshot::begin("unit-test");
        let _ = opt.activate(&cfg, &mut snap, &s, Some(me), "selftest.exe");
        if opt.reserved_mask() != 0 {
            assert_eq!(opt.reserved_mask() & opt.background_mask(), 0);
            assert_eq!(
                process::get_affinity(me).unwrap(),
                opt.reserved_mask(),
                "the game must be pinned to the reserved cores"
            );
        }
        opt.deactivate(&cfg, &mut snap);
    }

    #[test]
    fn suggested_cores_is_sane() {
        let _guard = crate::testlock::guard();
        let n = suggested_reserved_cores();
        assert!((2..=8).contains(&n), "unexpected suggestion {n}");
    }
}

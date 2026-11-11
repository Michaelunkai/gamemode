//! Durable record of every system change GameMode makes.
//!
//! Each activation writes a snapshot to
//! `%APPDATA%\GameMode\snapshot.json`. If GameMode is killed (crash, power loss,
//! Task Manager) the file survives, and the next start offers to restore it.
//! Deactivation replays the snapshot back to front.

#![allow(dead_code)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{GmError, GmResult};
use crate::win::{power, process, services, system};

/// One reversible change to the machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    /// The active power scheme, as a canonical GUID string.
    PowerScheme { previous: String },
    /// Processor power settings inside a scheme.
    ProcessorTuning {
        scheme: String,
        min_state: Option<u32>,
        max_state: Option<u32>,
        boost_mode: Option<u32>,
    },
    /// A registry value that was overwritten (or created).
    Registry {
        root: u32,
        subkey: String,
        value: String,
        /// `None` when the value did not exist and must be deleted on restore.
        prev_kind: Option<u32>,
        prev_data: Option<Vec<u8>>,
    },
    /// A service GameMode stopped and must start again.
    Service { name: String, was_running: bool },
    /// A process whose priority class was changed.
    ProcessPriority {
        pid: u32,
        exe: String,
        previous: u32,
    },
    /// A process whose affinity mask was changed.
    ProcessAffinity {
        pid: u32,
        exe: String,
        previous: usize,
    },
    /// Timer resolution was raised.
    TimerResolution { previous_100ns: u32 },
    /// Sleep inhibition was requested.
    SleepInhibition,
    /// A process working set was trimmed (used to allow re-trim next session).
    MemoryTrim { pid: u32, exe: String },
    /// Only informational; restore is a no-op but the UI shows what was applied.
    Note { text: String },
}

/// A complete activation record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Schema version of the snapshot format.
    pub version: u32,
    /// RFC3339 timestamp of when GameMode was activated.
    pub created: String,
    /// Product version that created the snapshot.
    pub app_version: String,
    /// The game that triggered activation, when there was one.
    #[serde(default)]
    pub triggered_by: String,
    /// Everything that was changed, in application order.
    pub changes: Vec<Change>,
    /// True once the snapshot has been fully replayed.
    #[serde(default)]
    pub restored: bool,
}

impl Snapshot {
    pub fn path() -> PathBuf {
        Config::data_dir().join("snapshot.json")
    }

    /// Start a new, empty snapshot.
    pub fn begin(triggered_by: &str) -> Self {
        Snapshot {
            version: 1,
            created: chrono::Local::now().to_rfc3339(),
            app_version: crate::VERSION.to_string(),
            triggered_by: triggered_by.to_string(),
            changes: Vec::new(),
            restored: false,
        }
    }

    /// Record one change and persist immediately (crash-safe journaling).
    pub fn push(&mut self, change: Change) {
        self.changes.push(change);
        // Persisting on every change costs a few hundred microseconds and buys us
        // a guaranteed-correct restore even if the process dies right after.
        if let Err(e) = self.flush() {
            log::warn!("could not persist snapshot: {e}");
        }
    }

    /// Write the snapshot to disk atomically.
    pub fn flush(&self) -> GmResult<()> {
        Config::ensure_dirs()?;
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| GmError::other(format!("cannot serialise snapshot: {e}")))?;
        let path = Self::path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, &path).or_else(|_| {
            std::fs::copy(&tmp, &path)?;
            std::fs::remove_file(&tmp)
        })?;
        Ok(())
    }

    /// Load a snapshot left behind by a previous run, if any.
    pub fn load() -> Option<Self> {
        let path = Self::path();
        if !path.exists() {
            return None;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<Snapshot>(&text) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("snapshot at {path:?} is unreadable ({e}); discarding");
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// True when a previous run left unreplayed changes behind.
    pub fn is_pending() -> bool {
        Self::load().map(|s| !s.restored && !s.changes.is_empty()).unwrap_or(false)
    }

    /// Delete the snapshot file (called after a verified restore).
    pub fn discard(&self) {
        let _ = std::fs::remove_file(Self::path());
    }

    /// Replay every change back to front, then mark the snapshot restored.
    ///
    /// Returns a human-readable report of what was undone and what failed.
    pub fn restore(&mut self) -> Vec<String> {
        let mut report = Vec::new();
        let mut failures = 0usize;

        // Work backwards: the last thing applied is the first thing undone.
        for change in self.changes.iter().rev() {
            let outcome = restore_change(change);
            match outcome {
                Ok(Some(msg)) => report.push(msg),
                Ok(None) => {}
                Err(e) => {
                    failures += 1;
                    report.push(format!("could not undo {change:?}: {e}"));
                    log::warn!("restore failure: {e}");
                }
            }
        }

        if failures == 0 {
            self.restored = true;
            let _ = self.flush();
        } else {
            log::warn!("{failures} restore steps failed; snapshot kept for a retry");
        }
        report
    }
}

/// Apply the inverse of a single recorded change.
fn restore_change(change: &Change) -> GmResult<Option<String>> {
    match change {
        Change::PowerScheme { previous } => {
            if let Some(g) = parse_guid(previous) {
                power::set_active_scheme(&g)?;
                return Ok(Some(format!(
                    "power scheme restored to {}",
                    power::scheme_name(&g)
                )));
            }
            Ok(None)
        }
        Change::ProcessorTuning {
            scheme,
            min_state,
            max_state,
            boost_mode,
        } => {
            let Some(g) = parse_guid(scheme) else {
                return Ok(None);
            };
            let t = power::ProcessorTuning {
                min_state: *min_state,
                max_state: *max_state,
                boost_mode: *boost_mode,
            };
            let notes = power::restore_processor_tuning(&g, &t);
            Ok(Some(format!("processor tuning: {}", notes.join(", "))))
        }
        Change::Registry {
            root,
            subkey,
            value,
            prev_kind,
            prev_data,
        } => {
            match (prev_kind, prev_data) {
                (Some(kind), Some(data)) => {
                    system::reg_write_raw(*root, subkey, value, *kind, data)?;
                    Ok(Some(format!("registry value {subkey}\\{value} restored")))
                }
                _ => {
                    // The value was created by GameMode: remove it again.
                    system::reg_delete_value(*root, subkey, value)?;
                    Ok(Some(format!("registry value {subkey}\\{value} removed")))
                }
            }
        }
        Change::Service { name, was_running } => {
            if *was_running {
                services::start_service(name, 8000)?;
                Ok(Some(format!("service {name} started again")))
            } else {
                Ok(None)
            }
        }
        Change::ProcessPriority { pid, exe, previous } => {
            if process::is_process_alive(*pid) {
                process::set_priority_class(*pid, *previous)?;
                Ok(Some(format!("{exe} (pid {pid}) priority restored")))
            } else {
                Ok(None)
            }
        }
        Change::ProcessAffinity { pid, exe, previous } => {
            if process::is_process_alive(*pid) && *previous != 0 {
                process::set_affinity(*pid, *previous)?;
                Ok(Some(format!("{exe} (pid {pid}) affinity restored")))
            } else {
                Ok(None)
            }
        }
        Change::TimerResolution { previous_100ns } => {
            if *previous_100ns == 0 {
                system::clear_timer_resolution()?;
                Ok(Some("timer resolution released".into()))
            } else {
                system::set_timer_resolution(*previous_100ns)?;
                Ok(Some(format!(
                    "timer resolution restored to {:.2} ms",
                    *previous_100ns as f32 / 10_000.0
                )))
            }
        }
        Change::SleepInhibition => {
            system::allow_sleep()?;
            Ok(Some("sleep inhibition cleared".into()))
        }
        Change::MemoryTrim { .. } => Ok(None),
        Change::Note { text } => Ok(Some(format!("info: {text}"))),
    }
}

/// Parse a canonical `XXXXXXXX-XXXX-...` GUID string.
pub fn parse_guid(s: &str) -> Option<windows_sys::core::GUID> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return None;
    }
    let data1 = u32::from_str_radix(parts[0], 16).ok()?;
    let data2 = u16::from_str_radix(parts[1], 16).ok()?;
    let data3 = u16::from_str_radix(parts[2], 16).ok()?;
    let p4 = parts[3].as_bytes();
    let p5 = parts[4].as_bytes();
    if p4.len() != 4 || p5.len() != 12 {
        return None;
    }
    let hex = |b: &[u8]| -> Option<Vec<u8>> {
        let s = std::str::from_utf8(b).ok()?;
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
            .collect()
    };
    let h4 = hex(p4)?;
    let h5 = hex(p5)?;
    let mut data4 = [0u8; 8];
    data4[..2].copy_from_slice(&h4);
    data4[2..].copy_from_slice(&h5);
    Some(windows_sys::core::GUID {
        data1,
        data2,
        data3,
        data4,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_parse_round_trip() {
        let _guard = crate::testlock::guard();
        let g = power::GUID_HIGH_PERFORMANCE;
        let s = power::guid_to_string(&g);
        let back = parse_guid(&s).expect("parse");
        assert!(super::super::win::power::guid_eq(&back, &g));
    }

    #[test]
    fn guid_parse_rejects_garbage() {
        let _guard = crate::testlock::guard();
        assert!(parse_guid("not-a-guid").is_none());
        assert!(parse_guid("12345678-1234-1234-1234").is_none());
    }

    #[test]
    fn snapshot_serialises_and_round_trips() {
        let _guard = crate::testlock::guard();
        let mut snap = Snapshot::begin("test.exe");
        snap.push(Change::Note {
            text: "hello".into(),
        });
        snap.push(Change::Registry {
            root: 0,
            subkey: "Software\\Test".into(),
            value: "V".into(),
            prev_kind: Some(4),
            prev_data: Some(vec![1, 0, 0, 0]),
        });
        let text = serde_json::to_string(&snap).expect("serialise");
        let back: Snapshot = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back.changes.len(), 2);
        assert_eq!(back.triggered_by, "test.exe");
        assert!(!back.restored);
    }

    #[test]
    fn registry_change_restores_exact_value() {
        let _guard = crate::testlock::guard();
        // Journal a real registry change through the snapshot machinery and
        // prove the restore puts the original bytes back.
        let key = r"Software\GameMode\SnapshotTest";
        let _ = system::reg_delete_value(0, key, "Round");
        let mut snap = Snapshot::begin("unit-test");
        let prev = system::reg_read(0, key, "Round");
        assert!(prev.is_none(), "test needs a clean starting point");
        system::reg_write_dword(0, key, "Round", 777).expect("write 777");
        snap.push(Change::Registry {
            root: 0,
            subkey: key.into(),
            value: "Round".into(),
            prev_kind: None,
            prev_data: None,
        });
        assert_eq!(system::reg_read(0, key, "Round").unwrap().as_u32(), Some(777));

        let report = snap.restore();
        assert!(snap.restored, "restore should succeed: {report:?}");
        assert!(
            system::reg_read(0, key, "Round").is_none(),
            "the created value must be deleted again"
        );
    }

    #[test]
    fn restore_of_existing_registry_value_puts_old_data_back() {
        let _guard = crate::testlock::guard();
        let key = r"Software\GameMode\SnapshotTest2";
        system::reg_write_dword(0, key, "Existing", 42).expect("seed");
        let mut snap = Snapshot::begin("unit-test");
        let prev = system::reg_read(0, key, "Existing").expect("seed readable");
        snap.push(Change::Registry {
            root: 0,
            subkey: key.into(),
            value: "Existing".into(),
            prev_kind: Some(prev.kind),
            prev_data: Some(prev.data.clone()),
        });
        system::reg_write_dword(0, key, "Existing", 99).expect("overwrite");
        assert_eq!(
            system::reg_read(0, key, "Existing").unwrap().as_u32(),
            Some(99)
        );

        snap.restore();
        assert_eq!(
            system::reg_read(0, key, "Existing").unwrap().as_u32(),
            Some(42),
            "original value must come back"
        );
        let _ = system::reg_delete_value(0, key, "Existing");
    }

    #[test]
    fn timer_resolution_restores_to_previous() {
        let _guard = crate::testlock::guard();
        let before = system::query_timer_resolution().map(|(_, _, cur)| cur).unwrap_or(0);
        let mut snap = Snapshot::begin("unit-test");
        if system::set_timer_resolution(5000).is_ok() {
            snap.push(Change::TimerResolution {
                previous_100ns: before,
            });
            snap.restore();
            let after = system::query_timer_resolution().map(|(_, _, cur)| cur).unwrap_or(0);
            assert_eq!(after, before, "timer resolution must return to its old value");
        }
    }

    #[test]
    fn power_scheme_restore_returns_to_original() {
        let _guard = crate::testlock::guard();
        let original = power::active_scheme().expect("active scheme");
        let mut snap = Snapshot::begin("unit-test");
        snap.push(Change::PowerScheme {
            previous: power::guid_to_string(&original),
        });
        snap.restore();
        let after = power::active_scheme().expect("active scheme after");
        assert_eq!(
            power::guid_to_string(&after),
            power::guid_to_string(&original),
            "the original scheme must be active again"
        );
    }
}

//! Shared application state: the session plus the command queue the UI drives.
//!
//! Every source of change (tray menu, global hotkey, control panel, automatic
//! detection) reduces to one [`Command`]. The main loop drains the queue once per
//! frame and applies each command to the [`Session`], so there is exactly one
//! place where machine state changes and exactly one place that logs it.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::config::Config;
use gm_core::session::{Session, SessionState, Trigger};

/// How often the session is ticked (detection + statistics refresh).
pub const TICK: Duration = Duration::from_millis(500);

/// Something the main loop should do to the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Turn optimisation on.
    Enable,
    /// Turn optimisation off, restoring every change.
    Disable,
    /// Flip the current state.
    Toggle,
    /// Drop the runtime session and re-read `config.toml` from disk.
    ReloadConfig,
    /// Show the control panel window.
    ShowWindow,
    /// Hide the control panel window.
    HideWindow,
    /// Stop GameMode completely: deactivate, restore, exit.
    Quit,
    /// Open `config.toml` in the user's editor.
    OpenConfig,
    /// Toggle "start with Windows".
    SetAutostart(bool),
}

/// Shared state handed to the tray, the hotkey thread and the UI.
pub struct AppState {
    /// The single source of truth for the machine's tuning state.
    pub session: Mutex<Session>,
    /// Commands collected from other threads, drained by the main loop.
    tx: Sender<Command>,
    rx: Mutex<Receiver<Command>>,
    /// Set when the user asked the control panel to become visible.
    pub want_visible: Mutex<bool>,
    /// Set when the process should exit.
    pub want_quit: Mutex<bool>,
    /// When the process started, for the uptime readout.
    pub started_at: Instant,
    /// Whether the tray tooltip should be refreshed on the next frame.
    pub menu_dirty: Mutex<bool>,
}

impl AppState {
    /// Wraps a session in shared state.
    pub fn new(session: Session) -> Arc<Self> {
        let (tx, rx) = channel();
        Arc::new(AppState {
            session: Mutex::new(session),
            tx,
            rx: Mutex::new(rx),
            want_visible: Mutex::new(false),
            want_quit: Mutex::new(false),
            started_at: Instant::now(),
            menu_dirty: Mutex::new(true),
        })
    }

    /// Queues a command from any thread.
    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    /// Drains every queued command.
    pub fn drain(&self) -> Vec<Command> {
        let rx = match self.rx.lock() {
            Ok(rx) => rx,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut out = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            out.push(cmd);
        }
        out
    }

    /// Applies one queued command to the live session/config.
    pub fn apply(&self, command: Command) {
        match command {
            Command::Enable => {
                self.with_session(|s| {
                    s.enable(Trigger::Manual);
                });
            }
            Command::Disable => {
                self.with_session(|s| {
                    s.disable("user disable");
                });
            }
            Command::Toggle => {
                self.with_session(|s| {
                    s.toggle();
                });
            }
            Command::ReloadConfig => {
                if let Ok(cfg) = Config::load() {
                    self.with_session(|s| {
                        s.apply_config(cfg);
                    });
                }
            }
            Command::ShowWindow => {
                *self.want_visible.lock().unwrap_or_else(|e| e.into_inner()) = true;
            }
            Command::HideWindow => {
                *self.want_visible.lock().unwrap_or_else(|e| e.into_inner()) = false;
            }
            Command::Quit => {
                *self.want_quit.lock().unwrap_or_else(|e| e.into_inner()) = true;
            }
            Command::OpenConfig => {
                let _ = gm_core::win::system::open_in_editor(&Config::config_path());
            }
            Command::SetAutostart(on) => {
                let _ = crate::autostart::set(on);
                self.mark_menu_dirty();
            }
        }
        self.mark_menu_dirty();
    }

    /// Runs `f` with exclusive access to the session.
    pub fn with_session<R>(&self, f: impl FnOnce(&mut Session) -> R) -> R {
        let mut guard = match self.session.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        f(&mut guard)
    }

    /// Snapshot of the values the UI and tray need.
    pub fn snapshot(&self) -> UiSnapshot {
        self.with_session(|s| UiSnapshot {
            state: s.state,
            is_on: s.is_on(),
            status: format!("{} · {}", s.state.label(), s.detection.reason),
            detection: s.detection.title.clone(),
            detection_exe: s.detection.exe.clone(),
            cpu: s.samples.iter().map(|p| p.cpu_percent).sum::<f32>().min(100.0),
            games_found: s.detection.candidates.len(),
            hotkey: s.config.hotkey_label(),
            trigger: s.trigger.map(|t| match t {
                Trigger::Manual => "manual",
                Trigger::Automatic => "automatic",
            }),
            actions: s
                .last_reports
                .iter()
                .map(|r| (r.name.clone(), format!("{} · {}", r.status.symbol(), r.detail)))
                .collect(),
            log: s.log.iter().map(|l| l.text.clone()).collect(),
            samples: s
                .samples
                .iter()
                .map(|p| (p.exe.clone(), p.cpu_percent))
                .collect(),
        })
    }

    /// Uptime of this process.
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Reads the configuration currently in effect.
    pub fn config(&self) -> Config {
        self.with_session(|s| s.config.clone())
    }

    /// Flags the tray menu for a rebuild.
    pub fn mark_menu_dirty(&self) {
        if let Ok(mut d) = self.menu_dirty.lock() {
            *d = true;
        }
    }
}

/// The subset of session state the UI renders.
#[derive(Debug, Clone)]
pub struct UiSnapshot {
    pub state: SessionState,
    pub is_on: bool,
    pub status: String,
    pub detection: String,
    pub detection_exe: String,
    pub cpu: f32,
    pub games_found: usize,
    pub hotkey: String,
    pub trigger: Option<&'static str>,
    pub actions: Vec<(String, String)>,
    pub log: Vec<String>,
    pub samples: Vec<(String, f32)>,
}
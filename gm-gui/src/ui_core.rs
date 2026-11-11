//! The control panel window, drawn with egui 0.37.
//!
//! The window is a pure view: it reads a `UiSnapshot` and emits `Command`s. It
//! never touches the session directly, so closing the window can never leave the
//! machine half-tuned.

use std::sync::Arc;

use eframe::egui;
use gm_core::config::{BoostLevel, DetectMode, PowerPlan};
use gm_core::session::{LogLevel, SessionState};

use crate::app::{AppState, Command, UiSnapshot};
use crate::lifecycle;
use crate::hotkeys::HotkeyManager;
use crate::tray::Tray;

/// Accent colour matching the icon gradient.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(96, 165, 250);
/// Green used for the active state.
const ACTIVE: egui::Color32 = egui::Color32::from_rgb(52, 211, 153);
/// Amber used for the arming state.
const ARMING: egui::Color32 = egui::Color32::from_rgb(251, 191, 36);
/// Muted text colour.
const MUTED: egui::Color32 = egui::Color32::from_rgb(148, 163, 184);

/// The eframe application.
pub struct Panel {
    state: Arc<AppState>,
    snapshot: UiSnapshot,
    tab: Tab,
    /// Editable copy of the configuration.
    config: gm_core::config::Config,
    /// Set after the user edits a setting, so it is only saved once per change.
    dirty: bool,
    tray: Arc<std::sync::Mutex<Option<Tray>>>,
    hotkey: Option<HotkeyManager>,
    /// Whether the window is currently shown (mirrors AppState::want_visible).
    visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Tuning,
    Games,
    Log,
}

impl Panel {
    /// Builds the panel from shared state.
    pub fn new(state: Arc<AppState>, tray: Arc<std::sync::Mutex<Option<Tray>>>, hotkey: Option<HotkeyManager>, visible: bool) -> Self {
        let config = state.config();
        let snapshot = state.snapshot();
        Self {
            state,
            snapshot,
            tab: Tab::Overview,
            config,
            dirty: false,
            tray,
            hotkey,
            visible,
        }
    }

    /// Full teardown called once when the process is about to exit.
    fn shutdown(&mut self) {
        // Restore every machine change before the window goes away.
        let mut s = self.state.session.lock().unwrap_or_else(|e| e.into_inner());
        let _ = lifecycle::shutdown(&mut s, "process exit");
    }
}

impl eframe::App for Panel {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.tick();
        // Keep ticking while the window is hidden so detection and the tray stay live.
        ui.ctx().request_repaint_after(crate::app::TICK);
        // Poll hotkey presses from the dedicated thread.
        if let Some(hk) = self.hotkey.as_ref() {
            if hk.poll().is_some() {
                self.state.send(Command::Toggle);
            }
        }

        // Surface a Quit that came from another thread (IPC, tray on another loop).
        if *self.state.want_quit.lock().unwrap_or_else(|e| e.into_inner()) {
            self.shutdown();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Show / hide requests from the tray, IPC and hotkey threads.
        let want = *self.state.want_visible.lock().unwrap_or_else(|e| e.into_inner());
        if want != self.visible {
            self.visible = want;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(want));
            if want {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }

        egui::Panel::top("header").show(ui, |ui| self.header(ui));
        egui::Panel::bottom("footer").show(ui, |ui| self.footer(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            match self.tab {
                Tab::Overview => self.overview(ui),
                Tab::Tuning => self.tuning(ui),
                Tab::Games => self.games(ui),
                Tab::Log => self.log(ui),
            }
        });
    }
}

impl Panel {
    /// Applies queued commands, refreshes the snapshot, syncs the tray and the hotkey.
    fn tick(&mut self) {
        // Tray menu clicks and a double click on the icon reach the main loop as commands.
        while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if let Some(command) = crate::tray::command_for(ev.id.as_ref()) {
                self.state.send(command);
            }
        }
        while let Ok(ev) = tray_icon::TrayIconEvent::receiver().try_recv() {
            if let tray_icon::TrayIconEvent::DoubleClick { .. } = ev {
                self.state.send(Command::ShowWindow);
            }
        }
        for command in self.state.drain() {
            self.state.apply(command);
        }

        self.refresh();
        self.sync_tray();
    }

    /// Pulls a fresh snapshot and config copy.
    fn refresh(&mut self) {
        self.snapshot = self.state.snapshot();
        if !self.dirty {
            self.config = self.state.config();
        }
    }

    /// Keep the tray icon/tooltip/menu in sync with the session state.
    fn sync_tray(&mut self) {
        let tray = match self.tray.lock() {
            Ok(t) => t,
            Err(e) => e.into_inner(),
        };
        let Some(t) = tray.as_ref() else { return };
        let s = self.state.session.lock().unwrap_or_else(|e| e.into_inner());
        t.sync(
            s.state,
            &s.detection.title,
            &s.config.hotkey_label(),
        );
        t.sync_autostart();
        drop(s);
    }

    /// Persists pending configuration edits.
    fn save(&mut self) {
        self.dirty = false;
        let mut s = self.state.session.lock().unwrap_or_else(|e| e.into_inner());
        s.config = self.config.clone();
        match s.config.save() {
            Ok(()) => s.push_log(LogLevel::Info, "Settings saved"),
            Err(e) => s.push_log(
                LogLevel::Warn,
                format!("Could not save settings: {e}"),
            ),
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("GameMode").color(ACCENT).strong());
            ui.selectable_value(&mut self.tab, Tab::Overview, "Overview");
            ui.selectable_value(&mut self.tab, Tab::Tuning, "Tuning");
            ui.selectable_value(&mut self.tab, Tab::Games, "Games");
            ui.selectable_value(&mut self.tab, Tab::Log, "Log");
            ui.separator();
            let status = match self.snapshot.state {
                SessionState::Active => egui::RichText::new("ACTIVE").color(ACTIVE).strong(),
                SessionState::Arming => egui::RichText::new("ARMING").color(ARMING).strong(),
                SessionState::Draining => egui::RichText::new("RESTORING").color(ARMING).strong(),
                SessionState::Idle => egui::RichText::new("OFF").color(MUTED).strong(),
            };
            ui.label(status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Quit GameMode").clicked() {
                    self.shutdown();
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
        ui.separator();
    }

    fn footer(&self, ui: &mut egui::Ui) {
        ui.separator();
        let up = self.state.uptime();
        ui.horizontal(|ui| {
            ui.label(format!("State: {}", self.snapshot.state.label()));
            ui.label(format!("Game: {}", self.snapshot.detection_exe));
            ui.label(format!("Uptime: {:02}:{:02}:{:02}", up.as_secs() / 3600, (up.as_secs() % 3600) / 60, up.as_secs() % 60));
        });
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        let on = self.snapshot.is_on;
        let color = if on { ACTIVE } else { MUTED };
        egui::Frame::default().fill(color.linear_multiply(0.10)).corner_radius(6.0).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading(egui::RichText::new(if on { "ON" } else { "OFF" }).color(color).strong());
                ui.label(format!("{} · {}", self.snapshot.status, self.snapshot.hotkey));
                let game = if self.snapshot.detection.is_empty() { "none" } else { self.snapshot.detection.as_str() };
                ui.label(format!("Detected: {game} · candidates: {} · CPU: {:.0}%", self.snapshot.games_found, self.snapshot.cpu));
                if let Some(t) = self.snapshot.trigger {
                    ui.label(format!("Started by: {t}"));
                }
                ui.add_space(4.0);
                if ui.button(if on { "Disable GameMode" } else { "Enable GameMode" }).clicked() {
                    if on {
                        self.state.send(Command::Disable);
                    } else {
                        self.state.send(Command::Enable);
                    }
                }
            });
        });
        ui.add_space(12.0);

        ui.collapsing("What is running right now", |ui| {
            egui::Grid::new("proc_grid").striped(true).show(ui, |ui| {
                ui.label("Process"); ui.label("CPU %"); ui.end_row();
                for p in &self.snapshot.samples {
                    ui.label(&p.0);
                    ui.label(format!("{:.1}", p.1));
                    ui.end_row();
                }
                if self.snapshot.samples.is_empty() {
                    ui.label("none sampled yet");
                    ui.end_row();
                }
            });
        });

        ui.add_space(8.0);
        ui.label(format!("Last actions: {}", self.snapshot.actions.len()));
        egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
            for (name, detail) in &self.snapshot.actions {
                ui.label(format!("{name} — {detail}"));
            }
        });
    }

    fn tuning(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.label("Set the boost level for the detected game:");
        egui::ComboBox::from_label("Boost level")
            .selected_text(match self.config.boost_level {
                BoostLevel::Off => "Off (game boost only)",
                BoostLevel::Balanced => "Balanced",
                BoostLevel::Aggressive => "Aggressive",
                BoostLevel::Extreme => "Extreme",
            })
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut self.config.boost_level, BoostLevel::Off, "Off (game boost only)").changed();
                changed |= ui.selectable_value(&mut self.config.boost_level, BoostLevel::Balanced, "Balanced").changed();
                changed |= ui.selectable_value(&mut self.config.boost_level, BoostLevel::Aggressive, "Aggressive").changed();
                changed |= ui.selectable_value(&mut self.config.boost_level, BoostLevel::Extreme, "Extreme").changed();
            });
        ui.add_space(8.0);
        ui.label("Power scheme while active:");
        egui::ComboBox::from_label("Power plan")
            .selected_text(match self.config.power_plan {
                PowerPlan::Keep => "Leave unchanged",
                PowerPlan::Balanced => "Balanced",
                PowerPlan::HighPerformance => "High performance",
                PowerPlan::UltimatePerformance => "Ultimate performance",
            })
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut self.config.power_plan, PowerPlan::Keep, "Leave unchanged").changed();
                changed |= ui.selectable_value(&mut self.config.power_plan, PowerPlan::Balanced, "Balanced").changed();
                changed |= ui.selectable_value(&mut self.config.power_plan, PowerPlan::HighPerformance, "High performance").changed();
                changed |= ui.selectable_value(&mut self.config.power_plan, PowerPlan::UltimatePerformance, "Ultimate performance").changed();
            });
        ui.add_space(8.0);
        changed |= ui.checkbox(&mut self.config.reserve_cores, "Reserve CPU cores for the game").on_hover_text("Pins background work off the cores the game uses.").changed();
        changed |= ui.checkbox(&mut self.config.demote_background, "Demote background processes").on_hover_text("Lowers the priority of non-game processes so the game gets CPU first.").changed();
        changed |= ui.checkbox(&mut self.config.high_timer_resolution, "0.5 ms timer resolution (lower input latency)").changed();
        changed |= ui.checkbox(&mut self.config.prevent_sleep, "Keep system and display awake while gaming").changed();
        changed |= ui.checkbox(&mut self.config.disable_game_dvr, "Disable Game DVR / Game Bar background recording").changed();
        changed |= ui.checkbox(&mut self.config.enable_hags, "Enable HAGS (GPU scheduling)").changed();
        changed |= ui.checkbox(&mut self.config.network_latency_tweaks, "Network latency tweaks (TCP ACK)").changed();
        changed |= ui.checkbox(&mut self.config.disable_menu_hover_delay, "Remove menu hover delay").changed();
        changed |= ui.checkbox(&mut self.config.disable_fullscreen_optimizations, "Disable fullscreen optimisations for registered games").changed();
        ui.add_space(8.0);
        changed |= ui.checkbox(&mut self.config.manage_services, "Manage Windows services").on_hover_text("Pause non-essential services while active (requires admin).").changed();
        ui.add_space(8.0);
        changed |= ui.checkbox(&mut self.config.verbose_logging, "Verbose logging").changed();
        ui.add_space(12.0);
        self.dirty |= changed;
        if ui.button("Save settings").clicked() {
            self.save();
        }
    }

    fn games(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.label("Detection mode:");
        egui::ComboBox::from_label("Mode")
            .selected_text(match self.config.detect_mode {
                DetectMode::ListOnly => "Game list only",
                DetectMode::ForegroundOnly => "Foreground fullscreen only",
                DetectMode::Smart => "Smart (list + fullscreen + load)",
            })
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut self.config.detect_mode, DetectMode::ListOnly, "Game list only").changed();
                changed |= ui.selectable_value(&mut self.config.detect_mode, DetectMode::ForegroundOnly, "Foreground fullscreen only").changed();
                changed |= ui.selectable_value(&mut self.config.detect_mode, DetectMode::Smart, "Smart (list + fullscreen + load)").changed();
            });
        ui.add_space(8.0);
        ui.label("Listed game executables:");
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            for entry in &mut self.config.games {
                let label = entry.display();
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut entry.disabled, label).changed();
                    ui.label(format!("({})", entry.exe));
                });
            }
        });
        ui.add_space(8.0);
        ui.label("Automatic activation:");
        changed |= ui.checkbox(&mut self.config.auto_activate, "Activate automatically when a game is detected").on_hover_text("Turns GameMode on without you touching anything.").changed();
        changed |= ui.add(egui::DragValue::new(&mut self.config.activation_delay_secs).prefix("Arm delay (s): ")).changed();
        changed |= ui.add(egui::DragValue::new(&mut self.config.deactivation_grace_secs).prefix("Grace after game closes (s): ")).changed();
        self.dirty |= changed;
        ui.add_space(8.0);
        if ui.button("Save settings").clicked() {
            self.save();
        }
    }

    fn log(&mut self, ui: &mut egui::Ui) {
        ui.label("Session log");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for line in &self.snapshot.log {
                ui.label(line);
            }
        });
        ui.add_space(8.0);
        if ui.button("Open log file").clicked() {
            let p = gm_core::data_dir().join("gamemode.log");
            let _ = gm_core::win::system::open_in_editor(&p);
        }
    }
}

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

        // ── Master toggle card ────────────────────────────────────────────
        egui::Frame::default()
            .fill(color.linear_multiply(0.10))
            .corner_radius(8.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading(
                            egui::RichText::new(if on { "● GAME MODE ON" } else { "○ GAME MODE OFF" })
                                .color(color)
                                .size(20.0),
                        );
                        ui.label(format!("{} · {}", self.snapshot.status, self.snapshot.hotkey));
                        let game = if self.snapshot.detection.is_empty() {
                            "none"
                        } else {
                            self.snapshot.detection.as_str()
                        };
                        ui.label(format!(
                            "Detected: {game} · candidates: {} · CPU: {:.0}%",
                            self.snapshot.games_found,
                            self.snapshot.cpu
                        ));
                        if let Some(t) = self.snapshot.trigger {
                            ui.label(format!("Started by: {t}"));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(if on { "⏹  Disable" } else { "▶  Enable" }).min_size(egui::vec2(120.0, 36.0)))
                            .clicked()
                        {
                            if on {
                                self.state.send(Command::Disable);
                            } else {
                                self.state.send(Command::Enable);
                            }
                        }
                    });
                });
            });
        ui.add_space(10.0);

        // ── Real-time gauges ──────────────────────────────────────────────
        ui.add_space(10.0);
        ui.heading("Performance");
        let snap = &self.snapshot;

        // Trend series for the sparklines, split by metric.
        let cpu_hist: Vec<f32> = snap.history.iter().map(|h| h.0).collect();
        let gpu_hist: Vec<f32> = snap.history.iter().map(|h| h.1).collect();
        let ram_hist: Vec<f32> = snap.history.iter().map(|h| h.2).collect();

        // ── Three system tiles ─────────────────────────────────────────
        ui.columns(3, |cols| {
            Self::metric_tile(&mut cols[0], "CPU", snap.cpu, &cpu_hist, Self::gauge_color(snap.cpu, 60.0, 85.0));
            Self::metric_tile(&mut cols[1], "GPU", snap.gpu_usage, &gpu_hist, Self::gauge_color(snap.gpu_usage, 60.0, 85.0));
            Self::metric_tile(&mut cols[2], "RAM", snap.ram_percent, &ram_hist, Self::gauge_color(snap.ram_percent, 60.0, 85.0));
        });
        ui.add_space(8.0);
        ui.label(format!(
            "CPU: {} cores · {}",
            snap.cpu_count, snap.cpu_name
        ));
        ui.label(format!(
            "RAM: {:.1} / {:.1} GB used · {:.0}%",
            (snap.total_ram_mb - snap.available_ram_mb) / 1024.0,
            snap.total_ram_mb / 1024.0,
            snap.ram_percent
        ));
        ui.label(format!("GPU engines reporting: {}", snap.gpu_engines));
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(8.0);

        // ── Top consumers ─────────────────────────────────────────────
        ui.columns(2, |cols| {
            cols[0].vertical(|ui| {
                ui.strong("CPU");
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    egui::Grid::new("top_cpu_grid").num_columns(3).show(ui, |ui| {
                        for (exe, cpu, ram, is_game, demoted) in &self.snapshot.top_cpu {
                            let name = if *is_game {
                                egui::RichText::new(exe).strong().color(ACTIVE)
                            } else {
                                egui::RichText::new(exe)
                            };
                            ui.label(name);
                            ui.label(format!("{cpu:.0}%"));
                            ui.label(
                                egui::RichText::new(format!("{ram:.0} MB{}", if *demoted { " ↓" } else { "" }))
                                    .color(if *demoted { ACTIVE } else { MUTED }),
                            );
                            ui.end_row();
                        }
                    });
                });
            });
            cols[1].vertical(|ui| {
                ui.strong("RAM");
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    egui::Grid::new("top_ram_grid").num_columns(3).show(ui, |ui| {
                        for (exe, cpu, ram, is_game, _) in &self.snapshot.top_ram {
                            let name = if *is_game {
                                egui::RichText::new(exe).strong().color(ACTIVE)
                            } else {
                                egui::RichText::new(exe)
                            };
                            ui.label(name);
                            ui.label(format!("{ram:.0} MB"));
                            ui.label(format!("{cpu:.0}%"));
                            ui.end_row();
                        }
                        if self.snapshot.top_ram.is_empty() {
                            ui.label(egui::RichText::new("no processes sampled yet").color(MUTED));
                            ui.end_row();
                        }
                    });
                });
            });
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // ── Activation report (immediate readable list) ──────────────────
        if on && !self.snapshot.activation_report.is_empty() {
                ui.separator();
                ui.heading("✅ What was changed to maximize performance");
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        egui::Grid::new("activation_grid")
                            .striped(true)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("Category").strong());
                                ui.label(egui::RichText::new("Change").strong());
                                ui.label(egui::RichText::new("Detail").strong());
                                ui.end_row();
                                for (cat, change, detail) in &self.snapshot.activation_report {
                                    ui.label(cat);
                                    ui.label(change);
                                    ui.label(detail);
                                    ui.end_row();
                                }
                            });
                    });

                ui.add_space(8.0);
                ui.label(format!(
                    "Resources saved: CPU {:.0}% freed  ·  RAM {:.0} MB freed",
                    self.snapshot.cpu_saved, self.snapshot.ram_saved_mb
                ));
        }

        ui.add_space(8.0);

        // ── Process grid ──────────────────────────────────────────────────
        ui.collapsing("🖥️  What is running right now", |ui| {
            egui::Grid::new("proc_grid").striped(true).show(ui, |ui| {
                ui.label("Process");
                ui.label("CPU %");
                ui.label("RAM MB");
                ui.end_row();
                for (exe, cpu) in &self.snapshot.samples {
                    ui.label(exe);
                    ui.label(format!("{:.1}", cpu));
                    // Find RAM for this process (approximate; detailed per-proc RAM
                    // requires a second pass, here we use the overall available).
                    ui.label("—");
                    ui.end_row();
                }
                if self.snapshot.samples.is_empty() {
                    ui.label("none sampled yet");
                    ui.end_row();
                }
            });
        });

        ui.add_space(8.0);

        // ── Last actions (simple list) ────────────────────────────────────
        ui.collapsing("📋 Last actions", |ui| {
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .show(ui, |ui| {
                    for (name, detail) in &self.snapshot.actions {
                        ui.label(format!("{name} — {detail}"));
                    }
                    if self.snapshot.actions.is_empty() {
                        ui.label("no actions yet");
                    }
                });
        });
    }

    /// One dashboard tile: title, value, sparkline trend.
    fn metric_tile(
        ui: &mut egui::Ui,
        label: &str,
        value: f32,
        history: &[f32],
        accent: egui::Color32,
    ) {
        egui::Frame::default()
            .fill(egui::Color32::from_rgb(24, 24, 27))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(39, 39, 42)))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_min_width(170.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(label).size(11.0).color(MUTED).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{value:.0}%"))
                                .size(22.0)
                                .color(accent)
                                .strong(),
                        );
                    });
                });
                Self::sparkline(ui, history, accent, 26.0);
            });
    }

    /// A tiny filled sparkline drawn as a polyline inside a fixed height.
    fn sparkline(ui: &mut egui::Ui, points: &[f32], color: egui::Color32, height: f32) {
        let desired = egui::vec2(ui.available_width(), height);
        let (rect, _) = ui.allocate_at_least(desired, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::TRANSPARENT);

        let n = points.len().max(1);
        let max = points.iter().fold(1.0f32, |a, b| a.max(*b));
        let step = rect.width() / n.max(1) as f32;

        let pts: Vec<egui::Pos2> = points
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let y = rect.bottom() - (v / max) * (rect.height() - 4.0) - 2.0;
                egui::pos2(rect.left() + i as f32 * step, y)
            })
            .collect();

        if pts.len() > 1 {
            painter.add(egui::Shape::line(
                pts,
                egui::Stroke::new(1.6, color.gamma_multiply(0.9)),
            ));
        }
    }

    /// Pick a colour for a gauge: green below `ok`, amber below `warn`, red above.
    fn gauge_color(value: f32, ok: f32, warn: f32) -> egui::Color32 {
        if value <= ok {
            ACTIVE
        } else if value <= warn {
            ARMING
        } else {
            egui::Color32::from_rgb(248, 113, 113)
        }
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
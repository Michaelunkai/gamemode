//! GameMode — a system-tray gaming optimiser for Windows.
//!
//! The binary is thin: it owns the tray icon, the global hotkey and the control
//! panel, and delegates every machine change to `gm-core`.
//!
//! Behaviour contract:
//!  * the tray icon exists for as long as the process lives, in every state;
//!  * quitting from the tray stops GameMode completely — the session is
//!    deactivated (every machine change restored) and only then does the process
//!    exit;
//!  * a second launch hands its command to the running instance and exits.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod hotkeys;
mod icons;
mod lifecycle;
mod single_instance;
mod tray;
mod ui_core;

use std::sync::Arc;

use eframe::egui;
use gm_core::config::Config;
use gm_core::session::Session;
use gm_core::win::ipc::{IpcCommand, PipeServer};

use app::{AppState, Command};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let headless = args.iter().any(|a| a == "--headless" || a == "--console");
    // --tray (passed by the autostart entry): start hidden in the tray, no window.
    let tray_start = args.iter().any(|a| a == "--tray");
    let hold = args
        .iter()
        .position(|a| a == "--hold")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u64>().ok());

    gm_core::init_logging(headless);

    // ---- single instance -------------------------------------------------
    // A second launch never starts a competing optimiser; it forwards one
    // command to the instance that already owns the machine state. With no
    // recognised verb we send a harmless "status" ping which the running
    // instance answers before the caller exits.
    let verb = single_instance::verb_from_args(&args).unwrap_or(single_instance::Verb::Status);
    if single_instance::send_to_running(verb) {
        return;
    }

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gamemode: could not load configuration: {e}");
            Config::default()
        }
    };

    if headless {
        std::process::exit(run_headless(&mut config.clone(), hold.unwrap_or(0)));
    }

    std::process::exit(run_gui(config, tray_start));
}

use std::sync::Mutex;

/// Boots the control panel, the tray icon and the IPC server.
fn run_gui(config: Config, tray_start: bool) -> i32 {
    gm_core::init_logging(config.verbose_logging);

    let mut session = Session::new(config.clone());
    for line in lifecycle::recover(&mut session) {
        session.push_log(gm_core::session::LogLevel::Info, line);
    }
    let state = AppState::new(session);
    let tstate = state.clone();

    let tray = match tray::Tray::new(&tstate) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("GameMode: tray icon unavailable: {e}");
            None
        }
    };
    let tray = Arc::new(Mutex::new(tray));

    start_ipc(state.clone());
    let hk = start_hotkeys(&config);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_visible(!tray_start)
            .with_inner_size([700.0, 520.0])
            .with_min_inner_size([520.0, 360.0]),
        ..Default::default()
    };

    let panel = ui_core::Panel::new(state, tray, hk, !tray_start);

    let result = eframe::run_native(
        "GameMode",
        options,
        Box::new(|_cc| Ok(Box::new(panel))),
    );

    let _ = result;
    0
}

/// Serves the control pipe so a second launch can drive this instance.
///
/// The server lives on its own thread; every accepted command is translated into
/// a [`Command`] and pushed onto the main loop's queue.
fn start_ipc(state: Arc<AppState>) {
    std::thread::spawn(move || {
        let Ok(server) = PipeServer::new() else {
            log::warn!("control pipe unavailable; single-instance IPC disabled");
            return;
        };
        while let Some(raw) = server.accept_command() {
            match IpcCommand::parse(raw.trim()) {
                Some(IpcCommand::Toggle) => state.send(Command::Toggle),
                Some(IpcCommand::Enable) => state.send(Command::Enable),
                Some(IpcCommand::Disable) => state.send(Command::Disable),
                Some(IpcCommand::Show) => state.send(Command::ShowWindow),
                Some(IpcCommand::Hide) => state.send(Command::HideWindow),
                Some(IpcCommand::Quit) => state.send(Command::Quit),
                Some(IpcCommand::Reload) => state.send(Command::ReloadConfig),
                _ => {}
            }
        }
    });
}

/// Starts the global toggle hotkey, when the user enabled one.
fn start_hotkeys(config: &Config) -> Option<hotkeys::HotkeyManager> {
    if !config.hotkey_toggle_enabled {
        return None;
    }
    Some(hotkeys::HotkeyManager::start(
        config.hotkey_modifiers,
        config.hotkey_virtual_key,
    ))
}

/// Headless mode used for verification: apply, hold, restore, report.
fn run_headless(config: &mut Config, hold_seconds: u64) -> i32 {
    use gm_core::session::Trigger;

    gm_core::init_logging(config.verbose_logging);
    let mut session = Session::new(config.clone());

    let reports = session.enable(Trigger::Manual);
    println!("PHASE:activated");
    for r in &reports {
        println!("ACTION:{}:{}:{}", r.name, r.status.symbol(), r.detail);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(hold_seconds.max(1));
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    let restored = session.disable("headless verification finished");
    println!("PHASE:deactivated");
    for line in &restored {
        println!("RESTORED:{line}");
    }

    if session.is_on() {
        println!("RESULT:FAIL:still-active");
        1
    } else {
        println!("RESULT:OK");
        0
    }
}

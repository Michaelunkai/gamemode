# Architecture

GameMode is a Cargo workspace of two crates. The library does the risky work; the binary only owns the window, the tray and the pipes.

## Crate map

| Crate | Type | Role |
|-------|------|------|
| gm-core | library (gm_core) | detection, optimisation, snapshot/restore, Win32 wrappers, config |
| gm-gui | binary (gamemode.exe) | tray icon, global hotkey, egui panel, named-pipe IPC, autostart |

## gm-core modules

| File | Responsibility |
|------|----------------|
| lib.rs | crate root, logging init, data dir |
| prelude.rs | Windows string/pointer helpers |
| error.rs | GmError / GmResult |
| config.rs | Config model, TOML load/save, defaults |
| monitor.rs | process sampling, CPU deltas, game detection |
| optimizer.rs | ordered tuning actions and their undo |
| session.rs | state machine, enable/disable, journal |
| snapshot.rs | durable record of every system change |
| win/process.rs | process handles, priority, affinity |
| win/system.rs | power plan, timer resolution, sleep |
| win/services.rs | background service control |
| win/ipc.rs | named-pipe single-instance IPC |
| win/elevation.rs | privilege/environment checks |

## gm-gui modules

| File | Responsibility |
|------|----------------|
| main.rs | entry, arg parsing, wiring, panic safety |
| app.rs | AppState, Command queue, UiSnapshot |
| ui_core.rs | egui control panel (pure view) |
| tray.rs | tray icon, menu, tooltip lifetime |
| hotkeys.rs | global hotkey thread + message loop |
| single_instance.rs | client half of the IPC check |
| autostart.rs | HKCU Run key integration |
| lifecycle.rs | startup recovery, guaranteed shutdown |
| icons.rs | tray icon assets |

## Data flow

Every input source (tray menu, hotkey, panel, second launch) becomes a single Command.
The loop drains the queue once per frame, so machine state changes in exactly one
place and is logged in exactly one place.

Quitting always deactivates the session first, which restores the power plan, timer
resolution, sleep inhibition, priorities and registry values before the process exits.

## Single instance

Only one gamemode.exe owns the machine state. A second launch connects to the running
instance named pipe, sends one command (on/off/toggle/show/quit) and exits. If no server
answers, it becomes the primary instance.

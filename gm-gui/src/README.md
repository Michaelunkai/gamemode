# gm-gui/src

## Overview
Source of the `gamemode` binary (`[[bin]] name = "gamemode"`, entry `main.rs`). It owns the tray icon, the global toggle hotkey, the egui control panel, a named-pipe IPC server and the single-instance check. Every machine change is delegated to `gm-core` (`Session`, `Config`, `win::ipc`, `win::system`).

## Interface
- `main.rs`: `main()`; flags `--headless` / `--console` (headless apply, hold, restore run), `--hold <seconds>` and `--tray` (start hidden). Private `run_gui`, `start_ipc`, `start_hotkeys`, `run_headless`. Headless output lines are prefixed `PHASE:`, `ACTION:`, `RESTORED:`, `RESULT:`. Release builds use `windows_subsystem = "windows"`.
- `app.rs`: `Command` (`Enable`, `Disable`, `Toggle`, `ReloadConfig`, `ShowWindow`, `HideWindow`, `Quit`, `OpenConfig`, `SetAutostart(bool)`); `AppState` (`new(session)`, `send`, `drain`, `apply`, `with_session`, `snapshot`, `uptime`, `config`, `mark_menu_dirty`); `UiSnapshot`; `TICK` (500 ms).
- `ui_core.rs`: `Panel` (`new(state, tray, hotkey, visible)`, implements `eframe::App`); tabs Overview, Tuning, Games, Log are selectable in the header. Settings are written only by the "Save settings" button.
- `tray.rs`: `Tray` (`new`, `sync`, `sync_autostart`); menu ids `ID_TOGGLE`, `ID_SHOW`, `ID_AUTOSTART`, `ID_SETTINGS`, `ID_QUIT`; `command_for(id)` maps all five menu ids to a `Command`; `Panel::tick` polls `tray_icon::menu::MenuEvent` and `TrayIconEvent` (double click shows the window) and sends the result to the queue.
- `hotkeys.rs`: `HotkeyManager` (`start(modifiers, virtual_key)`, `poll`, `stop`, `Drop`); `HotkeyAction::Toggle`; `vk_from_label` (F1-F12 or a single A-Z/0-9 character).
- `single_instance.rs`: `Verb` (`Toggle`, `Enable`, `Disable`, `Show`, `Quit`, `Status`; `as_str`, `to_ipc`); `send_to_running`, `verb_from_args` (takes the arguments without the program name; accepts `on|enable|start`, `off|disable|stop`, `toggle`, `show|open|panel`, `quit|exit`, `status`, with optional `-` or `/` prefix).
- `autostart.rs`: `is_enabled`, `enable`, `disable`, `set(bool)`.
- `lifecycle.rs`: `recover`, `shutdown` (called from `Panel::shutdown`).
- `icons.rs`: `active()`, `idle()`; RGBA 32x32 bytes embedded from `../../assets/gamemode-32.rgba` and `../../assets/tray-off.rgba`, with a grey fallback.

## Invariants
- [main.rs:run_gui] The tray is created once and lives for the process lifetime. If creation fails the app continues without it.
- [main.rs:main] A second launch with a recognised verb is forwarded through `single_instance::send_to_running` and returns without starting its own session when a live instance accepts it.
- [app.rs:AppState::drain/apply] Commands from the tray, hotkey thread, IPC thread and UI are queued on an mpsc channel and applied by the main loop through `AppState::apply`. Poisoned mutexes are recovered with `into_inner`.
- [ui_core.rs:Panel::shutdown, on_exit] Exit goes through `lifecycle::shutdown`, which disables the session (`"process exit"`) and saves the config, once per process.
- [ui_core.rs:Panel::ui] `AppState::want_quit` closes the window; `AppState::want_visible` shows or hides it.
- [lifecycle.rs:shutdown] An `AtomicBool` (`SHUTTING_DOWN`) makes shutdown run its work at most once per process.
- [hotkeys.rs:normalize_modifiers] Only `MOD_ALT`, `MOD_CONTROL`, `MOD_SHIFT` and `MOD_WIN` are kept, and `MOD_NOREPEAT` is added at registration. A failed registration is logged and the manager never fires.
- [autostart.rs:disable] Win32 error 2 (value already absent) counts as success.
- [main.rs:main, autostart.rs:command_line] The autostart entry passes `--tray`; `main` parses it and creates the window hidden (`with_visible(false)`), and `Panel` keeps ticking through `request_repaint_after(TICK)`.

## Side Effects
- Spawns the IPC server thread (`start_ipc`, `gm_core::win::ipc::PipeServer`) and the hotkey thread (`RegisterHotKey` / `UnregisterHotKey` on id `0xA11CE`, message polling every 25 ms).
- Registry: writes and deletes value `GameMode` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, with the command line `"<exe>" --tray`.
- Disk: `Config::load` / `Config::save` (`config.toml`); `Command::OpenConfig` calls `ShellExecuteW` on the config path.
- Windows: tray icon, tooltip and menu updates; eframe window sized 700x520 (minimum 520x360).
- Logging via `gm_core::init_logging`; `eprintln!` for config-load and tray failures.

## Verification
- Run: `cargo test` (workspace root; 61 gm-core + 3 gm-gui tests pass). Unit tests in this crate exist only in `hotkeys.rs` (`function_keys_map_to_virtual_codes`, `letters_and_garbage_are_handled`, `modifiers_are_masked`).
- Parent package: [gm-gui](../README.md)

# gm-gui

## Overview
Cargo package `gm-gui`, which builds the Windows tray application `gamemode.exe` (bin `gamemode`, `src/main.rs`). It is a thin front end over the `gm-core` path dependency. It also depends on `eframe` 0.37 (default features off, `default_fonts` and `glow` on; `glow` supplies the renderer `eframe::run_native` needs), `tray-icon` 0.26, `image` 0.25 (png), `windows-sys`, `serde`, `serde_json`, `log`, `chrono` and `dirs`. Workspace metadata (version, edition, rust-version, license) is inherited from the workspace root.

## Interface
- [`build.rs`](./build.rs): build script using `winres`. It embeds `../assets/gamemode.ico`, version strings (FileDescription, ProductName, OriginalFilename, LegalCopyright, Comments) and an application manifest with `requestedExecutionLevel="asInvoker"` (`uiAccess="false"`), per-monitor-v2 DPI awareness and long-path awareness. Cargo reruns it when `build.rs` or `../assets/gamemode.ico` changes.
- [`src/`](./src/README.md): application modules (tray, hotkeys, IPC command forwarding, control panel, autostart, lifecycle).
- Command line of `gamemode.exe`: `--headless` / `--console` `--hold <seconds>` for a headless apply-and-restore run, and `--tray` to start hidden in the tray. The verbs `on`, `off`, `toggle`, `show`, `quit` and `status` (aliases in [`src/README.md`](./src/README.md)) are forwarded to a running instance.

## Invariants
- [build.rs:main] A failed resource compile only emits a `cargo:warning`; the build still succeeds and the exe falls back to a default icon.
- [build.rs:set_manifest] The process starts unelevated (`asInvoker`).
- Icons are compiled in from `../assets/gamemode-32.rgba` and `../assets/tray-off.rgba` ([src/icons.rs]), so those files must exist at build time alongside `../assets/gamemode.ico`.
- See [src/README.md](./src/README.md) for runtime invariants (single instance, tray lifetime, shutdown restores machine state).

## Side Effects
- Build time: `winres` invokes the Windows resource compiler and links the resulting resource into the binary.
- Runtime effects (registry Run key, config file, hotkey and IPC threads, tray icon) are listed in [src/README.md](./src/README.md).

## Verification
- Run: `cargo test` (workspace root; add `-p gm-gui` to limit it to this package).

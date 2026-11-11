# GameMode — Verification Report

Date: 2026-11-11
Environment: Windows 11, AMD Ryzen 7 9800X3D (8 cores / 16 threads), Admin role.
Toolchain: Rust 1.99.1 stable-x86_64-pc-windows-gnu (direct toolchain binaries; the
rustup proxy is broken in this environment).

## Build

```
cargo build                # debug workspace -> clean
cargo build -p gm-gui --release  -> clean, release binary
```

Release binary: `target/release-local/x86_64-pc-windows-gnu/release/gamemode.exe`
(6.4 MB, optimised, LTO thin, stripped symbols).

## Functional checks

| Check | Result | Evidence |
|---|---|---|
| `cargo build` workspace | PASS | `Finished` with no errors |
| Release build | PASS | 6.4 MB `gamemode.exe` |
| Headless `--headless --hold 3` | PASS | `RESULT:OK` |
| All actions applied (priority, power, timer, sleep) | PASS | `ACTION:…✔:` for each |
| Full restore on deactivate | PASS | `RESTORED:` for every action |
| Tray mode (`--tray`) runs background | PASS | tray icon visible, 1 process |
| Single-instance IPC | PASS | 2nd `--tray` launch forwards ping, exits |
| `quit` stops everything | PASS | 0 `gamemode.exe` after quit |

## Headless run output (sanitised)

```
PHASE:activated
ACTION:Privileges:✔:6 debug privilege(s) enabled
ACTION:Power plan:✔:switched to High performance
ACTION:Timer resolution:✔:raised to 0.50 ms
ACTION:Sleep inhibition:✔:system and display will not sleep
ACTION:GameMode:✔:activated (no game yet: waiting for a match)
PHASE:deactivated
RESTORED:sleep inhibition cleared
RESTORED:timer resolution restored
RESTORED:processor tuning restored
RESTORED:power scheme restored
RESTORED:GameMode deactivated
RESULT:OK
```

## Notes

* The GNU/MinGW toolchain is used (no MSVC). Linker: `gcc.exe` + `rust-lld`.
* `CARGO_TARGET_DIR` points at a shared cache (`F:\backup\rust\target`); for reproducible
  isolated builds use `--target-dir ./target/release-local` as done for the release build.
* `winres` (build-dependency) embeds `assets/gamemode.ico`, the `asInvoker` manifest and
  version resources into `gamemode.exe`.
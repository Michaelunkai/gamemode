# gm-core

## Overview
Workspace member crate `gm-core` (library `gm_core`, `src/lib.rs`): GameMode's detection, optimisation and exact-restore engine for Windows. Dependencies (Cargo.toml): `windows-sys`, `serde`, `serde_json`, `log`, `toml`, `chrono`, `dirs`; version, edition, rust-version and license come from the workspace.

## Interface
- Public API and module contracts: [src](./src/README.md) (re-exports `Config`, `Monitor`, `Optimizer`, `Session`, `Snapshot`, `GmError`/`GmResult`, plus `init_logging`, `is_elevated`, `data_dir`).
- Windows API wrappers: [src/win](./src/win/README.md).

## Invariants
- Every system change made while active is journaled in a `Snapshot` so it can be undone (see [src](./src/README.md)).
- Config values are clamped by `Config::validate` in `Config::load`.

## Side Effects
- Reads/writes `%APPDATA%\GameMode\` (`config.toml`, `snapshot.json`, `gamemode.log`).
- Modifies machine state while a session is active (power plan, services, process priority/affinity, timer resolution, registry); see [src/win](./src/win/README.md).

## Verification
- Run: `cargo test` (workspace root)

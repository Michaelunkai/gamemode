# gm-core/src

## Overview
Source of the `gm_core` library: detection, optimisation and exact restore for Windows. Layers (per `lib.rs`): `win` wrappers, `config`, `monitor`, `optimizer`, `session`, `snapshot`. `Session::tick()` is the single entry the GUI calls on a timer, keeping the GUI a pure view layer.

## Interface
- `lib.rs`: modules `config`, `error`, `monitor`, `optimizer`, `prelude`, `session`, `snapshot`, `win`, and `#[cfg(test)] testlock`. Re-exports `BoostLevel`, `Config`, `DetectMode`, `GameEntry`, `PowerPlan`, `GmError`, `GmResult`, `GameDetection`, `Monitor`, `ProcessSample`, `ActionReport`, `ActionStatus`, `Optimizer`, `Session`, `SessionState`, `Snapshot`. Also `VERSION`, `PRODUCT`, `init_logging(verbose)`, `is_elevated`, `data_dir`.
- `config.rs`: `DetectMode` (`ListOnly`, `ForegroundOnly`, `Smart`), `PowerPlan` (`Keep`, `Balanced`, `HighPerformance`, `UltimatePerformance`), `BoostLevel` (`Off`, `Balanced`, `Aggressive`, `Extreme`; `score`), `GameEntry`, `Config` (`load`, `save`, `validate`, `config_path`, `data_dir`, `ensure_dirs`, `is_whitelisted`, `is_game_name`, `add_game`, `remove_game`, `hotkey_label`), `default_games`, `default_whitelist`, `default_services`.
- `error.rs`: `GmError` (constructors `win32`, `hr`, `other`, `cfg`), `GmResult<T>`, `win32_message`.
- `monitor.rs`: `ProcessSample` (`ram_mb`), `GameDetection` (`found`), `Monitor` (`new`, `sample`, `system_cpu`, `detect`, `note_launcher_children`, `is_launcher_child`), `quick_probe`.
- `optimizer.rs`: `ActionStatus`, `ActionReport`, `OptimizerStats`, `Optimizer` (`new`, `is_primed`, `reserved_mask`, `background_mask`, `activate`, `deactivate`, `refresh_stats`, `rebind_game`), `priority_class_label`, `suggested_reserved_cores`.
- `session.rs`: `SessionState` (`Idle`, `Arming`, `Active`, `Draining`), `LogLine`, `LogLevel`, `Trigger`, `Session` (`new`, `recover`, `enable`, `disable`, `toggle`, `tick`, `apply_config`, `reload_config`, `phase_elapsed`, `phase_remaining`, `active_duration`, `log_path`, `tooltip`, `is_on`, `push_log`), `TickResult`, `SystemInfo`, `system_info`, `cpu_brand`, `physical_core_count`, `windows_build`.
- `snapshot.rs`: `Change` (`PowerScheme`, `ProcessorTuning`, `Registry`, `Service`, `ProcessPriority`, `ProcessAffinity`, `TimerResolution`, `SleepInhibition`, `MemoryTrim`, `Note`), `Snapshot` (`path`, `begin`, `push`, `flush`, `load`, `is_pending`, `discard`, `restore`), `parse_guid`.
- `prelude.rs`: `Wide`, `Ansi`, `Handle`, string helpers (`wide`, `from_wide`, `from_pwstr`), `last_error`, `set_last_error`, `ok`, `read_text`, `exe_name`, `expand_env`, `csidl` constants, `known_folder`, `pct_round`, `clamp_i64`, `CvPtr`, `clear_error`, `win32_message`, `proc_address`.
- `testlock.rs`: `guard()` (test-only mutex).
- Subdirectory: [win](./win/README.md) - Win32 wrappers.

## Invariants
- [config.rs:Config::validate] `version` forced to 1; `activation_delay_secs` <= 120, `deactivation_grace_secs` <= 600, `cpu_detect_threshold` in 1.0..=99.0, `poll_interval_ms` in 250..=15000, `reserved_core_count` <= 64; game and whitelist exe names lower-cased; whitelist sorted, deduplicated and always contains the current executable name.
- [snapshot.rs] Every system-modifying optimiser action journals prior state as a `Change`; `restore` replays it back to front.
- [session.rs:push_log] In-memory log is capped at 256 lines.
- [optimizer.rs] Game is raised before background work is demoted (module doc).
- [testlock.rs] Tests altering power scheme, timer resolution, sleep inhibition or process priority must hold `testlock::guard()`.

## Side Effects
- Disk: `%APPDATA%\GameMode\config.toml` (atomic save via temp file), `snapshot.json` (survives crashes; next start offers restore), `gamemode.log` (`init_logging`; also stderr when verbose).
- System: see [win](./win/README.md) (power plan, services, priorities, affinity, timer, registry, sleep inhibition).
- Process table sampling via `Monitor::sample`.

## Verification
- Run: `cargo test`

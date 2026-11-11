# win

## Overview
Thin, safe wrappers over the Win32/NT APIs GameMode needs (`mod.rs`). Each submodule owns one concern so the optimiser above stays free of raw handles and access masks. Failures are converted into `GmError`.

## Interface
- `elevation.rs`: `is_elevated`, `elevation_type`, `is_admin_member`, `enable_privilege`, `enable_all_privileges`, `relaunch_elevated`, `current_token`.
- `ipc.rs`: `pipe_name`, `IpcCommand` (`Toggle`, `Enable`, `Disable`, `Show`, `Hide`, `Quit`, `Status`, `Reload`; `as_str`, `parse`), `send_command`, `is_instance_running`, `PipeServer` (`new`, `stop_flag`, `shutdown`, `accept_command`), `read_line`, `write_line`, `resolve`.
- `power.rs`: `GUID_BALANCED`, `GUID_HIGH_PERFORMANCE`, `GUID_POWER_SAVER`, `GUID_ULTIMATE`, `guid_to_string`, `active_scheme`, `set_active_scheme`, `plan_guid`, `read_processor_setting`, `write_processor_setting`, `ProcessorTuning`, `capture_processor_tuning`, `apply_max_performance`, `restore_processor_tuning`, `ensure_ultimate_scheme`, `guid_eq`, `scheme_name`.
- `process.rs`: `ACCESS_FULL_CONTROL`, `ACCESS_QUERY`, `ProcessInfo`, `ProcessCounters`, `snapshot_processes`, `open_process`, `process_path`, `process_counters`, priority/affinity/boost setters (`get_priority_class`, `set_priority_class`, `set_high_priority`, `set_idle_priority`, `set_below_normal_priority`, `set_priority_boost`, `get_affinity`, `set_affinity`), `reserve_mask`, `background_mask`, `logical_cpu_count`, memory (`set_memory_priority`, `set_power_throttling`, `trim_working_set`, `set_working_set_limits`, `set_working_set_bytes`, `total_ram_bytes`, `available_ram_bytes`), threads (`threads_of`, `set_thread_priority`, `boost_all_threads`), `is_process_alive`, `terminate_process`, `SystemTimes`, `system_times`, `current_pid`, `proc_address`, `sleep_ms`.
- `services.rs`: `ServiceState` (`label`, `is_running`), `service_state`, `stop_service`, `start_service`, `service_exists`.
- `system.rs`: timer resolution (`query_timer_resolution`, `set_timer_resolution`, `clear_timer_resolution`, `timer_ms`), sleep (`prevent_sleep`, `allow_sleep`, `sleep_inhibited`), foreground/window (`is_fullscreen_app_active`, `notification_state`, `foreground_window`, `window_pid`, `window_title`, `is_foreground_process`), registry (`RegValue`, `reg_read`, `reg_write_string`, `reg_write_dword`, `reg_delete_value`, `reg_write_raw`, `RegChange::capture`/`restore`), `apply_gaming_tweaks`, `MmcssRegistration::register`, `audio_engine_pid`, `set_foreground_priority_boost`, `open_in_editor`.

## Invariants
- [process.rs] Each function performs one documented Win32 call and does not keep a handle open longer than needed.
- [services.rs] Only services listed in `services_to_stop` are touched; state is recorded before a stop so it can be restarted on deactivation.
- [power.rs] The previously active scheme GUID is restored exactly (GUID-based PowrProf API).
- [system.rs] `NtSetTimerResolution`/`NtQueryTimerResolution` are resolved at runtime from `ntdll.dll` via `GetProcAddress`, not linked statically.
- [ipc.rs] The pipe security descriptor admits any authenticated user, so elevated and unelevated instances can talk; the first instance owns the pipe, later launches only send a command.

## Side Effects
- Changes process priority, affinity, working set, memory priority, power throttling and thread priorities; can terminate processes (`terminate_process`).
- Stops/starts Windows services; switches the active power scheme and processor settings; may create the Ultimate scheme (`ensure_ultimate_scheme`).
- Sets the system timer resolution, sleep inhibition and registry values (`reg_write_*`, `reg_delete_value`, `apply_gaming_tweaks`).
- Creates and listens on a named pipe; `relaunch_elevated` triggers a UAC prompt; `enable_privilege` adjusts the process token; `open_in_editor` launches an external program.

## Verification
- Run: `cargo test` (from the workspace root; tests touching machine-global state take `crate::testlock::guard`).

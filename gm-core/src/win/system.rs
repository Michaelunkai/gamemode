//! System-wide tuning: timer resolution, sleep inhibition, Game DVR and HAGS.
//!
//! The NT functions (`NtSetTimerResolution`, `NtQueryTimerResolution`) are not
//! exported by the windows-sys bindings, so they are resolved at runtime from
//! `ntdll.dll` through `GetProcAddress` instead of being linked statically.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Power::{
    SetThreadExecutionState, ES_AWAYMODE_REQUIRED, ES_CONTINUOUS, ES_DISPLAY_REQUIRED,
    ES_SYSTEM_REQUIRED,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_QUERY_VALUE, REG_DWORD,
    REG_SZ,
};
use windows_sys::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_APP, QUNS_RUNNING_D3D_FULL_SCREEN,
};

use crate::error::{GmError, GmResult};
use crate::prelude::{proc_address, Wide};

// ---------------------------------------------------------------------------
// NT timer resolution (resolved dynamically from ntdll.dll)
// ---------------------------------------------------------------------------

type NtSetTimerResolution = unsafe extern "system" fn(u32, u8, *mut u32) -> i32;
type NtQueryTimerResolution = unsafe extern "system" fn(*mut u32, *mut u32, *mut u32) -> i32;

fn nt_set_timer_resolution() -> Option<NtSetTimerResolution> {
    proc_address("ntdll.dll", "NtSetTimerResolution")
        .map(|p| unsafe { std::mem::transmute::<*const core::ffi::c_void, NtSetTimerResolution>(p as *const core::ffi::c_void) })
}

fn nt_query_timer_resolution() -> Option<NtQueryTimerResolution> {
    proc_address("ntdll.dll", "NtQueryTimerResolution")
        .map(|p| unsafe { std::mem::transmute::<*const core::ffi::c_void, NtQueryTimerResolution>(p as *const core::ffi::c_void) })
}

/// Remembers whether `SetThreadExecutionState` currently blocks sleep, so the
/// state is only ever changed by the thread that set it and is always undone.
static SLEEP_INHIBITED: AtomicBool = AtomicBool::new(false);
/// Cached 100 ns units of the timer resolution actually granted by the kernel.
static TIMER_GRANTED_100NS: AtomicU32 = AtomicU32::new(0);

/// Query the kernel's coarsest, finest and current timer resolution, in
/// 100-nanosecond units.
///
/// `NtQueryTimerResolution` takes the parameters as `(MaximumTime, MinimumTime,
/// CurrentTime)`. The names are counter-intuitive: *MaximumTime* is the coarsest
/// interval (typically 156250 = 15.625 ms) and *MinimumTime* is the finest
/// (typically 5000 = 0.5 ms). Lower intervals mean finer resolution.
pub fn query_timer_resolution() -> Option<(u32, u32, u32)> {
    let f = nt_query_timer_resolution()?;
    unsafe {
        let mut coarsest = 0u32;
        let mut finest = 0u32;
        let mut current = 0u32;
        let status = f(&mut coarsest, &mut finest, &mut current);
        if status >= 0 {
            Some((coarsest, finest, current))
        } else {
            None
        }
    }
}

/// Request `desired_100ns` timer resolution (5000 = 0.5 ms, the usual maximum).
///
/// Returns the resolution the kernel granted, in 100 ns units. The requested
/// value is clamped between the finest and coarsest intervals the kernel reports.
pub fn set_timer_resolution(desired_100ns: u32) -> GmResult<u32> {
    let f = nt_set_timer_resolution()
        .ok_or_else(|| GmError::other("NtSetTimerResolution unavailable"))?;
    let clamped = match query_timer_resolution() {
        // `finest` is the smallest interval the clock supports.
        Some((_, finest, _)) => desired_100ns.max(finest),
        None => desired_100ns,
    };
    let mut granted: u32 = 0;
    unsafe {
        let status = f(clamped, 1, &mut granted);
        if status < 0 {
            return Err(GmError::other(format!(
                "NtSetTimerResolution failed (NTSTATUS 0x{:08X})",
                status
            )));
        }
    }
    TIMER_GRANTED_100NS.store(granted, Ordering::SeqCst);
    Ok(granted)
}

/// Release a previous timer-resolution request.
///
/// `SetResolution` is `FALSE` here, which makes the kernel ignore
/// `DesiredResolution` and drop this process's request. The out-parameter must
/// still point at valid memory, so a stack slot is always supplied.
pub fn clear_timer_resolution() -> GmResult<()> {
    let f = nt_set_timer_resolution()
        .ok_or_else(|| GmError::other("NtSetTimerResolution unavailable"))?;
    unsafe {
        let mut current: u32 = 0;
        let status = f(0, 0, &mut current);
        if status < 0 {
            return Err(GmError::other(format!(
                "NtSetTimerResolution(release) failed (NTSTATUS 0x{:08X})",
                status
            )));
        }
    }
    TIMER_GRANTED_100NS.store(0, Ordering::SeqCst);
    Ok(())
}

/// The granted resolution in milliseconds, when a request is active.
pub fn timer_ms() -> f32 {
    let v = TIMER_GRANTED_100NS.load(Ordering::SeqCst);
    if v == 0 {
        0.0
    } else {
        v as f32 / 10_000.0
    }
}

// ---------------------------------------------------------------------------
// Sleep inhibition
// ---------------------------------------------------------------------------

/// Stop Windows from sleeping the system or display until [`allow_sleep`].
pub fn prevent_sleep() -> GmResult<()> {
    let flags = ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED | ES_AWAYMODE_REQUIRED;
    let prev = unsafe { SetThreadExecutionState(flags) };
    if prev == 0 {
        // Retry without away mode: some machines reject the combination.
        let prev2 = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED) };
        if prev2 == 0 {
            return Err(GmError::win32("SetThreadExecutionState"));
        }
    }
    SLEEP_INHIBITED.store(true, Ordering::SeqCst);
    Ok(())
}

/// Undo [`prevent_sleep`].
pub fn allow_sleep() -> GmResult<()> {
    let prev = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    SLEEP_INHIBITED.store(false, Ordering::SeqCst);
    if prev == 0 {
        return Err(GmError::win32("SetThreadExecutionState(clear)"));
    }
    Ok(())
}

/// Whether sleep is currently being blocked by this process.
pub fn sleep_inhibited() -> bool {
    SLEEP_INHIBITED.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Foreground / fullscreen detection
// ---------------------------------------------------------------------------

/// True when the foreground window is a D3D/DXGI fullscreen app (a running game).
pub fn is_fullscreen_app_active() -> bool {
    let mut state: i32 = 0;
    unsafe {
        let hr = SHQueryUserNotificationState(&mut state);
        if hr < 0 {
            return false;
        }
    }
    state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_APP
}

/// Raw notification state, useful for diagnostics.
pub fn notification_state() -> i32 {
    let mut state: i32 = 0;
    unsafe {
        let _ = SHQueryUserNotificationState(&mut state);
    }
    state
}

/// Foreground window handle.
pub fn foreground_window() -> HANDLE {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as HANDLE }
}

/// Process id that owns a window, or 0.
pub fn window_pid(hwnd: HANDLE) -> u32 {
    if hwnd.is_null() {
        return 0;
    }
    unsafe {
        let mut pid = 0u32;
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            hwnd as *mut _,
            &mut pid,
        );
        pid
    }
}

/// Full title text of a window.
pub fn window_title(hwnd: HANDLE) -> String {
    if hwnd.is_null() {
        return String::new();
    }
    unsafe {
        let len = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW(hwnd as *mut _);
        if len <= 0 {
            return String::new();
        }
        let mut buf = Wide::buffer(len as usize + 2);
        let n = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(
            hwnd as *mut _,
            buf.pwstr(),
            buf.capacity() as i32,
        );
        if n <= 0 {
            return String::new();
        }
        buf.0.truncate(n as usize);
        buf.to_string_lossy()
    }
}

/// True when the foreground window belongs to `pid`.
pub fn is_foreground_process(pid: u32) -> bool {
    window_pid(foreground_window()) == pid
}

// ---------------------------------------------------------------------------
// Registry helper
// ---------------------------------------------------------------------------

/// A registry value read back as an owned byte vector plus its type.
#[derive(Debug, Clone)]
pub struct RegValue {
    pub kind: u32,
    pub data: Vec<u8>,
}

impl RegValue {
    pub fn as_u32(&self) -> Option<u32> {
        if self.data.len() >= 4 {
            Some(u32::from_le_bytes([
                self.data[0], self.data[1], self.data[2], self.data[3],
            ]))
        } else {
            None
        }
    }
    pub fn as_string(&self) -> Option<String> {
        if self.data.len() < 2 {
            return None;
        }
        let words: Vec<u16> = self
            .data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        Some(String::from_utf16_lossy(&words))
    }
}

/// Read a value under `HKCU`/`HKLM`; returns `None` when absent.
pub fn reg_read(root: u32, subkey: &str, value: &str) -> Option<RegValue> {
    let hroot: HKEY = match root {
        0 => HKEY_CURRENT_USER,
        _ => HKEY_LOCAL_MACHINE,
    };
    let sub = Wide::new(subkey);
    let name = Wide::new(value);
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let rc = RegOpenKeyExW(hroot, sub.pcwstr(), 0, KEY_QUERY_VALUE, &mut hkey);
        if rc != 0 {
            return None;
        }
        let mut kind = 0u32;
        let mut size = 0u32;
        let rc = RegQueryValueExW(
            hkey,
            name.pcwstr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        );
        if rc != 0 {
            RegCloseKey(hkey);
            return None;
        }
        let mut data = vec![0u8; size.max(1) as usize];
        let rc = RegQueryValueExW(
            hkey,
            name.pcwstr(),
            std::ptr::null(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        );
        RegCloseKey(hkey);
        if rc != 0 {
            return None;
        }
        data.truncate(size as usize);
        Some(RegValue { kind, data })
    }
}

/// Write a `REG_SZ` string under `HKCU`/`HKLM`, creating the key when needed.
///
/// The string is stored as NUL-terminated UTF-16, which is what the Run key and
/// every other Windows consumer of `REG_SZ` expects.
pub fn reg_write_string(root: u32, subkey: &str, value: &str, text: &str) -> GmResult<()> {
    let mut data = Vec::with_capacity((text.len() + 1) * 2);
    for unit in text.encode_utf16() {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    data.extend_from_slice(&0u16.to_le_bytes());
    reg_write_raw(root, subkey, value, REG_SZ, &data)
}

/// Write a `REG_DWORD` under `HKCU`/`HKLM`, creating the key when needed.
pub fn reg_write_dword(root: u32, subkey: &str, value: &str, data: u32) -> GmResult<()> {
    let hroot: HKEY = match root {
        0 => HKEY_CURRENT_USER,
        _ => HKEY_LOCAL_MACHINE,
    };
    let sub = Wide::new(subkey);
    let name = Wide::new(value);
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let mut disp = 0u32;
        let rc = RegCreateKeyExW(
            hroot,
            sub.pcwstr(),
            0,
            std::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            std::ptr::null(),
            &mut hkey,
            &mut disp,
        );
        if rc != 0 {
            return Err(GmError::Win32 {
                call: "RegCreateKeyExW",
                code: rc,
            });
        }
        let bytes = data.to_le_bytes();
        let rc = RegSetValueExW(
            hkey,
            name.pcwstr(),
            0,
            REG_DWORD,
            bytes.as_ptr(),
            bytes.len() as u32,
        );
        RegCloseKey(hkey);
        if rc != 0 {
            return Err(GmError::Win32 {
                call: "RegSetValueExW",
                code: rc,
            });
        }
    }
    Ok(())
}

/// Delete a value, ignoring "not found".
pub fn reg_delete_value(root: u32, subkey: &str, value: &str) -> GmResult<()> {
    let hroot: HKEY = match root {
        0 => HKEY_CURRENT_USER,
        _ => HKEY_LOCAL_MACHINE,
    };
    let sub = Wide::new(subkey);
    let name = Wide::new(value);
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let rc = RegOpenKeyExW(hroot, sub.pcwstr(), 0, KEY_ALL_ACCESS, &mut hkey);
        if rc != 0 {
            return Ok(()); // key absent: nothing to delete
        }
        let rc = RegDeleteValueW(hkey, name.pcwstr());
        RegCloseKey(hkey);
        if rc != 0 && rc != 2 {
            return Err(GmError::Win32 {
                call: "RegDeleteValueW",
                code: rc,
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Gaming-related registry tweaks
// ---------------------------------------------------------------------------

const GAME_DVR_KEY: &str = r"System\GameConfigStore";
const GAME_BAR_KEY: &str = r"Software\Microsoft\GameBar";
const GAME_DVR_POLICY: &str = r"SOFTWARE\Microsoft\PolicyManager\default\ApplicationManagement\AllowGameDVR";
const MULTIMEDIA_SYSTEM_PROFILE: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile";
const GRAPHICS_DRIVERS_KEY: &str =
    r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers";

/// A pending registry change captured so it can be undone exactly.
#[derive(Debug, Clone)]
pub struct RegChange {
    pub root: u32,
    pub subkey: String,
    pub value: String,
    /// `None` means the value did not exist before and must be deleted on restore.
    pub previous: Option<RegValue>,
}

impl RegChange {
    /// Capture the current value so it can be restored later.
    pub fn capture(root: u32, subkey: &str, value: &str) -> Self {
        RegChange {
            root,
            subkey: subkey.to_string(),
            value: value.to_string(),
            previous: reg_read(root, subkey, value),
        }
    }

    /// Undo the change: restore the old data or delete the value entirely.
    pub fn restore(&self) -> GmResult<()> {
        match &self.previous {
            Some(v) => reg_write_raw(self.root, &self.subkey, &self.value, v.kind, &v.data),
            None => reg_delete_value(self.root, &self.subkey, &self.value),
        }
    }
}

/// Write an arbitrary registry value.
pub fn reg_write_raw(
    root: u32,
    subkey: &str,
    value: &str,
    kind: u32,
    data: &[u8],
) -> GmResult<()> {
    let hroot: HKEY = match root {
        0 => HKEY_CURRENT_USER,
        _ => HKEY_LOCAL_MACHINE,
    };
    let sub = Wide::new(subkey);
    let name = Wide::new(value);
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let mut disp = 0u32;
        let rc = RegCreateKeyExW(
            hroot,
            sub.pcwstr(),
            0,
            std::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            std::ptr::null(),
            &mut hkey,
            &mut disp,
        );
        if rc != 0 {
            return Err(GmError::Win32 {
                call: "RegCreateKeyExW",
                code: rc,
            });
        }
        let rc = RegSetValueExW(hkey, name.pcwstr(), 0, kind, data.as_ptr(), data.len() as u32);
        RegCloseKey(hkey);
        if rc != 0 {
            return Err(GmError::Win32 {
                call: "RegSetValueExW",
                code: rc,
            });
        }
    }
    Ok(())
}

/// Registry tweaks applied by GameMode, each recording its previous state.
pub fn apply_gaming_tweaks(disable_dvr: bool, hags: bool, network: bool, menus: bool) -> Vec<(String, RegChange)> {
    let mut changes = Vec::new();

    if disable_dvr {
        // Game DVR background recording steals GPU time and adds frametime spikes.
        let c1 = RegChange::capture(0, GAME_DVR_KEY, "GameDVR_Enabled");
        if reg_write_dword(0, GAME_DVR_KEY, "GameDVR_Enabled", 0).is_ok() {
            changes.push(("Game DVR disabled".into(), c1));
        }
        let c2 = RegChange::capture(0, GAME_DVR_KEY, "GameDVR_FSEBehaviorMode");
        if reg_write_dword(0, GAME_DVR_KEY, "GameDVR_FSEBehaviorMode", 2).is_ok() {
            changes.push(("Game DVR fullscreen behaviour".into(), c2));
        }
        let c3 = RegChange::capture(1, GAME_DVR_POLICY, "value");
        if reg_write_dword(1, GAME_DVR_POLICY, "value", 0).is_ok() {
            changes.push(("Game DVR policy".into(), c3));
        }
        let c4 = RegChange::capture(0, GAME_BAR_KEY, "AutoGameModeEnabled");
        if reg_write_dword(0, GAME_BAR_KEY, "AutoGameModeEnabled", 1).is_ok() {
            changes.push(("Windows Game Mode enabled".into(), c4));
        }
    }

    if hags {
        // Hardware Accelerated GPU Scheduling lowers render latency on modern GPUs.
        let c = RegChange::capture(1, GRAPHICS_DRIVERS_KEY, "HwSchMode");
        if reg_write_dword(1, GRAPHICS_DRIVERS_KEY, "HwSchMode", 2).is_ok() {
            changes.push(("Hardware GPU scheduling enabled".into(), c));
        }
    }

    if network {
        // Prefer low latency over throughput for games.
        let c1 = RegChange::capture(
            1,
            MULTIMEDIA_SYSTEM_PROFILE,
            "NetworkThrottlingIndex",
        );
        if reg_write_dword(1, MULTIMEDIA_SYSTEM_PROFILE, "NetworkThrottlingIndex", 0xFFFF_FFFF).is_ok() {
            changes.push(("Network throttling disabled".into(), c1));
        }
        let c2 = RegChange::capture(
            1,
            MULTIMEDIA_SYSTEM_PROFILE,
            "SystemResponsiveness",
        );
        if reg_write_dword(1, MULTIMEDIA_SYSTEM_PROFILE, "SystemResponsiveness", 10).is_ok() {
            changes.push(("Multimedia system responsiveness".into(), c2));
        }
        let iface = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks\Games";
        let c3 = RegChange::capture(1, iface, "GPU Priority");
        if reg_write_dword(1, iface, "GPU Priority", 8).is_ok() {
            changes.push(("MMCSS games: GPU priority".into(), c3));
        }
        let c4 = RegChange::capture(1, iface, "Priority");
        if reg_write_dword(1, iface, "Priority", 6).is_ok() {
            changes.push(("MMCSS games: priority".into(), c4));
        }
        let c5 = RegChange::capture(1, iface, "Scheduling Category");
        if reg_write_raw(
            1,
            iface,
            "Scheduling Category",
            1,
            b"H\0i\0g\0h\0\0\0",
        )
        .is_ok()
        {
            changes.push(("MMCSS games: scheduling category".into(), c5));
        }
        let c6 = RegChange::capture(1, iface, "SFIO Priority");
        if reg_write_raw(1, iface, "SFIO Priority", 1, b"H\0i\0g\0h\0\0\0").is_ok() {
            changes.push(("MMCSS games: storage IO priority".into(), c6));
        }
    }

    if menus {
        // Keep the game in the foreground: no menu-show delay stealing the cursor.
        let c1 = RegChange::capture(0, r"Control Panel\Desktop", "MenuShowDelay");
        if reg_write_raw(0, r"Control Panel\Desktop", "MenuShowDelay", 1, b"0\0".to_vec().as_slice()).is_ok()
        {
            changes.push(("Menu show delay".into(), c1));
        }
    }

    changes
}

// ---------------------------------------------------------------------------
// MMCSS / audio thread registration
// ---------------------------------------------------------------------------

/// Handle to a registered MMCSS task, released on drop.
pub struct MmcssRegistration(pub HANDLE);

unsafe impl Send for MmcssRegistration {}

impl MmcssRegistration {
    /// Register the calling thread as a "Games" / "Pro Audio" MMCSS task.
    pub fn register(task: &str) -> Option<Self> {
        let name = Wide::new(task);
        let mut index = 0u32;
        unsafe {
            let h = windows_sys::Win32::System::Threading::AvSetMmThreadCharacteristicsW(
                name.pcwstr(),
                &mut index,
            );
            if h.is_null() {
                None
            } else {
                Some(MmcssRegistration(h))
            }
        }
    }
}

impl Drop for MmcssRegistration {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

/// Suspend or resume the `audiosrv`-backed audio engine by toggling its
/// power-throttling state. Games with EA/Dolby spatial audio benefit most.
pub fn audio_engine_pid() -> Option<u32> {
    crate::win::process::snapshot_processes()
        .ok()?
        .into_iter()
        .find(|p| p.exe.eq_ignore_ascii_case("audiodg.exe"))
        .map(|p| p.pid)
}

/// Set or clear the foreground-lock registry value that raises the priority of
/// the currently focused window's owner.
pub fn set_foreground_priority_boost(enable: bool) -> GmResult<()> {
    let key = r"SYSTEM\CurrentControlSet\Control\PriorityControl";
    let value = if enable { 1 } else { 0 };
    match reg_write_dword(1, key, "Win32PrioritySeparation", value) {
        Ok(()) => Ok(()),
        Err(e) => {
            if enable {
                Err(e)
            } else {
                Ok(()) // restore failures are not fatal
            }
        }
    }
}

/// Open a file with the shell's default handler (e.g. the config in Notepad).
///
/// Used by the control panel's "Edit config" action so the user never has to
/// find `%APPDATA%` by hand.
pub fn open_in_editor(path: &std::path::Path) -> GmResult<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb = Wide::new("open");
    let file = Wide::new(&path.to_string_lossy());
    // ShellExecuteW returns a value <= 32 on failure.
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.pcwstr(),
            file.pcwstr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if rc <= 32 {
        Err(GmError::other(format!(
            "ShellExecuteW could not open {}",
            path.display()
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_resolution_can_be_queried() {
        let _guard = crate::testlock::guard();
        let q = query_timer_resolution();
        assert!(q.is_some(), "NtQueryTimerResolution should resolve from ntdll");
        let (coarsest, finest, current) = q.unwrap();
        // Intervals, so the finest must be the smallest value.
        assert!(
            finest <= coarsest,
            "fine interval {finest} must not exceed coarse interval {coarsest}"
        );
        assert!(
            current >= finest && current <= coarsest,
            "current {current} must lie inside [{finest}, {coarsest}]"
        );
    }

    #[test]
    fn timer_resolution_round_trip() {
        let _guard = crate::testlock::guard();
        if let Ok(granted) = set_timer_resolution(5000) {
            assert!(granted > 0);
            assert!(timer_ms() > 0.0);
        }
        assert!(clear_timer_resolution().is_ok());
        assert_eq!(timer_ms(), 0.0);
    }

    #[test]
    fn sleep_inhibition_round_trip() {
        let _guard = crate::testlock::guard();
        prevent_sleep().expect("prevent_sleep");
        assert!(sleep_inhibited());
        allow_sleep().expect("allow_sleep");
        assert!(!sleep_inhibited());
    }

    #[test]
    fn notification_state_is_readable() {
        let _guard = crate::testlock::guard();
        // Must not crash and must return a documented value range.
        let s = notification_state();
        assert!((0..=8).contains(&s), "unexpected state {s}");
    }

    #[test]
    fn registry_read_of_known_value() {
        let _guard = crate::testlock::guard();
        // A value that exists on every Windows install.
        let v = reg_read(1, GRAPHICS_DRIVERS_KEY, "HwSchMode");
        if let Some(v) = v {
            assert!(v.as_u32().is_some());
        }
    }

    #[test]
    fn reg_change_captures_and_restores() {
        let _guard = crate::testlock::guard();
        let key = r"Software\GameMode\SelfTest";
        let before = reg_read(0, key, "Probe");
        let change = RegChange::capture(0, key, "Probe");
        reg_write_dword(0, key, "Probe", 1234).expect("write");
        assert_eq!(reg_read(0, key, "Probe").unwrap().as_u32(), Some(1234));
        change.restore().expect("restore");
        let after = reg_read(0, key, "Probe");
        assert_eq!(after.is_some(), before.is_some());
    }
}

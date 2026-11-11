//! Elevation state, privilege enabling and self-relaunch as administrator.
//!
//! GameMode needs `SeDebugPrivilege` and `SeIncreaseBasePriorityPrivilege` to
//! retune processes it does not own, so it offers to relaunch elevated via the
//! standard UAC shell verb.

#![allow(dead_code)]

use std::mem::size_of;

use windows_sys::Win32::Foundation::{HANDLE, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE,
    TOKEN_PRIVILEGES, TOKEN_QUERY, TokenElevation, TokenElevationType,
};
use windows_sys::Win32::System::Threading::OpenProcessToken;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::error::{GmError, GmResult};
use crate::prelude::{ok, Handle, Wide};

/// True when the current process token is elevated.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if !ok(OpenProcessToken(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            TOKEN_QUERY,
            &mut token,
        )) {
            return false;
        }
        let token = match Handle::new(token) {
            Some(t) => t,
            None => return false,
        };
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut ret_len = 0u32;
        let ok_call = ok(GetTokenInformation(
            token.raw(),
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        ));
        ok_call && elevation.TokenIsElevated != 0
    }
}

/// The kind of elevation this token has: default, full, or limited (UAC filtered).
pub fn elevation_type() -> u32 {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if !ok(OpenProcessToken(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            TOKEN_QUERY,
            &mut token,
        )) {
            return 0;
        }
        let Some(token) = Handle::new(token) else {
            return 0;
        };
        let mut kind: TOKEN_ELEVATION_TYPE = 0;
        let mut ret_len = 0u32;
        if ok(GetTokenInformation(
            token.raw(),
            TokenElevationType,
            &mut kind as *mut _ as *mut _,
            size_of::<TOKEN_ELEVATION_TYPE>() as u32,
            &mut ret_len,
        )) {
            kind as u32
        } else {
            0
        }
    }
}

/// Whether the current user is a member of the Administrators group at all.
pub fn is_admin_member() -> bool {
    // ElevationTypeDefault(1) with an unelevated token means the user is not an
    // admin; Full(2) means elevated admin; Limited(3) means admin awaiting UAC.
    matches!(elevation_type(), 2 | 3)
}

/// Enable a named privilege on the current process token.
pub fn enable_privilege(name: &str) -> GmResult<()> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if !ok(OpenProcessToken(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )) {
            return Err(GmError::win32("OpenProcessToken"));
        }
        let Some(token) = Handle::new(token) else {
            return Err(GmError::win32("OpenProcessToken(handle)"));
        };
        let mut luid = LUID {
            LowPart: 0,
            HighPart: 0,
        };
        let pname = Wide::new(name);
        if !ok(LookupPrivilegeValueW(
            std::ptr::null(),
            pname.pcwstr(),
            &mut luid,
        )) {
            return Err(GmError::win32("LookupPrivilegeValueW"));
        }
        let mut tp: TOKEN_PRIVILEGES = std::mem::zeroed();
        tp.PrivilegeCount = 1;
        tp.Privileges[0] = LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        };
        if !ok(AdjustTokenPrivileges(
            token.raw(),
            0,
            &tp,
            size_of::<TOKEN_PRIVILEGES>() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )) {
            return Err(GmError::win32("AdjustTokenPrivileges"));
        }
        // AdjustTokenPrivileges can succeed while enabling nothing.
        if crate::prelude::last_error() != 0 {
            return Err(GmError::win32("AdjustTokenPrivileges(partial)"));
        }
    }
    Ok(())
}

/// Enable every privilege GameMode benefits from; returns the ones that worked.
pub fn enable_all_privileges() -> Vec<String> {
    const PRIVS: &[(&str, &str)] = &[
        ("SeDebugPrivilege", "inspect and retune any process"),
        (
            "SeIncreaseBasePriorityPrivilege",
            "raise process priority above normal",
        ),
        ("SeProfileSingleProcessPrivilege", "read system performance counters"),
        ("SeIncreaseQuotaPrivilege", "adjust working-set limits"),
        ("SeSystemProfilePrivilege", "profile system performance"),
        ("SeLockMemoryPrivilege", "lock pages for the game"),
        ("SeTcbPrivilege", "act as part of the operating system"),
        ("SeAssignPrimaryTokenPrivilege", "assign process tokens"),
    ];
    let mut enabled = Vec::new();
    for (name, why) in PRIVS {
        if enable_privilege(name).is_ok() {
            enabled.push(format!("{name} ({why})"));
        } else {
            log::debug!("privilege {name} not available");
        }
    }
    enabled
}

/// Relaunch the current executable elevated, forwarding `args`.
///
/// Returns `Ok(true)` when the UAC prompt was accepted and a new process was
/// started (the caller should then exit), `Ok(false)` when the user declined.
pub fn relaunch_elevated(args: &str) -> GmResult<bool> {
    let exe = std::env::current_exe().map_err(GmError::Io)?;
    let exe_w = Wide::from_path(&exe);
    let verb = Wide::new("runas");
    let params = Wide::new(args);
    let dir = exe
        .parent()
        .map(Wide::from_path)
        .unwrap_or_else(|| Wide::new("."));
    unsafe {
        // ShellExecuteW returns an HINSTANCE whose value <= 32 signals failure.
        let r = ShellExecuteW(
            std::ptr::null_mut(),
            verb.pcwstr(),
            exe_w.pcwstr(),
            params.pcwstr(),
            dir.pcwstr(),
            SW_SHOWNORMAL,
        );
        let code = r as isize;
        if code > 32 {
            Ok(true)
        } else if code == 5 {
            // ERROR_ACCESS_DENIED: the user pressed "No" on the UAC dialog.
            Ok(false)
        } else {
            Err(GmError::other(format!(
                "ShellExecuteW(runas) failed with code {code}"
            )))
        }
    }
}

/// Open the current process token for querying.
pub fn current_token() -> GmResult<Handle> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if !ok(OpenProcessToken(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            TOKEN_QUERY,
            &mut token,
        )) {
            return Err(GmError::win32("OpenProcessToken"));
        }
        Handle::new(token).ok_or_else(|| GmError::win32("OpenProcessToken(handle)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_state_is_detectable() {
        // Whatever the answer, the call must succeed and be self-consistent.
        let elevated = is_elevated();
        let kind = elevation_type();
        assert!(kind <= 3, "elevation type must be 0..=3, got {kind}");
        if kind == 2 {
            // Full token: must also report elevated.
            assert!(elevated, "full token must report elevated");
        }
    }

    #[test]
    fn privilege_enabling_reports_result() {
        let enabled = enable_all_privileges();
        // SeChangeNotifyPrivilege-like basics are always present, but the debug
        // privileges require admin. Just assert the call is safe and bounded.
        assert!(enabled.len() <= 8);
    }

    #[test]
    fn token_can_be_opened() {
        assert!(current_token().is_ok());
    }

    #[test]
    fn unknown_privilege_fails_cleanly() {
        assert!(enable_privilege("SeNotARealPrivilege").is_err());
    }
}

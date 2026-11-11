//! "Start with Windows" support via the per-user Run key.
//!
//! Uses `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, which needs no
//! administrator rights and applies only to the current user.

use std::path::PathBuf;

use gm_core::error::{GmError, GmResult};
use gm_core::win::system;

/// Registry value name GameMode owns.
const VALUE: &str = "GameMode";
/// Run key under HKEY_CURRENT_USER.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Root selector understood by `gm_core::win::system` (`0` = HKEY_CURRENT_USER).
const HKCU: u32 = 0;

/// Full command line stored in the Run key.
fn command_line() -> GmResult<String> {
    let exe: PathBuf =
        std::env::current_exe().map_err(|e| GmError::other(format!("current_exe: {e}")))?;
    Ok(format!("\"{}\" --tray", exe.display()))
}

/// True when GameMode is registered to start with Windows.
pub fn is_enabled() -> bool {
    system::reg_read(HKCU, RUN_KEY, VALUE)
        .map(|v| v.as_string().is_some())
        .unwrap_or(false)
}

/// Register GameMode to start with Windows.
pub fn enable() -> GmResult<()> {
    let line = command_line()?;
    system::reg_write_string(HKCU, RUN_KEY, VALUE, &line)
}

/// Remove the startup registration.
pub fn disable() -> GmResult<()> {
    match system::reg_delete_value(HKCU, RUN_KEY, VALUE) {
        Ok(()) => Ok(()),
        // Already absent is success, not an error.
        Err(GmError::Win32 { code: 2, .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Set the startup registration to match `want`.
pub fn set(want: bool) -> GmResult<()> {
    if want {
        enable()
    } else {
        disable()
    }
}

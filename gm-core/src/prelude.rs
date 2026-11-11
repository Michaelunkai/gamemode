//! Shared Windows type shorthands and small safe helpers used across the crate.
//!
//! windows-sys 0.61 represents string parameters as raw pointers (`PCWSTR = *const u16`),
//! so every helper here returns an owned buffer that the caller keeps alive for the
//! duration of the call. Nothing in this module returns a pointer to a temporary.
#![allow(dead_code)]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows_sys::core::{PCSTR, PCWSTR, PWSTR};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

/// A NUL-terminated UTF-16 buffer suitable for any `*const u16` / `PWSTR` parameter.
#[derive(Clone)]
pub struct Wide(pub Vec<u16>);

impl Wide {
    #[inline]
    pub fn new(s: &str) -> Self {
        Wide(std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect())
    }
    #[inline]
    pub fn from_path(p: &Path) -> Self {
        Wide(p.as_os_str().encode_wide().chain(std::iter::once(0)).collect())
    }
    /// Pointer valid for as long as `self` is alive and not moved.
    #[inline]
    pub fn pcwstr(&self) -> PCWSTR {
        self.0.as_ptr()
    }
    /// Mutable pointer for output parameters (`PWSTR`).
    #[inline]
    pub fn pwstr(&mut self) -> PWSTR {
        self.0.as_mut_ptr()
    }
    #[inline]
    pub fn capacity(&self) -> u32 {
        self.0.len() as u32
    }
    /// Allocate a writable buffer of `n` UTF-16 code units (zeroed).
    #[inline]
    pub fn buffer(n: usize) -> Self {
        Wide(vec![0u16; n.max(1)])
    }
    /// Decode the buffer back into a `String`, stopping at the first NUL.
    #[inline]
    pub fn to_string_lossy(&self) -> String {
        let end = self.0.iter().position(|&c| c == 0).unwrap_or(self.0.len());
        String::from_utf16_lossy(&self.0[..end])
    }
}

/// A NUL-terminated ANSI/UTF-8 byte buffer for `PCSTR` parameters (e.g. `GetProcAddress`).
#[derive(Clone)]
pub struct Ansi(pub Vec<u8>);

impl Ansi {
    #[inline]
    pub fn new(s: &str) -> Self {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        Ansi(v)
    }
    #[inline]
    pub fn pcstr(&self) -> PCSTR {
        self.0.as_ptr()
    }
}

#[inline]
pub fn wide(s: &str) -> Vec<u16> {
    Wide::new(s).0
}

#[inline]
pub fn from_wide(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
            if len > 1 << 20 {
                break;
            }
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

#[inline]
pub fn from_pwstr(p: PWSTR) -> String {
    from_wide(p)
}

#[inline]
pub fn last_error() -> u32 {
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

#[inline]
pub fn set_last_error(code: u32) {
    unsafe { windows_sys::Win32::Foundation::SetLastError(code) }
}

/// True when a `BOOL` return means success.
#[inline]
pub fn ok(b: i32) -> bool {
    b != 0
}

/// RAII wrapper for a raw `HANDLE` returned by the Win32 API.
pub struct Handle(pub HANDLE);

unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Handle {
    /// Wrap a handle, rejecting `NULL` and `INVALID_HANDLE_VALUE` (both `-1` and null).
    #[inline]
    pub fn new(h: HANDLE) -> Option<Self> {
        if h.is_null() || h as isize == -1 {
            None
        } else {
            Some(Handle(h))
        }
    }
    #[inline]
    pub fn raw(&self) -> HANDLE {
        self.0
    }
    #[inline]
    pub fn into_raw(self) -> HANDLE {
        let h = self.0;
        std::mem::forget(self);
        h
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Read a whole file as UTF-8, trimming a BOM and carriage returns.
pub fn read_text(path: &Path) -> Option<String> {
    let raw = std::fs::read(path).ok()?;
    let mut s = String::from_utf8_lossy(&raw).into_owned();
    if s.starts_with('\u{feff}') {
        s.remove(0);
    }
    Some(s.replace('\r', ""))
}

/// Return the file name of the running executable (e.g. `gamemode.exe`).
pub fn exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "gamemode.exe".to_string())
}

/// Expand a `%VAR%`-style environment string (e.g. `%ProgramFiles%`) to a full path.
pub fn expand_env(s: &str) -> String {
    let src = Wide::new(s);
    let mut out = Wide::buffer(32768);
    unsafe {
        let n = windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW(
            src.pcwstr(),
            out.pwstr(),
            out.capacity(),
        );
        if n > 0 && (n as usize) <= out.0.len() {
            out.0.truncate(n as usize);
            return out.to_string_lossy();
        }
    }
    s.to_string()
}

/// Windows special-folder constants for [`known_folder`].
pub mod csidl {
    pub const PROGRAMS: u32 = 0x0002;
    pub const PERSONAL: u32 = 0x0005;
    pub const APP_DATA: u32 = 0x001A;
    pub const LOCAL_APP_DATA: u32 = 0x001C;
    pub const COMMON_APP_DATA: u32 = 0x0023;
    pub const PROGRAM_FILES: u32 = 0x0026;
    pub const PROGRAM_FILES_COMMON: u32 = 0x002B;
    pub const STARTUP: u32 = 0x0007;
    pub const COMMON_STARTUP: u32 = 0x0018;
    pub const DESKTOP: u32 = 0x0010;
}

/// Retrieve a Windows CSIDL folder path (e.g. `{appdata}`).
pub fn known_folder(id: u32) -> Option<PathBuf> {
    let mut path = Wide::buffer(260);
    unsafe {
        let hr = windows_sys::Win32::UI::Shell::SHGetFolderPathW(
            std::ptr::null_mut(),
            id as i32,
            std::ptr::null_mut(),
            0,
            path.pwstr(),
        );
        if hr < 0 {
            return None;
        }
    }
    let s = path.to_string_lossy();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

/// Round and clamp a float percentage into `0..=100`.
#[inline]
pub fn pct_round(v: f32) -> u8 {
    let r = v.round();
    if r.is_nan() || r <= 0.0 {
        0
    } else if r >= 100.0 {
        100
    } else {
        r as u8
    }
}

#[inline]
pub fn clamp_i64(v: i64, lo: i64, hi: i64) -> i64 {
    v.clamp(lo, hi)
}

pub type CvPtr = *mut c_void;

/// Clears the calling thread's last-error value before a call we want to check.
#[inline]
pub fn clear_error() {
    unsafe { windows_sys::Win32::Foundation::SetLastError(0) };
}

/// Formats a Win32 error code via `FormatMessageW`.
pub fn win32_message(code: u32) -> String {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        FormatMessageW, FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS,
    };
    let mut buf = Wide::buffer(1024);
    let n = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            std::ptr::null(),
            code,
            0,
            buf.pwstr(),
            buf.capacity(),
            std::ptr::null_mut(),
        )
    };
    if n == 0 {
        return format!("unknown error {code}");
    }
    buf.0.truncate(n as usize);
    buf.to_string_lossy().trim().to_string()
}

// ---------------------------------------------------------------------------
// Dynamic symbol resolution
// ---------------------------------------------------------------------------

/// Resolves an exported function by name from a DLL, loading it if necessary.
///
/// Used for the handful of NT functions the windows-sys bindings do not expose.
/// The DLL handle is intentionally leaked: it stays valid for the process life.
pub fn proc_address(dll: &str, function: &str) -> Option<*const c_void> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
    let dll_w = Wide::new(dll);
    let name = Ansi::new(function);
    unsafe {
        let mut module = GetModuleHandleW(dll_w.pcwstr());
        if module.is_null() {
            module = LoadLibraryW(dll_w.pcwstr());
            if module.is_null() {
                return None;
            }
        }
        let addr = GetProcAddress(module, name.pcstr());
        addr.map(|f| f as usize as *const c_void)
    }
}

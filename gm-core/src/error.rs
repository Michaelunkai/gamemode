//! Error type shared by every GameMode subsystem.

use std::fmt;

#[derive(Debug)]
pub enum GmError {
    /// A Win32 call failed; carries `GetLastError()`.
    Win32 { call: &'static str, code: u32 },
    /// A Win32 call failed with an explicit HRESULT.
    Hr { call: &'static str, hr: i32 },
    Io(std::io::Error),
    Config(String),
    NotFound(String),
    Denied(String),
    Other(String),
}

pub type GmResult<T> = Result<T, GmError>;

impl GmError {
    pub fn win32(call: &'static str) -> Self {
        GmError::Win32 {
            call,
            code: crate::prelude::last_error(),
        }
    }
    pub fn hr(call: &'static str, hr: i32) -> Self {
        GmError::Hr { call, hr }
    }
    pub fn other<S: Into<String>>(s: S) -> Self {
        GmError::Other(s.into())
    }
    pub fn cfg<S: Into<String>>(s: S) -> Self {
        GmError::Config(s.into())
    }
}

impl fmt::Display for GmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GmError::Win32 { call, code } => {
                write!(f, "{call} failed (Win32 error {code}: {})", win32_message(*code))
            }
            GmError::Hr { call, hr } => write!(f, "{call} failed (HRESULT 0x{:08X})", hr),
            GmError::Io(e) => write!(f, "I/O error: {e}"),
            GmError::Config(s) => write!(f, "configuration error: {s}"),
            GmError::NotFound(s) => write!(f, "not found: {s}"),
            GmError::Denied(s) => write!(f, "access denied: {s}"),
            GmError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for GmError {}

impl From<std::io::Error> for GmError {
    fn from(e: std::io::Error) -> Self {
        GmError::Io(e)
    }
}

impl From<&str> for GmError {
    fn from(s: &str) -> Self {
        GmError::Other(s.to_string())
    }
}

impl From<String> for GmError {
    fn from(s: String) -> Self {
        GmError::Other(s)
    }
}

/// Human readable text for the common Win32 error codes we may hit.
pub fn win32_message(code: u32) -> String {
    match code {
        0 => "success".into(),
        2 => "file not found".into(),
        3 => "path not found".into(),
        5 => "access denied (run GameMode as administrator)".into(),
        6 => "invalid handle".into(),
        8 => "not enough memory".into(),
        13 => "invalid data".into(),
        31 => "a device attached to the system is not functioning".into(),
        87 => "invalid parameter".into(),
        122 => "the data area passed to a system call is too small".into(),
        123 => "the filename, directory name or volume label syntax is incorrect".into(),
        1073 => "the specified service already exists".into(),
        1060 => "the specified service does not exist".into(),
        1056 => "an instance of the service is already running".into(),
        1062 => "the service has not been started".into(),
        1058 => "the service cannot be started, either because it is disabled or because it has no enabled devices associated with it".into(),
        1450 => "insufficient system resources to complete the requested service".into(),
        231 => "all pipe instances are busy".into(),
        _ => "unknown error".into(),
    }
}

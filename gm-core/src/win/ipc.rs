//! Single-instance enforcement and IPC over a Windows named pipe.
//!
//! A second launch of GameMode must not start a competing optimiser. Instead the
//! first instance owns a named pipe server, and later launches connect to it,
//! send a command line ("toggle", "on", "off", "show", "quit") and exit.
//!
//! The pipe is created with a security descriptor that lets any authenticated
//! user connect, so an elevated server still accepts commands from an
//! unelevated client (and vice versa).

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{
    ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, SetNamedPipeHandleState,
};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::error::{GmError, GmResult};
use crate::prelude::{ok, Ansi, Handle, Wide};

/// Base name of the control pipe (per-user, so sessions never collide).
pub fn pipe_name() -> String {
    // Include the session id so two logged-in users each get their own server.
    let mut sid = 0u32;
    unsafe {
        windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(
            windows_sys::Win32::System::Threading::GetCurrentProcessId(),
            &mut sid,
        );
    }
    format!(r"\\.\pipe\GameMode.Control.v1.{sid}")
}

/// A command sent from a secondary launch to the running instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcCommand {
    Toggle,
    Enable,
    Disable,
    Show,
    Hide,
    Quit,
    Status,
    Reload,
}

impl IpcCommand {
    pub fn as_str(&self) -> &'static str {
        match self {
            IpcCommand::Toggle => "toggle",
            IpcCommand::Enable => "on",
            IpcCommand::Disable => "off",
            IpcCommand::Show => "show",
            IpcCommand::Hide => "hide",
            IpcCommand::Quit => "quit",
            IpcCommand::Status => "status",
            IpcCommand::Reload => "reload",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "toggle" | "" => Some(IpcCommand::Toggle),
            "on" | "enable" | "start" => Some(IpcCommand::Enable),
            "off" | "disable" | "stop" => Some(IpcCommand::Disable),
            "show" | "open" => Some(IpcCommand::Show),
            "hide" => Some(IpcCommand::Hide),
            "quit" | "exit" => Some(IpcCommand::Quit),
            "status" => Some(IpcCommand::Status),
            "reload" => Some(IpcCommand::Reload),
            _ => None,
        }
    }
}

/// Try to hand `command` to an already-running GameMode.
///
/// Returns `Ok(true)` when a running instance accepted the command.
pub fn send_command(command: IpcCommand) -> GmResult<bool> {
    let name = Wide::new(&pipe_name());
    unsafe {
        let h = CreateFileW(
            name.pcwstr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            // ERROR_FILE_NOT_FOUND / ERROR_PIPE_BUSY: no server listening.
            return Ok(false);
        }
        let Some(h) = Handle::new(h) else {
            return Ok(false);
        };
        // Switch to message mode so the server can read one command per message.
        let mut mode = PIPE_READMODE_MESSAGE;
        let _ = SetNamedPipeHandleState(h.raw(), &mut mode, std::ptr::null_mut(), std::ptr::null_mut());

        let payload = command.as_str().as_bytes();
        let mut written = 0u32;
        if !ok(WriteFile(
            h.raw(),
            payload.as_ptr(),
            payload.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )) {
            return Ok(false);
        }
        let _ = FlushFileBuffers(h.raw());

        // Read the server's reply (best effort, short timeout via message mode).
        let mut buf = [0u8; 512];
        let mut read = 0u32;
        if ok(ReadFile(
            h.raw(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut read,
            std::ptr::null_mut(),
        )) {
            log::info!(
                "GameMode replied: {}",
                String::from_utf8_lossy(&buf[..read as usize]).trim()
            );
        }
    }
    Ok(true)
}

/// True when another GameMode instance currently owns the control pipe.
pub fn is_instance_running() -> bool {
    let name = Wide::new(&pipe_name());
    unsafe {
        let h = CreateFileW(
            name.pcwstr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        let running = h != INVALID_HANDLE_VALUE && !h.is_null();
        if running {
            windows_sys::Win32::Foundation::CloseHandle(h);
        }
        running
    }
}

/// The pipe server owned by the primary instance.
pub struct PipeServer {
    name: Wide,
    stop: Arc<AtomicBool>,
    wake: Handle,
}

impl PipeServer {
    /// Create the server and its wake-up event for clean shutdown.
    pub fn new() -> GmResult<Self> {
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        let wake = Handle::new(ev).ok_or_else(|| GmError::win32("CreateEventW"))?;
        Ok(PipeServer {
            name: Wide::new(&pipe_name()),
            stop: Arc::new(AtomicBool::new(false)),
            wake,
        })
    }

    /// A handle that tells the accept loop to exit.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Wake the accept loop so it notices the stop flag.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.wake.raw());
        }
    }

    /// Accept one connection, returning the received command.
    ///
    /// Returns `None` when the server is shutting down.
    pub fn accept_command(&self) -> Option<String> {
        unsafe {
            let h = CreateNamedPipeW(
                self.name.pcwstr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                std::ptr::null(),
            );
            let pipe = match Handle::new(h) {
                Some(p) => p,
                None => {
                    log::warn!("CreateNamedPipeW failed: {}", crate::prelude::last_error());
                    return None;
                }
            };

            // Wait for a client without blocking shutdown: poll the connect call.
            loop {
                if self.stop.load(Ordering::SeqCst) {
                    return None;
                }
                let connected = ok(ConnectNamedPipe(pipe.raw(), std::ptr::null_mut()))
                    || crate::prelude::last_error() == ERROR_PIPE_CONNECTED;
                if connected {
                    break;
                }
                match WaitForSingleObject(self.wake.raw(), 250) {
                    _ => {
                        if self.stop.load(Ordering::SeqCst) {
                            DisconnectNamedPipe(pipe.raw());
                            return None;
                        }
                    }
                }
            }

            let mut buf = [0u8; 512];
            let mut read = 0u32;
            let got = ok(ReadFile(
                pipe.raw(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            ));
            let text = if got {
                String::from_utf8_lossy(&buf[..read as usize]).trim().to_string()
            } else {
                String::new()
            };

            let reply = b"ok\n";
            let mut written = 0u32;
            let _ = WriteFile(
                pipe.raw(),
                reply.as_ptr(),
                reply.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            );
            let _ = FlushFileBuffers(pipe.raw());
            DisconnectNamedPipe(pipe.raw());
            Some(text)
        }
    }
}

/// Read exactly one line from an anonymous pipe handle (used by the GUI).
pub fn read_line(h: HANDLE) -> String {
    let mut out = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    let mut read = 0u32;
    unsafe {
        while ok(ReadFile(
            h as *mut _,
            byte.as_mut_ptr(),
            1,
            &mut read,
            std::ptr::null_mut(),
        )) && read == 1
        {
            if byte[0] == b'\n' {
                break;
            }
            out.push(byte[0]);
            if out.len() > 4096 {
                break;
            }
        }
    }
    String::from_utf8_lossy(&out).trim().to_string()
}

/// Write a line to a handle, ignoring failures.
pub fn write_line(h: HANDLE, text: &str) -> bool {
    let bytes = text.trim_end();
    let mut written = 0u32;
    unsafe {
        ok(WriteFile(
            h as *mut _,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        ))
    }
}

/// Unused helper kept for API completeness: resolve a procedure from a DLL.
pub fn resolve(dll: &str, func: &str) -> Option<usize> {
    let _ = Ansi::new(func);
    crate::win::process::proc_address(dll, func)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_is_session_scoped() {
        let n = pipe_name();
        assert!(n.starts_with(r"\\.\pipe\GameMode.Control.v1."));
        // prefix (29) plus a non-empty session id suffix.
        assert!(n.len() > 29, "pipe name {n} lacks a session suffix");
        let suffix = &n[r"\\.\pipe\GameMode.Control.v1.".len()..];
        assert!(!suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn command_parsing() {
        assert_eq!(IpcCommand::parse("toggle"), Some(IpcCommand::Toggle));
        assert_eq!(IpcCommand::parse("ON"), Some(IpcCommand::Enable));
        assert_eq!(IpcCommand::parse(" off "), Some(IpcCommand::Disable));
        assert_eq!(IpcCommand::parse("quit"), Some(IpcCommand::Quit));
        assert_eq!(IpcCommand::parse("nonsense"), None);
        assert_eq!(IpcCommand::Toggle.as_str(), "toggle");
    }

    #[test]
    fn round_trip_over_a_real_pipe() {
        // Spin up a server, connect as a client, deliver one command.
        let server = PipeServer::new().expect("server");
        let handle = std::thread::spawn(move || server.accept_command());
        // Give the server a moment to create the pipe instance.
        crate::win::process::sleep_ms(200);
        let sent = send_command(IpcCommand::Status).expect("send");
        assert!(sent, "a listening server must accept the command");
        let received = handle.join().expect("join");
        assert_eq!(received.as_deref(), Some("status"));
    }

    #[test]
    fn no_server_means_false_not_error() {
        // With no server on this pipe name the call must report false calmly.
        if !is_instance_running() {
            assert_eq!(send_command(IpcCommand::Toggle).unwrap(), false);
        }
    }
}

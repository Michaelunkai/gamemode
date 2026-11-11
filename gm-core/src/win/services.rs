//! Service control used to pause non-essential background services while gaming.
//!
//! Only services the user lists in `services_to_stop` are touched, each service
//! state is recorded before the stop, and anything GameMode stopped is started
//! again on deactivation.

#![allow(dead_code)]

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
    SERVICE_CONTROL_STOP, SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_START, SERVICE_STATUS_PROCESS, SERVICE_STOP,
};

use crate::error::{GmError, GmResult};
use crate::prelude::{ok, Wide};

/// Snapshot of one service's runtime state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    Stopped,
    StartPending,
    StopPending,
    ContinuePending,
    PausePending,
    Paused,
    Unknown(u32),
}

impl ServiceState {
    fn from_code(c: u32) -> Self {
        match c {
            1 => ServiceState::Stopped,
            2 => ServiceState::StartPending,
            3 => ServiceState::StopPending,
            4 => ServiceState::Running,
            5 => ServiceState::ContinuePending,
            6 => ServiceState::PausePending,
            7 => ServiceState::Paused,
            other => ServiceState::Unknown(other),
        }
    }
    pub fn label(&self) -> String {
        match self {
            ServiceState::Running => "running".into(),
            ServiceState::Stopped => "stopped".into(),
            ServiceState::StartPending => "starting".into(),
            ServiceState::StopPending => "stopping".into(),
            ServiceState::ContinuePending => "continuing".into(),
            ServiceState::PausePending => "pausing".into(),
            ServiceState::Paused => "paused".into(),
            ServiceState::Unknown(c) => format!("unknown({c})"),
        }
    }
    pub fn is_running(&self) -> bool {
        matches!(self, ServiceState::Running | ServiceState::StartPending)
    }
}

/// RAII handle for the service control manager.
struct Scm(HANDLE);
unsafe impl Send for Scm {}
impl Drop for Scm {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

/// RAII handle for one service.
struct Service(HANDLE);
unsafe impl Send for Service {}
impl Drop for Service {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

fn open_scm() -> GmResult<Scm> {
    unsafe {
        let h = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if h.is_null() {
            Err(GmError::win32("OpenSCManagerW"))
        } else {
            Ok(Scm(h))
        }
    }
}

fn open_service(scm: &Scm, name: &str, access: u32) -> GmResult<Service> {
    let n = Wide::new(name);
    unsafe {
        let h = OpenServiceW(scm.0, n.pcwstr(), access);
        if h.is_null() {
            Err(GmError::win32("OpenServiceW"))
        } else {
            Ok(Service(h))
        }
    }
}

/// Query the state of a single named service.
pub fn service_state(name: &str) -> GmResult<ServiceState> {
    let scm = open_scm()?;
    let svc = open_service(&scm, name, SERVICE_QUERY_STATUS)?;
    let mut buf: SERVICE_STATUS_PROCESS = unsafe { std::mem::zeroed() };
    let mut needed = 0u32;
    let size = std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32;
    unsafe {
        if !ok(QueryServiceStatusEx(
            svc.0,
            SC_STATUS_PROCESS_INFO,
            &mut buf as *mut _ as *mut u8,
            size,
            &mut needed,
        )) {
            return Err(GmError::win32("QueryServiceStatusEx"));
        }
    }
    Ok(ServiceState::from_code(buf.dwCurrentState))
}

/// Stop a service and wait up to `timeout_ms` for it to reach the stopped state.
pub fn stop_service(name: &str, timeout_ms: u64) -> GmResult<()> {
    let scm = open_scm()?;
    let svc = open_service(&scm, name, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    let mut status: SERVICE_STATUS_PROCESS = unsafe { std::mem::zeroed() };
    let mut needed = 0u32;
    let size = std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32;
    unsafe {
        if !ok(ControlService(
            svc.0,
            SERVICE_CONTROL_STOP,
            &mut status as *mut _ as *mut _,
        )) {
            // Already stopped or stopping is not an error for our purposes.
            let code = crate::prelude::last_error();
            if code == 1062 {
                return Ok(());
            }
            return Err(GmError::Win32 {
                call: "ControlService(STOP)",
                code,
            });
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        unsafe {
            if !ok(QueryServiceStatusEx(
                svc.0,
                SC_STATUS_PROCESS_INFO,
                &mut status as *mut _ as *mut u8,
                size,
                &mut needed,
            )) {
                break;
            }
        }
        if status.dwCurrentState == 1 {
            return Ok(());
        }
        crate::win::process::sleep_ms(150);
    }
    Ok(())
}

/// Start a service and wait up to `timeout_ms` for it to reach the running state.
pub fn start_service(name: &str, timeout_ms: u64) -> GmResult<()> {
    let scm = open_scm()?;
    let svc = open_service(&scm, name, SERVICE_START | SERVICE_QUERY_STATUS)?;
    unsafe {
        if !ok(windows_sys::Win32::System::Services::StartServiceW(
            svc.0,
            0,
            std::ptr::null(),
        )) {
            let code = crate::prelude::last_error();
            // 1056 = already running.
            if code != 1056 {
                return Err(GmError::Win32 {
                    call: "StartServiceW",
                    code,
                });
            }
        }
    }
    let mut status: SERVICE_STATUS_PROCESS = unsafe { std::mem::zeroed() };
    let mut needed = 0u32;
    let size = std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        unsafe {
            if !ok(QueryServiceStatusEx(
                svc.0,
                SC_STATUS_PROCESS_INFO,
                &mut status as *mut _ as *mut u8,
                size,
                &mut needed,
            )) {
                break;
            }
        }
        if status.dwCurrentState == SERVICE_RUNNING {
            return Ok(());
        }
        crate::win::process::sleep_ms(200);
    }
    Ok(())
}

/// True when the named service exists on this machine.
pub fn service_exists(name: &str) -> bool {
    let Ok(scm) = open_scm() else { return false };
    open_service(&scm, name, SERVICE_QUERY_STATUS).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_service_exists_and_reports_state() {
        // "DcomLaunch" exists on every Windows install and is always running.
        assert!(service_exists("DcomLaunch"), "DcomLaunch must exist");
        let st = service_state("DcomLaunch").expect("state readable");
        assert!(st.is_running(), "DcomLaunch should be running, got {st:?}");
    }

    #[test]
    fn missing_service_reports_false() {
        assert!(!service_exists("GameModeDefinitelyNotAService"));
    }

    #[test]
    fn unsupported_service_open_fails_cleanly() {
        let r = service_state("GameModeDefinitelyNotAService");
        assert!(r.is_err());
    }
}

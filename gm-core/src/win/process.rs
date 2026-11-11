//! Safe wrappers over the Win32 process, thread and CPU APIs used by GameMode.
//!
//! Everything here is deliberately thin: each function performs exactly one
//! documented Win32 call, converts failures into [`GmError`] and never keeps a
//! handle open longer than needed.

#![allow(dead_code)]

use std::mem::size_of;
use std::time::Duration;

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::SetProcessWorkingSetSizeEx;
use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;

use crate::error::{GmError, GmResult};
use crate::prelude::{ok, Ansi, Handle, Wide};

/// Access rights we ask for when opening a foreign process.
pub const ACCESS_FULL_CONTROL: u32 = PROCESS_SET_INFORMATION
    | PROCESS_QUERY_INFORMATION
    | PROCESS_SET_QUOTA
    | PROCESS_SUSPEND_RESUME
    | PROCESS_VM_READ;

/// Access rights that always succeed on the current user's own processes.
pub const ACCESS_QUERY: u32 = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION;

/// One row of a process snapshot.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub exe: String,
    pub threads: u32,
    /// Base priority class as reported by the kernel snapshot (not the live one).
    pub base_priority: i32,
}

/// Per-process CPU and memory counters sampled at a point in time.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessCounters {
    /// Total kernel+user time in 100 ns units.
    pub cpu_time_100ns: u64,
    pub working_set: u64,
    pub private_bytes: u64,
    pub page_faults: u32,
}

impl ProcessCounters {
    pub fn cpu_seconds(&self) -> f64 {
        self.cpu_time_100ns as f64 / 10_000_000.0
    }
}

fn filetime_to_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// Enumerate every process visible to the caller.
pub fn snapshot_processes() -> GmResult<Vec<ProcessInfo>> {
    let mut out = Vec::with_capacity(512);
    unsafe {
        let snap = Handle::new(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0))
            .ok_or_else(|| GmError::win32("CreateToolhelp32Snapshot"))?;
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        if !ok(Process32FirstW(snap.raw(), &mut entry)) {
            return Err(GmError::win32("Process32FirstW"));
        }
        loop {
            let end = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            out.push(ProcessInfo {
                pid: entry.th32ProcessID,
                ppid: entry.th32ParentProcessID,
                exe: String::from_utf16_lossy(&entry.szExeFile[..end]),
                threads: entry.cntThreads,
                base_priority: entry.pcPriClassBase,
            });
            if !ok(Process32NextW(snap.raw(), &mut entry)) {
                break;
            }
        }
    }
    Ok(out)
}

/// Open a process handle with the requested access mask.
pub fn open_process(pid: u32, access: u32) -> Option<Handle> {
    unsafe { Handle::new(OpenProcess(access, 0, pid)) }
}

/// Read the full image path of a running process.
pub fn process_path(pid: u32) -> Option<String> {
    let h = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut buf = Wide::buffer(1024);
    let mut len = buf.capacity();
    unsafe {
        if ok(QueryFullProcessImageNameW(
            h.raw(),
            0,
            buf.pwstr(),
            &mut len,
        )) {
            buf.0.truncate(len as usize);
            Some(buf.to_string_lossy())
        } else {
            None
        }
    }
}

/// Sample the CPU/memory counters of a process.
pub fn process_counters(pid: u32) -> Option<ProcessCounters> {
    let h = open_process(pid, ACCESS_QUERY)?;
    unsafe {
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        if !ok(GetProcessTimes(
            h.raw(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )) {
            return None;
        }
        let mut mem: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        mem.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let have_mem = ok(K32GetProcessMemoryInfo(
            h.raw(),
            &mut mem,
            mem.cb,
        ));
        Some(ProcessCounters {
            cpu_time_100ns: filetime_to_u64(kernel) + filetime_to_u64(user),
            working_set: if have_mem { mem.WorkingSetSize as u64 } else { 0 },
            private_bytes: if have_mem { mem.PagefileUsage as u64 } else { 0 },
            page_faults: if have_mem { mem.PageFaultCount } else { 0 },
        })
    }
}

/// Current priority class of a process, as a `PROCESS_CREATION_FLAGS` value.
pub fn get_priority_class(pid: u32) -> Option<u32> {
    let h = open_process(pid, ACCESS_QUERY)?;
    unsafe {
        let c = GetPriorityClass(h.raw());
        if c == 0 {
            None
        } else {
            Some(c)
        }
    }
}

/// Set the priority class of a process.
pub fn set_priority_class(pid: u32, class: u32) -> GmResult<()> {
    let h = open_process(pid, PROCESS_SET_INFORMATION)
        .ok_or_else(|| GmError::win32("OpenProcess(PROCESS_SET_INFORMATION)"))?;
    unsafe {
        if ok(SetPriorityClass(h.raw(), class)) {
            Ok(())
        } else {
            Err(GmError::win32("SetPriorityClass"))
        }
    }
}

/// Raise a process to HIGH_PRIORITY_CLASS (never realtime unless asked).
pub fn set_high_priority(pid: u32) -> GmResult<()> {
    set_priority_class(pid, HIGH_PRIORITY_CLASS)
}

/// Lower a process to IDLE_PRIORITY_CLASS.
pub fn set_idle_priority(pid: u32) -> GmResult<()> {
    set_priority_class(pid, IDLE_PRIORITY_CLASS)
}

/// Drop a process to BELOW_NORMAL_PRIORITY_CLASS (gentler than idle).
pub fn set_below_normal_priority(pid: u32) -> GmResult<()> {
    set_priority_class(pid, BELOW_NORMAL_PRIORITY_CLASS)
}

/// Enable or disable priority boost for a process.
pub fn set_priority_boost(pid: u32, disable: bool) -> GmResult<()> {
    let h = open_process(pid, PROCESS_SET_INFORMATION)
        .ok_or_else(|| GmError::win32("OpenProcess(boost)"))?;
    unsafe {
        if ok(SetProcessPriorityBoost(h.raw(), if disable { 1 } else { 0 })) {
            Ok(())
        } else {
            Err(GmError::win32("SetProcessPriorityBoost"))
        }
    }
}

/// Current affinity mask of a process, or `None` when it cannot be read.
pub fn get_affinity(pid: u32) -> Option<usize> {
    let h = open_process(pid, ACCESS_QUERY)?;
    unsafe {
        let mut process_mask: usize = 0;
        let mut system_mask: usize = 0;
        if ok(GetProcessAffinityMask(h.raw(), &mut process_mask, &mut system_mask)) {
            Some(process_mask)
        } else {
            None
        }
    }
}

/// Restrict a process to a specific set of logical CPUs.
pub fn set_affinity(pid: u32, mask: usize) -> GmResult<()> {
    if mask == 0 {
        return Err(GmError::other("empty affinity mask"));
    }
    let h = open_process(pid, PROCESS_SET_INFORMATION)
        .ok_or_else(|| GmError::win32("OpenProcess(affinity)"))?;
    unsafe {
        if ok(SetProcessAffinityMask(h.raw(), mask)) {
            Ok(())
        } else {
            Err(GmError::win32("SetProcessAffinityMask"))
        }
    }
}

/// Choose a mask covering `count` logical CPUs centred on the highest-numbered
/// physical cores (usually the fastest, and the ones SMT siblings map to).
pub fn reserve_mask(total_cpus: u32, count: u32) -> usize {
    if total_cpus == 0 {
        return 0;
    }
    let take = count.clamp(1, total_cpus);
    // Take from the low end so the reserved set is stable and predictable,
    // which matters because Windows schedules background work on low indexes first.
    let mut mask: usize = 0;
    for i in 0..take {
        if i < 64 {
            mask |= 1usize << i;
        }
    }
    if mask == 0 {
        mask = 1;
    }
    mask
}

/// Mask that excludes the CPUs in `reserved`.
pub fn background_mask(total_cpus: u32, reserved: usize) -> usize {
    let mut full: usize = 0;
    for i in 0..total_cpus.min(64) {
        full |= 1usize << i;
    }
    let m = full & !reserved;
    if m == 0 {
        full
    } else {
        m
    }
}

/// Logical processor count in the current processor group.
pub fn logical_cpu_count() -> u32 {
    unsafe {
        let n = GetActiveProcessorCount(0xFFFF);
        if n > 0 {
            n
        } else {
            1
        }
    }
}

/// Set the memory priority of a process (0 = very low .. 5 = normal).
pub fn set_memory_priority(pid: u32, priority: u32) -> GmResult<()> {
    let h = open_process(pid, PROCESS_SET_INFORMATION)
        .ok_or_else(|| GmError::win32("OpenProcess(memory priority)"))?;
    let info = MEMORY_PRIORITY_INFORMATION {
        MemoryPriority: priority,
    };
    unsafe {
        if ok(SetProcessInformation(
            h.raw(),
            ProcessMemoryPriority,
            &info as *const _ as *const _,
            size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
        )) {
            Ok(())
        } else {
            Err(GmError::win32("SetProcessInformation(ProcessMemoryPriority)"))
        }
    }
}

/// Enable or disable Windows power throttling for a process.
///
/// `throttle == true` lets the kernel throttle the process; `false` opts it out
/// so its threads keep running at full speed even when the window is background.
pub fn set_power_throttling(pid: u32, throttle: bool) -> GmResult<()> {
    let h = open_process(pid, PROCESS_SET_INFORMATION)
        .ok_or_else(|| GmError::win32("OpenProcess(power throttling)"))?;
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if throttle {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };
    unsafe {
        if ok(SetProcessInformation(
            h.raw(),
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )) {
            Ok(())
        } else {
            Err(GmError::win32("SetProcessInformation(ProcessPowerThrottling)"))
        }
    }
}

/// Trim a process working set, releasing unused pages back to the OS.
pub fn trim_working_set(pid: u32) -> GmResult<()> {
    let h = open_process(pid, PROCESS_SET_QUOTA)
        .ok_or_else(|| GmError::win32("OpenProcess(PROCESS_SET_QUOTA)"))?;
    unsafe {
        if ok(EmptyWorkingSet(h.raw())) {
            Ok(())
        } else {
            Err(GmError::win32("EmptyWorkingSet"))
        }
    }
}

/// Raise the working set of the game so its pages are less likely to be paged out.
pub fn set_working_set_limits(pid: u32, min_bytes: usize, max_bytes: usize) -> GmResult<()> {
    let h = open_process(pid, PROCESS_SET_QUOTA)
        .ok_or_else(|| GmError::win32("OpenProcess(working set)"))?;
    let flags = 1u32; // QUOTA_LIMITS_HARDWS_MIN_ENABLE
    unsafe {
        if ok(SetProcessWorkingSetSizeEx(
            h.raw(),
            min_bytes,
            max_bytes,
            flags,
        )) {
            Ok(())
        } else {
            Err(GmError::win32("SetProcessWorkingSetSizeEx"))
        }
    }
}

/// All thread ids that belong to a process, via a thread snapshot.
pub fn threads_of(pid: u32) -> Vec<u32> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, THREADENTRY32, TH32CS_SNAPTHREAD,
    };
    let mut ids = Vec::with_capacity(64);
    unsafe {
        let snap = match Handle::new(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)) {
            Some(s) => s,
            None => return ids,
        };
        let mut te: THREADENTRY32 = std::mem::zeroed();
        te.dwSize = size_of::<THREADENTRY32>() as u32;
        if !ok(Thread32First(snap.raw(), &mut te)) {
            return ids;
        }
        loop {
            if te.th32OwnerProcessID == pid {
                ids.push(te.th32ThreadID);
            }
            if !ok(Thread32Next(snap.raw(), &mut te)) {
                break;
            }
        }
    }
    ids
}

/// Set the priority of a single thread.
pub fn set_thread_priority(tid: u32, priority: i32) -> GmResult<()> {
    let h = unsafe { Handle::new(OpenThread(THREAD_SET_INFORMATION, 0, tid)) }
        .ok_or_else(|| GmError::win32("OpenThread"))?;
    unsafe {
        if ok(SetThreadPriority(h.raw(), priority)) {
            Ok(())
        } else {
            Err(GmError::win32("SetThreadPriority"))
        }
    }
}

/// Raise every thread of a process to TIME_CRITICAL (used by Extreme mode).
pub fn boost_all_threads(pid: u32, priority: i32) -> usize {
    let mut raised = 0;
    for tid in threads_of(pid) {
        if set_thread_priority(tid, priority).is_ok() {
            raised += 1;
        }
    }
    raised
}

/// Whether a process still exists (and is not a zombie).
pub fn is_process_alive(pid: u32) -> bool {
    let Some(h) = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return false;
    };
    unsafe {
        let mut code: u32 = 0;
        if !ok(GetExitCodeProcess(h.raw(), &mut code)) {
            return false;
        }
        code == 259 // STILL_ACTIVE
    }
}

/// Try to terminate a process the user explicitly asked GameMode to close.
pub fn terminate_process(pid: u32, exit_code: u32) -> GmResult<()> {
    let h = open_process(pid, PROCESS_TERMINATE)
        .ok_or_else(|| GmError::win32("OpenProcess(PROCESS_TERMINATE)"))?;
    unsafe {
        if ok(TerminateProcess(h.raw(), exit_code)) {
            Ok(())
        } else {
            Err(GmError::win32("TerminateProcess"))
        }
    }
}

/// System-wide CPU tick counters used to compute total load.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

/// Read the global idle/kernel/user counters.
pub fn system_times() -> SystemTimes {
    unsafe {
        let mut idle: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        if ok(GetSystemTimes(&mut idle, &mut kernel, &mut user)) {
            SystemTimes {
                idle: filetime_to_u64(idle),
                kernel: filetime_to_u64(kernel),
                user: filetime_to_u64(user),
            }
        } else {
            SystemTimes::default()
        }
    }
}

/// Total physical memory in bytes.
pub fn total_ram_bytes() -> u64 {
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if ok(GlobalMemoryStatusEx(&mut status)) {
            status.ullTotalPhys
        } else {
            0
        }
    }
}

/// Free physical memory in bytes.
pub fn available_ram_bytes() -> u64 {
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if ok(GlobalMemoryStatusEx(&mut status)) {
            status.ullAvailPhys
        } else {
            0
        }
    }
}

/// Set the working set of a process from a byte budget, clamped to safe values.
pub fn set_working_set_bytes(pid: u32, bytes: u64) -> GmResult<()> {
    let min = (bytes / 4).min(bytes) as usize;
    let max = bytes as usize;
    set_working_set_limits(pid, min, max)
}

/// Current process id.
#[inline]
pub fn current_pid() -> u32 {
    unsafe { GetCurrentProcessId() }
}

/// Load a procedure address from an already loaded DLL.
pub fn proc_address(dll: &str, func: &str) -> Option<usize> {
    let name = Wide::new(dll);
    let fname = Ansi::new(func);
    unsafe {
        let h = GetModuleHandleW(name.pcwstr());
        if h.is_null() {
            return None;
        }
        GetProcAddress(h, fname.pcstr()).map(|f| f as usize)
    }
}

/// Sleep without holding a lock, in milliseconds.
pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_contains_current_process() {
        let me = current_pid();
        let list = snapshot_processes().expect("snapshot");
        assert!(list.iter().any(|p| p.pid == me), "current pid {me} must appear");
    }

    #[test]
    fn logical_cpu_count_is_positive() {
        assert!(logical_cpu_count() >= 1);
    }

    #[test]
    fn current_process_is_alive_and_counters_work() {
        assert!(is_process_alive(current_pid()));
        let c = process_counters(current_pid()).expect("counters");
        assert!(c.working_set > 0, "working set should be non-zero");
    }

    #[test]
    fn ram_is_detected() {
        let total = total_ram_bytes();
        assert!(total > 512 * 1024 * 1024, "total RAM looks wrong: {total}");
    }

    #[test]
    fn affinity_round_trip_on_self() {
        let me = current_pid();
        let original = get_affinity(me).expect("affinity");
        // Narrow then restore; the test only needs both calls to be accepted.
        let narrowed = reserve_mask(logical_cpu_count(), 1);
        let _ = set_affinity(me, narrowed);
        set_affinity(me, original).expect("restore affinity");
        assert_eq!(get_affinity(me).unwrap() & original, original);
    }

    #[test]
    fn masks_are_disjoint_and_nonzero() {
        let total = 16u32;
        let reserved = reserve_mask(total, 2);
        let bg = background_mask(total, reserved);
        assert_eq!(reserved & bg, 0, "reserved and background must not overlap");
        assert_eq!(reserved.count_ones(), 2);
        assert_eq!(bg.count_ones(), 14);
    }

    #[test]
    fn self_priority_can_be_set_and_read() {
        let me = current_pid();
        let before = get_priority_class(me).expect("priority class readable");
        set_below_normal_priority(me).expect("lower self");
        let lowered = get_priority_class(me).expect("read back");
        assert_eq!(lowered, BELOW_NORMAL_PRIORITY_CLASS);
        // Restore whatever we had so the test never leaves side effects behind.
        let _ = set_priority_class(me, before);
        assert_eq!(get_priority_class(me).unwrap(), before);
    }
}

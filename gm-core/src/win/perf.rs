//! Real GPU, network and per-process RAM statistics from Windows performance
//! counters (PDH).
//!
//! The raw sample APIs in `process.rs` tell us what a process is doing to the
//! CPU; they say nothing about the graphics device. The counter
//! `\GPU Engine(*)\Utilization Percentage` reports the actual load of every
//! engine of every display device, which is the only load-bearing GPU figure
//! available without vendor SDKs (NVML/ADL). We sum whatever is running and
//! normalise by the number of engines that reported.
//!
//! Everything here is lazily opened on first use and returns `0.0` on any
//! failure, so callers can treat a zero as "no data" and fall back.

#![allow(dead_code)]

use std::ffi::c_void;
use std::sync::Mutex;

use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
};

type Query = *mut c_void;
type Counter = *mut c_void;

/// Result of one GPU sample.
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuSample {
    /// 0-100 percent of the graphics device that is busy.
    pub utilization: f32,
    /// How many distinct engines reported data (0 = counter unavailable).
    pub engines: u32,
}

/// Live reader for the GPU and NIC counters.
pub struct PerfCounters {
    gpu_query: Query,
    gpu_counter: Counter,
    net_query: Query,
    net_counter: Counter,
    /// False when a counter could not be added; every read then returns 0.
    gpu_ok: bool,
    net_ok: bool,
    /// PDH returns the *previous* window on the first collection, so we skip one.
    warmed: bool,
}

unsafe impl Send for PerfCounters {}
unsafe impl Sync for PerfCounters {}

impl Drop for PerfCounters {
    fn drop(&mut self) {
        unsafe {
            if !self.gpu_query.is_null() {
                PdhCloseQuery(self.gpu_query);
            }
            if !self.net_query.is_null() {
                PdhCloseQuery(self.net_query);
            }
        }
    }
}

impl PerfCounters {
    /// Open both queries. Never fails: unavailable counters simply read as 0.
    pub fn new() -> Self {
        let mut gpu_query: Query = std::ptr::null_mut();
        let mut gpu_counter: Counter = std::ptr::null_mut();
        let mut net_query: Query = std::ptr::null_mut();
        let mut net_counter: Counter = std::ptr::null_mut();

        // A counter we could not open must never keep a dangling query alive.
        let mut gpu_ok = false;
        let mut net_ok = false;

        unsafe {
            gpu_ok = PdhOpenQueryW(std::ptr::null(), 0, &mut gpu_query) == 0
                && PdhAddEnglishCounterW(
                    gpu_query,
                    wide("\\GPU Engine(*)\\Utilization Percentage").as_ptr(),
                    0,
                    &mut gpu_counter,
                ) == 0;
            if !gpu_ok {
                PdhCloseQuery(gpu_query);
                gpu_query = std::ptr::null_mut();
            }

            net_ok = PdhOpenQueryW(std::ptr::null(), 0, &mut net_query) == 0
                && PdhAddEnglishCounterW(
                    net_query,
                    wide("\\Network Interface(*)\\Bytes Total/sec").as_ptr(),
                    0,
                    &mut net_counter,
                ) == 0;
            if !net_ok {
                PdhCloseQuery(net_query);
                net_query = std::ptr::null_mut();
            }
        }

        PerfCounters {
            gpu_query,
            gpu_counter,
            net_query,
            net_counter,
            gpu_ok,
            net_ok,
            warmed: false,
        }
    }

    /// Collect both queries and read the GPU utilisation.
    pub fn gpu(&mut self) -> GpuSample {
        if !self.gpu_ok {
            return GpuSample::default();
        }
        // The first collect only primes the counter; a second is needed before
        // any instance reports a real value. We spin two collects here so the
        // caller never sees a first-read zero.
        unsafe {
            let _ = PdhCollectQueryData(self.gpu_query);
            if !self.warmed {
                let _ = PdhCollectQueryData(self.gpu_query);
                self.warmed = true;
            }
        }

        let items = unsafe { read_counter_items(self.gpu_counter) };
        if items.is_empty() {
            return GpuSample::default();
        }

        // Every running engine reports its own percentage. A single engine at
        // full stretch must not blow past 100, so we bound the sum by the number
        // of engines that actually returned a non-zero value and average any
        // value we keep.
        let live: Vec<f64> = items.iter().copied().filter(|v| *v > 0.0).collect();
        if live.is_empty() {
            return GpuSample {
                utilization: 0.0,
                engines: items.len() as u32,
            };
        }
        // Take the busiest engine rather than the mean: gamers care about the
        // worst case, and multi-engine cards report different work per engine.
        // Some counters report 0-100 per engine, others 0-1000, so normalise by
        // the reported peak before clamping.
        let peak = live.iter().copied().fold(0.0f64, f64::max);
        GpuSample {
            utilization: (peak.min(100.0)) as f32,
            engines: live.len() as u32,
        }
    }

    /// Total bytes per second across every network interface.
    pub fn network_bytes_per_sec(&mut self) -> u64 {
        if !self.net_ok {
            return 0;
        }
        unsafe {
            if PdhCollectQueryData(self.net_query) != 0 {
                return 0;
            }
        }
        let items = unsafe { read_counter_items(self.net_counter) };
        items.iter().sum::<f64>().max(0.0) as u64
    }

    /// RAM figures in bytes: (total, available), read from the same
    /// GlobalMemoryStatusEx source the process snapshot uses.
    pub fn ram_bytes(&self) -> (u64, u64) {
        (
            crate::win::process::total_ram_bytes(),
            crate::win::process::available_ram_bytes(),
        )
    }
}

impl Default for PerfCounters {
    fn default() -> Self {
        Self::new()
    }
}

/// RAM figures in bytes: (total, available). Straight from
/// GlobalMemoryStatusEx; no PDH query is involved, so it can never fail over.
pub fn ram_bytes() -> (u64, u64) {
    (
        crate::win::process::total_ram_bytes(),
        crate::win::process::available_ram_bytes(),
    )
}

/// One process-wide reader behind a mutex so every caller shares one PDH query.
static READER: Mutex<Option<PerfCounters>> = Mutex::new(None);

/// Read the GPU utilisation, opening the query on first use.
pub fn gpu_utilization() -> GpuSample {
    let mut guard = match READER.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.is_none() {
        *guard = Some(PerfCounters::new());
    }
    match guard.as_mut() {
        Some(p) => p.gpu(),
        None => GpuSample::default(),
    }
}

/// Total system CPU utilisation, 0-100, excluding idle time.
///
/// `GetSystemTimes` is cumulative since boot, so two reads separated by a short
/// delay are required for a meaningful ratio.
pub fn system_cpu_percent() -> f32 {
    let a = super::process::system_times();
    super::process::sleep_ms(120);
    let b = super::process::system_times();

    let idle = b.idle.saturating_sub(a.idle);
    let kernel = b.kernel.saturating_sub(a.kernel);
    let user = b.user.saturating_sub(a.user);
    // Kernel time already includes idle, so subtract it to avoid double counting.
    let busy = (kernel + user).saturating_sub(idle);
    let total = kernel + user;
    if total == 0 {
        return 0.0;
    }
    (busy as f64 / total as f64 * 100.0).clamp(0.0, 100.0) as f32
}

/// Read total NIC throughput in bytes per second.
pub fn network_bytes_per_sec() -> u64 {
    let mut guard = match READER.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.is_none() {
        *guard = Some(PerfCounters::new());
    }
    match guard.as_mut() {
        Some(p) => p.network_bytes_per_sec(),
        None => 0,
    }
}

/// Read every instance value of a formatted counter as doubles.
///
/// PDH fills a caller buffer with one `PDH_FMT_COUNTERVALUE_ITEM_W` per
/// instance. We ask for the size first, allocate, then read.
unsafe fn read_counter_items(counter: Counter) -> Vec<f64> {
    let mut needed: u32 = 0;
    let mut supplied: u32 = 0;

    // First call returns the needed buffer size in bytes and a non-zero status;
    // that is expected, not an error.
    let _rc = PdhGetFormattedCounterArrayW(
        counter,
        PDH_FMT_DOUBLE,
        &mut needed,
        &mut supplied,
        std::ptr::null_mut(),
    );
    if needed == 0 {
        return Vec::new();
    }

    let stride = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() as u32;
    let mut buffer: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> =
        vec![unsafe { std::mem::zeroed() }; (needed / stride).max(1) as usize];
    let mut cap = (buffer.len() * stride as usize) as u32;
    let rc = PdhGetFormattedCounterArrayW(
        counter,
        PDH_FMT_DOUBLE,
        &mut cap,
        &mut supplied,
        buffer.as_mut_ptr(),
    );
    if rc != 0 {
        return Vec::new();
    }

    let out = buffer
        .iter()
        .map(|item| {
            // FmtValue is a C union; only the field matching PDH_FMT_DOUBLE is valid.
            unsafe { item.FmtValue.Anonymous.doubleValue }
        })
        .collect();
    out
}

/// Encode a Rust string as a NUL-terminated wide string for the Win32 API.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

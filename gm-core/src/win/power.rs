//! Windows power-scheme handling.
//!
//! GameMode switches to a high-performance plan while active and restores the
//! exact scheme that was active before, using the PowrProf GUID-based API.

#![allow(dead_code)]

use windows_sys::core::GUID;
use windows_sys::Win32::System::Power::{
    PowerGetActiveScheme, PowerReadACValueIndex, PowerSetActiveScheme, PowerWriteACValueIndex,
};

use crate::config::PowerPlan;
use crate::error::{GmError, GmResult};

/// The well-known Windows power scheme GUIDs.
pub const GUID_BALANCED: GUID = GUID::from_u128(0x381b4222_f694_41f0_9685_ff5bb260df2e);
pub const GUID_HIGH_PERFORMANCE: GUID = GUID::from_u128(0x8c5e7fda_e8bf_4a96_9a85_a6e23a8c635c);
pub const GUID_POWER_SAVER: GUID = GUID::from_u128(0xa1841308_3541_4fab_bc81_f71556f20b4a);
/// Ultimate Performance is not always present; it is created on demand.
pub const GUID_ULTIMATE: GUID = GUID::from_u128(0xe9a42b02_d5df_448d_aa00_03f14749eb61);

// Processor power subgroup and settings.
const SUB_PROCESSOR: GUID = GUID::from_u128(0x54533251_82be_4824_96c1_47b60b740d00);
/// PROCTHROTTLEMIN - minimum processor state.
const PROCTHROTTLEMIN: GUID = GUID::from_u128(0x893dee8e_2bef_41e0_89c6_b55d0929964c);
/// PROCTHROTTLEMAX - maximum processor state.
const PROCTHROTTLEMAX: GUID = GUID::from_u128(0xbc5038f7_23e0_4960_96da_33abaf5935ec);
/// PERFBOOSTMODE - processor performance boost mode.
const PERFBOOSTMODE: GUID = GUID::from_u128(0xbe337238_0d82_4146_a960_4f374d337634);

/// Format a GUID the way `powercfg` prints it.
pub fn guid_to_string(g: &GUID) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    )
}

/// The GUID of the currently active power scheme.
pub fn active_scheme() -> GmResult<GUID> {
    unsafe {
        let mut ptr: *mut GUID = std::ptr::null_mut();
        let rc = PowerGetActiveScheme(std::ptr::null_mut(), &mut ptr);
        if rc != 0 || ptr.is_null() {
            return Err(GmError::Win32 {
                call: "PowerGetActiveScheme",
                code: rc,
            });
        }
        let g = *ptr;
        // The API hands out a LocalAlloc'd buffer that we must free.
        windows_sys::Win32::Foundation::LocalFree(ptr as *mut _);
        Ok(g)
    }
}

/// Switch the active power scheme.
pub fn set_active_scheme(g: &GUID) -> GmResult<()> {
    unsafe {
        let rc = PowerSetActiveScheme(std::ptr::null_mut(), g);
        if rc != 0 {
            return Err(GmError::Win32 {
                call: "PowerSetActiveScheme",
                code: rc,
            });
        }
    }
    Ok(())
}

/// Map a [`PowerPlan`] onto its GUID, or `None` for `Keep`.
pub fn plan_guid(plan: PowerPlan) -> Option<GUID> {
    match plan {
        PowerPlan::Keep => None,
        PowerPlan::Balanced => Some(GUID_BALANCED),
        PowerPlan::HighPerformance => Some(GUID_HIGH_PERFORMANCE),
        PowerPlan::UltimatePerformance => Some(GUID_ULTIMATE),
    }
}

/// Read the AC value of a processor setting inside a scheme.
pub fn read_processor_setting(scheme: &GUID, setting: &GUID) -> Option<u32> {
    let mut value: u32 = 0;
    unsafe {
        let rc = PowerReadACValueIndex(
            std::ptr::null_mut(),
            scheme,
            &SUB_PROCESSOR,
            setting,
            &mut value,
        );
        if rc == 0 {
            Some(value)
        } else {
            None
        }
    }
}

/// Write an AC value and apply it to the active scheme.
pub fn write_processor_setting(scheme: &GUID, setting: &GUID, value: u32) -> GmResult<()> {
    unsafe {
        let rc = PowerWriteACValueIndex(std::ptr::null_mut(), scheme, &SUB_PROCESSOR, setting, value);
        if rc != 0 {
            return Err(GmError::Win32 {
                call: "PowerWriteACValueIndex",
                code: rc,
            });
        }
    }
    // Re-applying the active scheme makes the new value take effect immediately.
    set_active_scheme(scheme)
}

/// Three processor settings captured so they can be restored exactly.
#[derive(Debug, Clone, Copy)]
pub struct ProcessorTuning {
    pub min_state: Option<u32>,
    pub max_state: Option<u32>,
    pub boost_mode: Option<u32>,
}

/// Read the three settings GameMode may change.
pub fn capture_processor_tuning(scheme: &GUID) -> ProcessorTuning {
    ProcessorTuning {
        min_state: read_processor_setting(scheme, &PROCTHROTTLEMIN),
        max_state: read_processor_setting(scheme, &PROCTHROTTLEMAX),
        boost_mode: read_processor_setting(scheme, &PERFBOOSTMODE),
    }
}

/// Force the CPU to stay at 100% minimum state with boost enabled.
pub fn apply_max_performance(scheme: &GUID) -> Vec<String> {
    let mut notes = Vec::new();
    if write_processor_setting(scheme, &PROCTHROTTLEMIN, 100).is_ok() {
        notes.push("minimum processor state 100%".into());
    }
    if write_processor_setting(scheme, &PROCTHROTTLEMAX, 100).is_ok() {
        notes.push("maximum processor state 100%".into());
    }
    // 2 = Aggressive; 0 = Disabled.
    if write_processor_setting(scheme, &PERFBOOSTMODE, 2).is_ok() {
        notes.push("processor boost mode aggressive".into());
    }
    notes
}

/// Put the captured values back into `scheme`.
pub fn restore_processor_tuning(scheme: &GUID, t: &ProcessorTuning) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(v) = t.min_state {
        if write_processor_setting(scheme, &PROCTHROTTLEMIN, v).is_ok() {
            notes.push(format!("minimum processor state restored to {v}%"));
        }
    }
    if let Some(v) = t.max_state {
        if write_processor_setting(scheme, &PROCTHROTTLEMAX, v).is_ok() {
            notes.push(format!("maximum processor state restored to {v}%"));
        }
    }
    if let Some(v) = t.boost_mode {
        if write_processor_setting(scheme, &PERFBOOSTMODE, v).is_ok() {
            notes.push(format!("processor boost mode restored to {v}"));
        }
    }
    notes
}

/// Create the Ultimate Performance scheme from a template, if it is missing.
///
/// Runs `powercfg -duplicatescheme <ultimate-guid>`; the scheme can only be
/// created after it is duplicated from the template GUID.
pub fn ensure_ultimate_scheme() -> GmResult<()> {
    let mut cmdline: Vec<u16> = "powercfg.exe -duplicatescheme e9a42b02-d5df-448d-aa00-03f14749eb61\0"
        .encode_utf16()
        .collect();
    let mut si: windows_sys::Win32::System::Threading::STARTUPINFOW =
        unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<windows_sys::Win32::System::Threading::STARTUPINFOW>() as u32;
    let mut pi: windows_sys::Win32::System::Threading::PROCESS_INFORMATION =
        unsafe { std::mem::zeroed() };
    unsafe {
        let ok = windows_sys::Win32::System::Threading::CreateProcessW(
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0x0800_0000, // CREATE_NO_WINDOW
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        );
        if ok == 0 {
            return Err(GmError::win32("CreateProcessW(powercfg)"));
        }
        windows_sys::Win32::Foundation::CloseHandle(pi.hProcess);
        windows_sys::Win32::Foundation::CloseHandle(pi.hThread);
    }
    Ok(())
}

/// Compares two GUIDs field by field.
///
/// windows-sys's `GUID` deliberately implements neither `PartialEq` nor `Eq`,
/// so equality has to be spelled out.
pub fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// A friendly name for a known scheme GUID.
pub fn scheme_name(g: &GUID) -> String {
    if guid_eq(g, &GUID_BALANCED) {
        "Balanced".into()
    } else if guid_eq(g, &GUID_HIGH_PERFORMANCE) {
        "High performance".into()
    } else if guid_eq(g, &GUID_POWER_SAVER) {
        "Power saver".into()
    } else if guid_eq(g, &GUID_ULTIMATE) {
        "Ultimate performance".into()
    } else {
        guid_to_string(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_formatting_matches_powercfg() {
        let _guard = crate::testlock::guard();
        assert_eq!(
            guid_to_string(&GUID_HIGH_PERFORMANCE),
            "8C5E7FDA-E8BF-4A96-9A85-A6E23A8C635C"
        );
        assert_eq!(
            guid_to_string(&GUID_BALANCED),
            "381B4222-F694-41F0-9685-FF5BB260DF2E"
        );
    }

    #[test]
    fn scheme_names_resolve() {
        let _guard = crate::testlock::guard();
        assert_eq!(scheme_name(&GUID_BALANCED), "Balanced");
        assert_eq!(scheme_name(&GUID_HIGH_PERFORMANCE), "High performance");
    }

    #[test]
    fn active_scheme_is_readable() {
        let _guard = crate::testlock::guard();
        let g = active_scheme().expect("active scheme");
        assert_ne!(guid_to_string(&g), "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn processor_settings_are_readable_on_balanced() {
        let _guard = crate::testlock::guard();
        // Balanced always exposes min/max processor state.
        let t = capture_processor_tuning(&GUID_BALANCED);
        assert!(t.min_state.is_some(), "min processor state should be readable");
        assert!(t.max_state.is_some(), "max processor state should be readable");
        assert!(t.max_state.unwrap() <= 100);
    }

    #[test]
    fn plan_guid_mapping() {
        let _guard = crate::testlock::guard();
        assert!(plan_guid(PowerPlan::Keep).is_none());
        let mapped = plan_guid(PowerPlan::HighPerformance).unwrap();
        assert!(
            guid_eq(&mapped, &GUID_HIGH_PERFORMANCE),
            "high performance must map to its documented GUID"
        );
    }
}

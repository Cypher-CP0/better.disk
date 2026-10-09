//! Process measurements for the spikes: memory and start time.

use std::time::Duration;

use bd_core::time::Ticks;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryUsage {
    pub working_set: u64,
    pub peak_working_set: u64,
    /// Private commit (`PrivateUsage`): the closest single number to "how
    /// much memory this process owns".
    pub private_bytes: u64,
    pub peak_pagefile: u64,
}

pub fn memory_usage() -> Option<MemoryUsage> {
    let mut c = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: the EX struct starts with PROCESS_MEMORY_COUNTERS and `cb`
    // tells the API how much it may write.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(),
            c.cb,
        )
    };
    ok.as_bool().then_some(MemoryUsage {
        working_set: c.WorkingSetSize as u64,
        peak_working_set: c.PeakWorkingSetSize as u64,
        private_bytes: c.PrivateUsage as u64,
        peak_pagefile: c.PeakPagefileUsage as u64,
    })
}

fn filetime_i64(ft: FILETIME) -> i64 {
    ((i64::from(ft.dwHighDateTime)) << 32) | i64::from(ft.dwLowDateTime)
}

#[derive(Clone, Copy, Debug)]
pub struct ProcessTimes {
    /// When the OS created this process; startup timing starts here, not
    /// at `main`.
    pub created: Ticks,
    pub kernel: Duration,
    pub user: Duration,
}

pub fn process_times() -> Option<ProcessTimes> {
    let (mut created, mut exit, mut kernel, mut user) = Default::default();
    // SAFETY: out-pointers valid for the call.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .ok()?;
    }
    let dur = |ft: FILETIME| Duration::from_nanos(filetime_i64(ft).max(0) as u64 * 100);
    Some(ProcessTimes {
        created: Ticks::from_filetime(filetime_i64(created))?,
        kernel: dur(kernel),
        user: dur(user),
    })
}

/// Milliseconds since the process was created.
pub fn ms_since_process_start() -> Option<f64> {
    let created = process_times()?.created;
    let ticks = created.elapsed_until(Ticks::now());
    Some(ticks as f64 / 10_000.0)
}

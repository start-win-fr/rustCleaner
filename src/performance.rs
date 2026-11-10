//! Efficiency mode, the documented Windows mechanism behind Task Manager's option.
//! It lasts until the program closes and never closes or freezes anything.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::ffi::c_void;
use std::mem::size_of;
use std::thread::{available_parallelism, sleep};
use std::time::Duration;
use sysinfo::{MINIMUM_CPU_UPDATE_INTERVAL, Pid, ProcessesToUpdate, System};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetPriorityClass, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, OpenProcess,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
};

/// Windows internals: slowing them would slow the whole session.
const PROTECTED: [&str; 16] = [
    "system",
    "registry",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "svchost.exe",
    "dwm.exe",
    "explorer.exe",
    "fontdrvhost.exe",
    "sihost.exe",
    "ctfmon.exe",
    "taskhostw.exe",
    "audiodg.exe",
];

#[derive(Debug, Serialize)]
pub struct AppUsage {
    pub name: String,
    pub processes: u32,
    pub memory_bytes: u64,
    pub cpu_percent: f32,
    pub efficiency: bool,
    pub exe_path: Option<String>,
}

pub fn is_protected(name: &str) -> bool {
    let lower = name.to_lowercase();
    PROTECTED.contains(&lower.as_str()) || lower.starts_with("rustcleaner")
}

fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0;
    // SAFETY: `session` is a valid out pointer for the duration of the call.
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

fn own_session() -> Option<u32> {
    // SAFETY: takes no arguments and cannot fail.
    session_of(unsafe { GetCurrentProcessId() })
}

fn system_snapshot() -> Result<System> {
    let mut system = System::new().context("lecture de la liste des processus")?;
    system.refresh_processes(ProcessesToUpdate::All, true);
    Ok(system)
}

/// Lowercase names of every running program.
pub fn running_names() -> Result<HashSet<String>> {
    Ok(system_snapshot()?
        .processes()
        .values()
        .map(|process| process.name().to_string_lossy().to_lowercase())
        .collect())
}

fn is_eligible(pid: Pid, name: &str, session: Option<u32>) -> bool {
    !is_protected(name) && session.is_some() && session_of(pid.as_u32()) == session
}

pub fn list() -> Result<Vec<AppUsage>> {
    let mut system = system_snapshot()?;
    // CPU usage is measured between two samples.
    sleep(MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(400)));
    system.refresh_processes(ProcessesToUpdate::All, true);
    let cores = available_parallelism().map_or(1.0, |count| count.get() as f32);
    let session = own_session();

    let mut apps: BTreeMap<String, AppUsage> = BTreeMap::new();
    for (pid, process) in system.processes() {
        let name = process.name().to_string_lossy().into_owned();
        if !is_eligible(*pid, &name, session) {
            continue;
        }
        let app = apps.entry(name.to_lowercase()).or_insert_with(|| AppUsage {
            name: name.clone(),
            processes: 0,
            memory_bytes: 0,
            cpu_percent: 0.0,
            efficiency: false,
            exe_path: None,
        });
        if app.exe_path.is_none() {
            app.exe_path = process.exe().map(|path| path.display().to_string());
        }
        app.processes += 1;
        app.memory_bytes += process.memory();
        app.cpu_percent += process.cpu_usage() / cores;
        app.efficiency |= is_efficient(pid.as_u32());
    }
    let mut apps: Vec<AppUsage> = apps.into_values().collect();
    apps.sort_by_key(|app| std::cmp::Reverse(app.memory_bytes));
    Ok(apps)
}

/// Applies the mode to every process of a program and returns how many changed.
pub fn set_efficiency(name: &str, enabled: bool) -> Result<u32> {
    let system = system_snapshot()?;
    let session = own_session();
    let targets: Vec<u32> = system
        .processes()
        .iter()
        .filter(|(pid, process)| {
            let process_name = process.name().to_string_lossy();
            process_name.eq_ignore_ascii_case(name) && is_eligible(**pid, &process_name, session)
        })
        .map(|(pid, _)| pid.as_u32())
        .collect();
    if targets.is_empty() {
        bail!("ce programme n'est plus ouvert");
    }
    let changed = targets
        .iter()
        .filter(|pid| apply_to_pid(**pid, enabled).is_ok())
        .count();
    if changed == 0 {
        bail!("Windows a refusé le changement pour ce programme");
    }
    Ok(changed as u32)
}

struct ProcessHandle(*mut c_void);

impl ProcessHandle {
    fn open(pid: u32, access: u32) -> Result<Self> {
        // SAFETY: OpenProcess has no pointer arguments; the result is checked.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            bail!("accès refusé au processus {pid}");
        }
        Ok(Self(handle))
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcess and is closed only here.
        unsafe { CloseHandle(self.0) };
    }
}

pub fn apply_to_pid(pid: u32, enabled: bool) -> Result<()> {
    let process = ProcessHandle::open(pid, PROCESS_SET_INFORMATION)?;
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: if enabled {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
        StateMask: if enabled {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };
    let priority = if enabled {
        IDLE_PRIORITY_CLASS
    } else {
        NORMAL_PRIORITY_CLASS
    };
    // SAFETY: the handle is open; `state` outlives the call and its size is passed.
    let done = unsafe {
        SetProcessInformation(
            process.0,
            ProcessPowerThrottling,
            (&raw const state).cast(),
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) != 0
            && SetPriorityClass(process.0, priority) != 0
    };
    if !done {
        bail!("Windows a refusé le changement");
    }
    Ok(())
}

pub fn is_efficient(pid: u32) -> bool {
    let Ok(process) = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return false;
    };
    // SAFETY: the handle is open for the duration of the call.
    unsafe { GetPriorityClass(process.0) == IDLE_PRIORITY_CLASS }
}

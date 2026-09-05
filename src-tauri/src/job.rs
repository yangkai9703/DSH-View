//! Windows job object that binds the spawned DSH tree to this process's
//! lifetime: when the shell dies for any reason, Windows closes the job and
//! terminates cmd/npx/node regardless of how deep the chain is.

#![cfg(windows)]

use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

static JOB: OnceLock<isize> = OnceLock::new();

fn limit_info() -> JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    info
}

/// Create the job (once) and attach `pid`. Errors are swallowed: losing the
/// job only means the legacy taskkill fallback matters.
pub fn bind(pid: u32) {
    let job = *JOB.get_or_init(|| {
        unsafe { CreateJobObjectW(None, windows::core::PCWSTR::null()) }
            .map(|h| h.0 as isize)
            .unwrap_or(0)
    });
    if job == 0 {
        return;
    }
    unsafe {
        let hjob = HANDLE(job as *mut _);
        let _ = SetInformationJobObject(
            hjob,
            JobObjectExtendedLimitInformation,
            &limit_info() as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if let Ok(hproc) = windows::Win32::System::Threading::OpenProcess(
            windows::Win32::System::Threading::PROCESS_SET_QUOTA
                | windows::Win32::System::Threading::PROCESS_TERMINATE,
            false,
            pid,
        ) {
            // Nested jobs are allowed on Win8+; if the assign fails we just
            // lose the safety net silently.
            let _ = AssignProcessToJobObject(hjob, hproc);
            let _ = CloseHandle(hproc);
        }
    }
}

/// True when `pid` ended up inside our job (diagnostics only).
#[allow(dead_code)]
pub fn is_bound(pid: u32) -> bool {
    let Some(job) = JOB.get() else { return false };
    if *job == 0 {
        return false;
    }
    let mut inside = windows::core::BOOL::default();
    if let Ok(hproc) = unsafe {
        windows::Win32::System::Threading::OpenProcess(
            windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    } {
        unsafe {
            let _ = IsProcessInJob(hproc, Some(HANDLE(*job as *mut _)), &mut inside);
            let _ = CloseHandle(hproc);
        }
    }
    inside.as_bool()
}

/// Close (and thus kill) the job if we own one.
pub fn drop_dsh_job() {
    if let Some(job) = JOB.get() {
        if *job != 0 {
            unsafe {
                let _ = TerminateJobObject(HANDLE(*job as *mut _), 0);
            }
        }
    }
}
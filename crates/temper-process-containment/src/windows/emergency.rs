use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::TerminateJobObject;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::EmergencyTerminationHandle;

const FORCED_TERMINATION_EXIT_CODE: u32 = 143;
const HARD_KILL_EXIT_CODE: u32 = 137;

pub(super) fn job_emergency_handle(job: &OwnedHandle) -> io::Result<EmergencyTerminationHandle> {
    let forced_job = duplicate_job_handle(job)?;
    let hard_kill_job = duplicate_job_handle(job)?;
    EmergencyTerminationHandle::from_owners(
        "windows-job",
        move || terminate_job(&forced_job, FORCED_TERMINATION_EXIT_CODE),
        move || terminate_job(&hard_kill_job, HARD_KILL_EXIT_CODE),
    )
}

fn duplicate_job_handle(job: &OwnedHandle) -> io::Result<OwnedHandle> {
    let process = unsafe { GetCurrentProcess() };
    let mut duplicate: HANDLE = std::ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            process,
            raw(job),
            process,
            &raw mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: DuplicateHandle returned one newly owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate.cast()) })
}

fn terminate_job(job: &OwnedHandle, exit_code: u32) -> io::Result<()> {
    if unsafe { TerminateJobObject(raw(job), exit_code) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle().cast()
}

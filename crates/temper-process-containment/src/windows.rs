use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStringExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::os::windows::process::CommandExt as _;
use std::path::PathBuf;
use std::process::Child;
use std::ptr::{addr_of, null};

use windows_sys::Win32::Foundation::{
    ERROR_BAD_LENGTH, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_NO_MORE_FILES, FILETIME,
    HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, GetProcessTimes, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, ResumeThread, THREAD_SUSPEND_RESUME,
};

use crate::{
    BackendSpawn, ContainmentBackendFactory, ContainmentBackendKind, ContainmentBackendPolicy,
    ContainmentCommand, ContainmentKernel, ContainmentRootIdentity, ContainmentSignal,
    ContainmentSpec, DirectChildReap, MemberDiscovery, PreparedContainmentBackend, ProcessIdentity,
    RecursiveEmptyProof, SignalAttempt, SignalBatch,
};

const JOB_TERMINATION_EXIT_CODE: u32 = 137;
const SETUP_FAILURE_EXIT_CODE: u32 = 125;
const MAX_JOB_MEMBER_IDS: usize = 1_048_576;
const MAX_WINDOWS_PATH_CHARS: usize = 32_768;

/// Race-free Windows descendant containment based on a kill-on-close Job
/// Object. The payload is created suspended, assigned and independently
/// verified in the Job, and only then is its sole initial thread resumed.
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsJobBackendFactory;

impl ContainmentBackendFactory for WindowsJobBackendFactory {
    fn prepare_backend(
        &self,
        policy: ContainmentBackendPolicy,
        spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn PreparedContainmentBackend>> {
        if !matches!(
            policy,
            ContainmentBackendPolicy::Auto | ContainmentBackendPolicy::RequireWindowsJob
        ) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("Windows Job Objects do not satisfy policy {policy:?}"),
            ));
        }

        // Creation, KILL_ON_JOB_CLOSE configuration, and verification are all
        // preparation work. A caller cannot spawn until this succeeds.
        let job = create_kill_on_close_job()?;
        verify_kill_on_close(&job)?;
        let root = ContainmentRootIdentity::new(
            ContainmentBackendKind::WindowsJob,
            format!(
                "job:{:x}:{}",
                job_raw(&job) as usize,
                spec.identity.as_str()
            ),
        );
        Ok(Box::new(PreparedWindowsJob { job, root }))
    }
}

struct PreparedWindowsJob {
    job: OwnedHandle,
    root: ContainmentRootIdentity,
}

impl PreparedContainmentBackend for PreparedWindowsJob {
    fn kind(&self) -> ContainmentBackendKind {
        ContainmentBackendKind::WindowsJob
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.root.clone()
    }

    fn spawn_precontained(
        self: Box<Self>,
        command: ContainmentCommand,
    ) -> io::Result<BackendSpawn> {
        let PreparedWindowsJob { job, root } = *self;
        let mut command = command.into_std_command();
        command.creation_flags(CREATE_SUSPENDED);
        let mut child = command.spawn()?;

        if unsafe { AssignProcessToJobObject(job_raw(&job), child_raw(&child)) } == 0 {
            let error = io::Error::last_os_error();
            return Err(abort_suspended_spawn(
                &job,
                &mut child,
                io::Error::new(
                    error.kind(),
                    format!("assign process to Job Object: {error}"),
                ),
            ));
        }
        if let Err(error) = verify_assignment(&job, &child) {
            return Err(abort_suspended_spawn(&job, &mut child, error));
        }
        if let Err(error) = resume_initial_thread(child.id()) {
            return Err(abort_suspended_spawn(&job, &mut child, error));
        }

        let kernel = WindowsJobKernel {
            job,
            root,
            inspections: 0,
            direct_child_reaped: None,
        };
        Ok(BackendSpawn::new(child, Box::new(kernel)))
    }
}

struct WindowsJobKernel {
    job: OwnedHandle,
    root: ContainmentRootIdentity,
    inspections: u64,
    direct_child_reaped: Option<(u32, Option<i32>)>,
}

impl WindowsJobKernel {
    fn members(&mut self) -> io::Result<MemberDiscovery> {
        self.inspections = self.inspections.saturating_add(1);
        snapshot_members(&self.job)
    }
}

impl ContainmentKernel for WindowsJobKernel {
    fn backend_kind(&self) -> ContainmentBackendKind {
        ContainmentBackendKind::WindowsJob
    }

    fn root_identity(&self) -> ContainmentRootIdentity {
        self.root.clone()
    }

    fn discover_members(&mut self) -> io::Result<MemberDiscovery> {
        self.members()
    }

    fn signal_members(&mut self, signal: ContainmentSignal) -> io::Result<SignalBatch> {
        let members = self.members()?;
        let attempts = match signal {
            // Job Objects do not have a race-free graceful broadcast. Record
            // that fact and let the common state machine advance to KILL.
            ContainmentSignal::Term => members
                .members()
                .iter()
                .cloned()
                .map(|process| {
                    SignalAttempt::failed(
                        process,
                        signal,
                        "Windows Job Objects do not provide graceful TERM",
                    )
                })
                .collect(),
            ContainmentSignal::Kill => {
                if unsafe { TerminateJobObject(job_raw(&self.job), JOB_TERMINATION_EXIT_CODE) } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                members
                    .members()
                    .iter()
                    .cloned()
                    .map(|process| SignalAttempt::succeeded(process, signal))
                    .collect()
            }
        };
        Ok(SignalBatch::new(attempts, members.omitted()))
    }

    fn reap_direct_child(&mut self, child: &mut Child) -> io::Result<DirectChildReap> {
        if let Some((pid, exit_code)) = self.direct_child_reaped {
            return Ok(DirectChildReap::AlreadyReaped { pid, exit_code });
        }
        let pid = child.id();
        match child.try_wait()? {
            Some(status) => {
                let exit_code = status.code();
                self.direct_child_reaped = Some((pid, exit_code));
                Ok(DirectChildReap::Reaped { pid, exit_code })
            }
            None => Ok(DirectChildReap::Pending { pid }),
        }
    }

    fn verify_recursive_empty(&mut self) -> io::Result<RecursiveEmptyProof> {
        // This query is independent of the preceding discovery/signal query.
        // A successful empty list is the recursive Job membership proof.
        let members = self.members()?;
        if members.is_empty() {
            Ok(RecursiveEmptyProof::proven(self.inspections))
        } else {
            Ok(RecursiveEmptyProof::not_empty(
                members.members().to_vec(),
                members.omitted(),
            ))
        }
    }
}

fn create_kill_on_close_job() -> io::Result<OwnedHandle> {
    let raw = unsafe { CreateJobObjectW(null(), null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateJobObjectW returned this uniquely owned handle.
    let job = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job_raw(&job),
            JobObjectExtendedLimitInformation,
            addr_of!(limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

fn verify_kill_on_close(job: &OwnedHandle) -> io::Result<()> {
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    let mut returned = 0;
    if unsafe {
        QueryInformationJobObject(
            job_raw(job),
            JobObjectExtendedLimitInformation,
            (&raw mut limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            &raw mut returned,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE == 0 {
        return Err(io::Error::other(
            "Job Object did not retain JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE",
        ));
    }
    Ok(())
}

fn verify_assignment(job: &OwnedHandle, child: &Child) -> io::Result<()> {
    let mut in_job = 0;
    if unsafe { IsProcessInJob(child_raw(child), job_raw(job), &raw mut in_job) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if in_job == 0 {
        return Err(io::Error::other(
            "assigned process is not reported in its Job Object",
        ));
    }
    let members = query_member_ids(job)?;
    if !members.contains(&child.id()) {
        return Err(io::Error::other(
            "assigned process is absent from the Job Object membership query",
        ));
    }
    Ok(())
}

fn abort_suspended_spawn(job: &OwnedHandle, child: &mut Child, cause: io::Error) -> io::Error {
    let terminate_job = if unsafe { TerminateJobObject(job_raw(job), SETUP_FAILURE_EXIT_CODE) } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    let terminate_child = child.kill();
    let wait = if terminate_job.is_ok() || terminate_child.is_ok() {
        child.wait().map(|_| ())
    } else {
        Err(io::Error::other(
            "neither Job termination nor direct process termination succeeded",
        ))
    };
    io::Error::new(
        cause.kind(),
        format!(
            "{cause}; fail-closed Job termination: {terminate_job:?}; direct termination: {terminate_child:?}; wait: {wait:?}"
        ),
    )
}

fn resume_initial_thread(process_id: u32) -> io::Result<()> {
    let snapshot = toolhelp_snapshot(TH32CS_SNAPTHREAD)?;
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut thread_ids = Vec::new();
    let mut present = unsafe { Thread32First(job_raw(&snapshot), &raw mut entry) } != 0;
    if !present {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
            return Err(error);
        }
    }
    while present {
        if entry.th32OwnerProcessID == process_id {
            thread_ids.push(entry.th32ThreadID);
        }
        present = unsafe { Thread32Next(job_raw(&snapshot), &raw mut entry) } != 0;
        if !present {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                return Err(error);
            }
        }
    }
    if thread_ids.len() != 1 {
        return Err(io::Error::other(format!(
            "suspended process {process_id} has {} initial threads; expected exactly one",
            thread_ids.len()
        )));
    }

    let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_ids[0]) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenThread returned this uniquely owned handle.
    let thread = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    let previous_suspend_count = unsafe { ResumeThread(job_raw(&thread)) };
    if previous_suspend_count == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    if previous_suspend_count != 1 {
        return Err(io::Error::other(format!(
            "initial thread had unexpected suspend count {previous_suspend_count}"
        )));
    }
    Ok(())
}

fn query_member_ids(job: &OwnedHandle) -> io::Result<Vec<u32>> {
    let mut capacity = 64_usize;
    loop {
        if capacity > MAX_JOB_MEMBER_IDS {
            return Err(io::Error::other(format!(
                "Job Object membership exceeds {MAX_JOB_MEMBER_IDS} processes"
            )));
        }
        // Vec<usize> provides the alignment required by the variable-length
        // JOBOBJECT_BASIC_PROCESS_ID_LIST trailing array.
        let mut buffer = vec![0_usize; capacity.saturating_add(2)];
        let byte_len = u32::try_from(buffer.len().saturating_mul(size_of::<usize>()))
            .map_err(|_| io::Error::other("Job membership buffer exceeds u32"))?;
        let mut returned = 0;
        let success = unsafe {
            QueryInformationJobObject(
                job_raw(job),
                JobObjectBasicProcessIdList,
                buffer.as_mut_ptr().cast(),
                byte_len,
                &raw mut returned,
            )
        };
        let header = buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
        let assigned = unsafe { (*header).NumberOfAssignedProcesses as usize };
        let listed = unsafe { (*header).NumberOfProcessIdsInList as usize };
        if success == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_MORE_DATA as i32) {
                capacity = capacity
                    .saturating_mul(2)
                    .max(assigned)
                    .max(listed.saturating_add(1));
                continue;
            }
            return Err(error);
        }
        if assigned > listed {
            capacity = capacity.saturating_mul(2).max(assigned);
            continue;
        }
        if listed > capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Job membership query returned more entries than its buffer",
            ));
        }
        let ids = unsafe {
            std::slice::from_raw_parts(addr_of!((*header).ProcessIdList).cast::<usize>(), listed)
        };
        return ids
            .iter()
            .map(|pid| {
                u32::try_from(*pid).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "Windows process id exceeds u32")
                })
            })
            .collect();
    }
}

#[derive(Clone)]
struct SnapshotProcess {
    parent_pid: u32,
    executable: PathBuf,
}

fn snapshot_members(job: &OwnedHandle) -> io::Result<MemberDiscovery> {
    let ids = query_member_ids(job)?;
    if ids.is_empty() {
        return Ok(MemberDiscovery::empty());
    }
    let metadata = process_snapshot()?;
    let retained = ids.len().min(crate::MAX_SURVIVOR_IDENTITIES);
    let mut identities = Vec::with_capacity(retained);
    for pid in ids.iter().take(retained) {
        identities.push(process_identity(job, *pid, metadata.get(pid))?);
    }
    Ok(MemberDiscovery::new(identities, ids.len() - retained))
}

fn process_snapshot() -> io::Result<BTreeMap<u32, SnapshotProcess>> {
    let snapshot = toolhelp_snapshot(TH32CS_SNAPPROCESS)?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = BTreeMap::new();
    let mut present = unsafe { Process32FirstW(job_raw(&snapshot), &raw mut entry) } != 0;
    if !present {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
            return Err(error);
        }
    }
    while present {
        let end = entry
            .szExeFile
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(entry.szExeFile.len());
        processes.insert(
            entry.th32ProcessID,
            SnapshotProcess {
                parent_pid: entry.th32ParentProcessID,
                executable: PathBuf::from(OsString::from_wide(&entry.szExeFile[..end])),
            },
        );
        present = unsafe { Process32NextW(job_raw(&snapshot), &raw mut entry) } != 0;
        if !present {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                return Err(error);
            }
        }
    }
    Ok(processes)
}

fn process_identity(
    job: &OwnedHandle,
    pid: u32,
    snapshot: Option<&SnapshotProcess>,
) -> io::Result<ProcessIdentity> {
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            // The member exited after the Job snapshot. Retain a non-empty
            // placeholder so this inspection cannot be mistaken for proof.
            return Ok(ProcessIdentity::new(
                pid,
                snapshot.map_or(0, |process| process.parent_pid),
                pid,
                0,
                0,
                snapshot.map_or_else(
                    || PathBuf::from(format!("[exited-pid:{pid}]")),
                    |process| process.executable.clone(),
                ),
            ));
        }
        return Err(error);
    }
    // SAFETY: OpenProcess returned this uniquely owned handle.
    let process = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    let mut in_job = 0;
    if unsafe { IsProcessInJob(job_raw(&process), job_raw(job), &raw mut in_job) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if in_job == 0 {
        return Ok(ProcessIdentity::new(
            pid,
            snapshot.map_or(0, |process| process.parent_pid),
            pid,
            0,
            0,
            PathBuf::from(format!("[reused-pid:{pid}]")),
        ));
    }
    let mut creation: FILETIME = unsafe { zeroed() };
    let mut exit: FILETIME = unsafe { zeroed() };
    let mut kernel: FILETIME = unsafe { zeroed() };
    let mut user: FILETIME = unsafe { zeroed() };
    if unsafe {
        GetProcessTimes(
            job_raw(&process),
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let start_time = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);

    let mut image = vec![0_u16; MAX_WINDOWS_PATH_CHARS];
    let mut image_len = u32::try_from(image.len()).expect("Windows path bound fits u32");
    let executable = if unsafe {
        QueryFullProcessImageNameW(job_raw(&process), 0, image.as_mut_ptr(), &raw mut image_len)
    } != 0
    {
        PathBuf::from(OsString::from_wide(
            &image[..usize::try_from(image_len)
                .unwrap_or(image.len())
                .min(image.len())],
        ))
    } else if let Some(snapshot) = snapshot {
        snapshot.executable.clone()
    } else {
        return Err(io::Error::last_os_error());
    };

    Ok(ProcessIdentity::new(
        pid,
        snapshot.map_or(0, |process| process.parent_pid),
        pid,
        0,
        start_time,
        executable,
    ))
}

fn toolhelp_snapshot(flags: u32) -> io::Result<OwnedHandle> {
    for _ in 0..8 {
        let raw = unsafe { CreateToolhelp32Snapshot(flags, 0) };
        if raw != INVALID_HANDLE_VALUE {
            // SAFETY: CreateToolhelp32Snapshot returned this uniquely owned handle.
            return Ok(unsafe { OwnedHandle::from_raw_handle(raw.cast()) });
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_BAD_LENGTH as i32) {
            return Err(error);
        }
    }
    Err(io::Error::other(
        "Toolhelp snapshot remained unstable after eight attempts",
    ))
}

fn job_raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle().cast()
}

fn child_raw(child: &Child) -> HANDLE {
    child.as_raw_handle().cast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_job_has_verified_kill_on_close() {
        let job = create_kill_on_close_job().expect("create Job Object");
        verify_kill_on_close(&job).expect("verify kill-on-close limit");
        assert!(query_member_ids(&job).expect("query empty Job").is_empty());
    }
}

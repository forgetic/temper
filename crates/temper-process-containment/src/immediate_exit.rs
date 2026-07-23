//! Immediate whole-process termination primitives for bounded crash handoff.

/// Terminates the current process with `exit_code` without unwinding or running
/// process cleanup.
///
/// Unlike [`std::process::exit`] and [`std::process::abort`], this path runs
/// neither C/Rust exit handlers nor core-dump handling. It also does not flush
/// userspace buffers or drop Rust owners. Callers must durably record anything
/// they need before invoking it.
///
/// On Unix this uses `_exit(2)`. On Windows it uses `TerminateProcess` on the
/// current process; the spin loop only makes the diverging contract explicit
/// while the kernel completes asynchronous process teardown.
#[cfg(any(unix, windows))]
pub fn terminate_current_process_immediately(exit_code: u8) -> ! {
    #[cfg(unix)]
    {
        // SAFETY: `_exit` accepts every value representable by `u8` and does
        // not return. This module is the workspace's OS FFI boundary.
        unsafe { libc::_exit(i32::from(exit_code)) }
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

        // SAFETY: GetCurrentProcess returns the calling process's valid pseudo
        // handle. TerminateProcess accepts that handle and does not run DLL
        // detach callbacks or userspace process cleanup.
        let process = unsafe { GetCurrentProcess() };
        let _ = unsafe { TerminateProcess(process, u32::from(exit_code)) };
        loop {
            std::hint::spin_loop();
        }
    }
}

use std::io;

/// Run a trusted payload in a fresh helper process and retain its adopted
/// descendants until they exit naturally. This does not change the enclosing
/// containment: cancellation or loss of its owner still cleans the whole tree.
///
/// Call before starting threads and only in a dedicated helper process. The
/// closure must reap its directly owned children before returning.
#[doc(hidden)]
pub fn run_linux_transient_descendant_owner<T>(run: impl FnOnce() -> T) -> io::Result<T> {
    super::helper::become_subreaper()?;
    let result = run();
    loop {
        // SAFETY: waitpid writes to one valid local status integer. This fresh
        // helper exclusively owns all children remaining after the closure.
        let mut status = 0;
        if unsafe { libc::waitpid(-1, &raw mut status, 0) } >= 0 {
            continue;
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ECHILD) => return Ok(result),
            Some(libc::EINTR) => continue,
            _ => return Err(error),
        }
    }
}

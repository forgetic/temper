use crate::errors::backend_error;
use crate::FilesystemForge;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use temper_forge::ForgeResult;

impl FilesystemForge {
    /// Acquires the store-level exclusive advisory lock for a mutation.
    ///
    /// Every mutating read-modify-write-persist holds this lock for its whole
    /// critical section, so concurrent OS processes and threads sharing one store
    /// serialize onto a single resource. This is what extends ADR 0013's
    /// compare-and-swap across process boundaries (see ADR 0018). Reads stay
    /// lockless because the atomic temp-file rename always yields a complete
    /// old-or-new snapshot.
    ///
    /// The returned [`WriteLock`] releases the lock when dropped. Bind it to a
    /// named local for the whole operation (`let _guard = ...`); a bare
    /// `let _ = ...` would drop it immediately and defeat the lock.
    pub(crate) fn write_lock(&self) -> ForgeResult<WriteLock> {
        // The lock file lives in the store root, so the root must exist first.
        fs::create_dir_all(&self.root).map_err(|error| {
            backend_error(
                format!("create storage root {}", self.root.display()),
                error,
            )
        })?;
        let path = self.root.join(".lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| backend_error(format!("open lock file {}", path.display()), error))?;
        file.lock_exclusive()
            .map_err(|error| backend_error(format!("lock {}", path.display()), error))?;
        Ok(WriteLock { file })
    }
}

/// RAII guard for the filesystem store's exclusive write lock.
///
/// Releases the advisory lock on drop (including during unwind). The lock file
/// itself is never removed; it is a stable coordination point under the store
/// root.
#[must_use = "the store write lock releases as soon as the guard is dropped"]
pub(crate) struct WriteLock {
    file: File,
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

use temper_lib::{Duration, Id, List, Map, Set, Slab};

use crate::authority::Mount;
use crate::job::{self, Job};
use crate::kit::Kit;
use crate::knowledge::Seen;
use crate::path::{Name, Place};

/// The tools sub-model's limits (section 7), handed by its parent to every step
/// read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Kits at once, one per session. An open beyond them is refused as busy.
    pub kits: u32,
    /// Calls a kit runs at once. A call beyond them is answered `Busy`.
    pub calls: u32,
    /// Repositories an authority may name.
    pub repos: u32,
    /// The longest path the tools take, in bytes, as an absolute path's names
    /// joined by `/`: a path a call names, a mount, the working directory.
    pub path_bytes: u32,
    /// Files a kit remembers the LLM read. Past them, the one read longest
    /// ago is forgotten, and must be read again before it is changed.
    pub known_files: u32,
    /// The largest file the tools load or store.
    pub file_bytes: u32,
    /// The most content a read answers with.
    pub read_bytes: u32,
    /// The most entries a listing answers with.
    pub list_entries: u32,
    /// How long a file operation may take, within its call's deadline.
    pub file_timeout: Duration,
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64` or the limits cannot be honoured.
///
/// It counts the containers, their bookkeeping included, and the payloads, not
/// allocator overhead. What travels in events and requests is counted by the
/// layer that holds it: the content of a file loaded or to be stored, and the
/// outcomes answered.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let kits = Slab::<Kit>::worst_case(limits.kits)?.checked_add(u64::from(limits.kits).checked_mul(kit(limits)?)?)?;
    // Only running jobs hold a place, at most `calls` a kit; the slab has more
    // slots, for the jobs answered in an iteration.
    let running = u64::from(limits.kits).checked_mul(u64::from(limits.calls))?;
    let jobs = Slab::<Job>::worst_case(job::slots(limits)?)?.checked_add(running.checked_mul(job::held(limits))?)?;
    kits.checked_add(jobs)
}

/// What one kit holds beyond its slot: its authority, each path in it at most
/// `path_bytes` joined; what its LLM knows, a place for each file; and its
/// jobs' names.
fn kit(limits: &Limits) -> Option<u64> {
    let known = u64::from(limits.known_files).checked_mul(u64::from(limits.path_bytes))?;
    let knowledge = Map::<Place, Seen>::worst_case(limits.known_files)?.checked_add(known)?;
    let jobs = Set::<Id<Job>>::worst_case(limits.calls)?;
    authority(limits)?.checked_add(knowledge)?.checked_add(jobs)
}

/// What a kit's authority holds, each path in it at most `path_bytes` joined.
fn authority(limits: &Limits) -> Option<u64> {
    let path_bytes = u64::from(limits.path_bytes);
    // A name is at least a byte, and a slash parts it from the next.
    let cwd_names = limits.path_bytes.checked_add(1)? / 2;
    let cwd = List::<Name>::worst_case(cwd_names)?.checked_add(path_bytes)?;
    let mount_bytes = u64::from(limits.repos).checked_mul(path_bytes)?;
    let mounts = List::<Mount>::worst_case(limits.repos)?.checked_add(mount_bytes)?;
    cwd.checked_add(mounts)
}

use alloc::boxed::Box;

use skein_lib::{Duration, Id, List, Map, Queue, Slab};

use crate::boundary::{Conflicts, Landing, Repository};
use crate::cache::{Cloned, Workspace};
use crate::facts::Fact;
use crate::hold::{Hold, Tips};

/// The checkout child domain's limits (section 7), handed by its parent to
/// every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Workspaces on the worker's disk: the cache's bound, and the most that
    /// may be held at once. A prepare that finds every one held is refused.
    pub workspaces: u32,
    /// Repositories a spec names: at least one.
    pub repositories: u32,
    /// The longest workstream key, repository name or remote, branch or
    /// identity.
    pub name_bytes: u32,
    /// The longest commit message, its title and body together.
    pub message_bytes: u32,
    /// Maximum originally or still conflicted paths per repository. Zero
    /// disables merge starts while retaining legacy preparation behavior.
    pub conflicts: u32,
    /// Maximum bytes of one full relative conflicted path.
    pub path_bytes: u32,
    /// How long an operation that reaches the forge may take: a clone, a
    /// fetch, a branch's creation, a push.
    pub remote_timeout: Duration,
    /// How long one on the worker's disk may take: making a workspace,
    /// checking out, merging, committing.
    pub local_timeout: Duration,
    /// Facts kept until the parent drains them. Beyond them, facts are dropped
    /// and counted.
    pub facts: u32,
}

/// The most memory the domain holds under `limits`, in bytes (6.3), or `None`
/// if it does not fit a `u64` or the limits cannot be honoured: no workspace,
/// or no repository a spec may name.
///
/// It is the cache's and the holds'. Each workspace owns its key twice (in
/// the workspace and in the index of keys) and the names and remotes of the
/// repositories it holds. Each hold owns its spec's repositories, where each stands, and,
/// while it pushes or saves, the outcomes so far, the message and the
/// saved-work branch; a hold released in an iteration holds them until the
/// reclaim point. It counts the containers, their bookkeeping included, and
/// the payloads, not allocator overhead. The copies that go out in requests
/// are their receivers' to count. Facts own nothing beyond their queue.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.workspaces == 0 || limits.repositories == 0 {
        return None;
    }
    let name = u64::from(limits.name_bytes);
    let workspaces = Slab::<Workspace>::worst_case(limits.workspaces)?;
    let keys = Map::<Box<[u8]>, Id<Workspace>>::worst_case(limits.workspaces)?;
    let idle = Map::<u64, Id<Workspace>>::worst_case(limits.workspaces)?;
    let names = List::<Cloned>::worst_case(limits.repositories)?
        .checked_add(u64::from(limits.repositories).checked_mul(name.checked_mul(2)?)?)?;
    let workspace = name.checked_mul(2)?.checked_add(names)?;
    let cache = u64::from(limits.workspaces).checked_mul(workspace)?;
    let hold_slots = holds(limits)?;
    let holds = Slab::<Hold>::worst_case(hold_slots)?;
    // A repository's name, remote, identity, starting branch and push branch.
    let repository = name.checked_mul(4)?;
    let spec = List::<Repository>::worst_case(limits.repositories)?
        .checked_add(u64::from(limits.repositories).checked_mul(repository)?)?;
    let push = List::<Landing>::worst_case(limits.repositories)?
        .checked_add(u64::from(limits.message_bytes))?
        .checked_add(name)?;
    let paths = List::<Box<[u8]>>::worst_case(limits.conflicts)?
        .checked_add(u64::from(limits.conflicts).checked_mul(u64::from(limits.path_bytes))?)?;
    let path_sets = u64::from(limits.repositories).checked_mul(paths)?;
    let preparing = List::<Conflicts>::worst_case(limits.repositories)?.checked_add(path_sets)?;
    let pushing = push.checked_add(path_sets)?;
    let each = spec
        .checked_add(List::<Tips>::worst_case(limits.repositories)?)?
        .checked_add(preparing)?
        .checked_add(pushing)?;
    let held = u64::from(hold_slots).checked_mul(each)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    workspaces
        .checked_add(keys)?
        .checked_add(idle)?
        .checked_add(cache)?
        .checked_add(holds)?
        .checked_add(held)?
        .checked_add(facts)
}

/// The hold slab's capacity: two for each workspace. A hold holds one
/// workspace, so at most one for each is live; a hold released in an
/// iteration is reclaimed only at its end, and one made in an iteration is
/// not released in it (its prepare waits for io), so the live and the
/// released ones of an iteration are no more than twice the workspaces.
pub(crate) fn holds(limits: &Limits) -> Option<u32> {
    limits.workspaces.checked_mul(2)
}

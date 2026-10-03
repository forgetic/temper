use temper_lib::List;

use crate::rules::Branch;

/// The rules sub-model's limits (programming-model.md, section 7): what its
/// configuration may hold, and what a check takes in. A check asked about
/// more is refused at the entrance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Repositories the deployment may configure.
    pub repositories: u32,
    /// Protected branches the rules may list.
    pub protected: u32,
    /// The most bytes of a branch name.
    pub branch_bytes: u32,
    /// Grants a run may carry.
    pub grants: u32,
    /// Reviews a landing may carry: each reviewer's latest.
    pub reviews: u32,
    /// Gates a plan may add to a step.
    pub gates: u32,
    /// Branches a plan's changes may land on.
    pub lands: u32,
}

/// Findings beyond those a grant or a gate makes: the most a check writes of
/// its own (a plan's repository, size, estimate, protected landing and goal
/// bound; a landing's repository, read-only, CI and review; a run's three
/// spending bounds and read-only).
const OWN: u32 = 5;

/// The most findings one check writes under `limits`: the room its caller
/// provides.
#[must_use]
pub fn max_out(limits: &Limits) -> u32 {
    OWN.saturating_add(limits.grants).saturating_add(limits.gates)
}

/// The most memory the sub-model holds under `limits`, in bytes
/// (programming-model.md, 6.3), or `None` if it does not fit a `u64`: its
/// configuration, the protected branches' names included. What a check is
/// asked is its caller's, and its findings go into the caller's queue.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let protected = List::<Branch>::worst_case(limits.protected)?;
    let names = u64::from(limits.protected).checked_mul(u64::from(limits.branch_bytes))?;
    protected.checked_add(names)
}

/// Whether `len` items fit within `most`.
pub(crate) fn within(len: usize, most: u32) -> bool {
    match u32::try_from(len) {
        Ok(len) => len <= most,
        Err(_) => false,
    }
}

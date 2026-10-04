use alloc::boxed::Box;

use skein_lib::List;

use crate::boundary::Permission;
use crate::limits::{Limits, within};

/// The deployment's rules (engine-domain.md, section 7): its configuration of
/// the vocabulary fixed here, which is all the child domain keeps. The top
/// level holds it, and hands it to every check.
#[derive(Debug)]
pub struct Rules {
    /// How many repositories the deployment configures: writes go only to
    /// them.
    pub repositories: u32,
    /// The branches nothing lands on without green CI on its exact head and
    /// an approving review, which no run pushes to and no write deletes.
    pub protected: List<Branch>,
    /// The engine's own forge user, whose reviews count for nothing.
    pub engine: u64,
    /// The permission a review on a protected branch counts at: an approval
    /// by a person with less is no approval, and their request for changes
    /// holds nothing back. Write at least: a reader's review never counts.
    pub reviewer: Permission,
    /// The largest plan, in steps and in its estimate, that passes unasked,
    /// and who may accept a larger one.
    pub plan_steps: u32,
    pub plan_spend: u64,
    pub plan_acceptance: Permission,
    /// Who must accept a note of a repository's scope, and of the
    /// deployment's; nobody, where the deployment lets runs write them
    /// unasked.
    pub repository_notes: Option<Permission>,
    pub deployment_notes: Option<Permission>,
    /// The most a run may spend, a goal's runs in all, and the deployment's
    /// runs in all, in the deployment's unit of spend.
    pub run_spend: u64,
    pub goal_spend: u64,
    pub deployment_spend: u64,
    /// The permission each act of a person's needs.
    pub acts: Acts,
}

/// A protected branch: `name`, of the deployment's repository at
/// `repository`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Branch {
    pub repository: u32,
    pub name: Box<[u8]>,
}

/// The permission on a repository that each act of a person's needs. Accepting
/// or rejecting a proposal needs this much, and as much as the proposal
/// waits for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Acts {
    pub open: Permission,
    pub steer: Permission,
    pub accept: Permission,
    pub cancel: Permission,
    pub release: Permission,
    pub watch: Permission,
}

impl Rules {
    /// Whether the rules fit `limits`: the shell refuses a configuration that
    /// does not.
    #[must_use]
    pub fn fits(&self, limits: &Limits) -> bool {
        if self.repositories > limits.repositories || self.protected.capacity() > limits.protected {
            return false;
        }
        if self.reviewer < Permission::Write {
            return false;
        }
        for branch in &self.protected {
            if branch.repository >= self.repositories || branch.name.is_empty() {
                return false;
            }
            if !within(branch.name.len(), limits.branch_bytes) {
                return false;
            }
        }
        true
    }
}

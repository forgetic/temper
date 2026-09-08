//! Host-verified absence is distinct from existing source/read authority.

use super::OpaqueWorkspaceTargetIdentity;

/// A run-local proof that one explicit patch creation target was absent when
/// the trusted wrapper checked it. The patch tool must recheck at execution.
/// This never represents graph evidence or a successful read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingWorkspaceTarget {
    _identity: OpaqueWorkspaceTargetIdentity,
}

impl MissingWorkspaceTarget {
    pub fn new(identity: String) -> Option<Self> {
        Some(Self {
            _identity: OpaqueWorkspaceTargetIdentity::new(identity)?,
        })
    }
}

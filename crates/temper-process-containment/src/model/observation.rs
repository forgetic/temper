use super::{
    CleanupSnapshot, ContainmentBackendKind, ContainmentIdentity, ContainmentRootIdentity,
    ContainmentScope, MAX_DIAGNOSTIC_TEXT_BYTES, MAX_ROOT_IDENTITY_BYTES, bounded_text,
};

/// Fully identified cleanup progress delivered to an observer.
///
/// Unlike a bare [`CleanupSnapshot`], this shape always identifies the owner,
/// selected backend, and bounded backend root. This is particularly important
/// for blocked cleanup, which has no terminal [`super::CleanupReport`] yet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupObservation {
    identity: ContainmentIdentity,
    scope: ContainmentScope,
    backend: ContainmentBackendKind,
    root: ContainmentRootIdentity,
    snapshot: CleanupSnapshot,
}

impl CleanupObservation {
    pub fn new(
        identity: ContainmentIdentity,
        scope: ContainmentScope,
        backend: ContainmentBackendKind,
        root: ContainmentRootIdentity,
        snapshot: CleanupSnapshot,
    ) -> Self {
        Self {
            identity,
            scope,
            backend,
            root,
            snapshot,
        }
    }

    pub fn identity(&self) -> &ContainmentIdentity {
        &self.identity
    }

    pub fn scope(&self) -> &ContainmentScope {
        &self.scope
    }

    pub fn backend(&self) -> ContainmentBackendKind {
        self.backend
    }

    pub fn root(&self) -> &ContainmentRootIdentity {
        &self.root
    }

    pub fn snapshot(&self) -> &CleanupSnapshot {
        &self.snapshot
    }
}

/// Auto-selection evidence emitted by a capable backend factory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentCapabilityDiagnostic {
    cgroup_v2_mount: Option<String>,
    delegation: bool,
    nested_subtree_writable: bool,
    cgroup_kill: bool,
    pidfd: bool,
    selected_backend: ContainmentBackendKind,
    fallback_reason: Option<String>,
}

impl ContainmentCapabilityDiagnostic {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cgroup_v2_mount: Option<String>,
        delegation: bool,
        nested_subtree_writable: bool,
        cgroup_kill: bool,
        pidfd: bool,
        selected_backend: ContainmentBackendKind,
        fallback_reason: Option<String>,
    ) -> Self {
        Self {
            cgroup_v2_mount: cgroup_v2_mount
                .map(|value| bounded_text(value, MAX_ROOT_IDENTITY_BYTES)),
            delegation,
            nested_subtree_writable,
            cgroup_kill,
            pidfd,
            selected_backend,
            fallback_reason: fallback_reason
                .map(|value| bounded_text(value, MAX_DIAGNOSTIC_TEXT_BYTES)),
        }
    }

    pub fn cgroup_v2_mount(&self) -> Option<&str> {
        self.cgroup_v2_mount.as_deref()
    }

    pub fn delegation(&self) -> bool {
        self.delegation
    }

    pub fn nested_subtree_writable(&self) -> bool {
        self.nested_subtree_writable
    }

    pub fn cgroup_kill(&self) -> bool {
        self.cgroup_kill
    }

    pub fn pidfd(&self) -> bool {
        self.pidfd
    }

    pub fn selected_backend(&self) -> ContainmentBackendKind {
        self.selected_backend
    }

    pub fn fallback_reason(&self) -> Option<&str> {
        self.fallback_reason.as_deref()
    }
}

/// Identified evidence that auto-selection activated a descendant-complete
/// fallback for one owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentFallbackObservation {
    identity: ContainmentIdentity,
    scope: ContainmentScope,
    backend: ContainmentBackendKind,
    root: ContainmentRootIdentity,
    reason: String,
}

impl ContainmentFallbackObservation {
    pub fn new(
        identity: ContainmentIdentity,
        scope: ContainmentScope,
        backend: ContainmentBackendKind,
        root: ContainmentRootIdentity,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            identity,
            scope,
            backend,
            root,
            reason: bounded_text(reason.into(), MAX_DIAGNOSTIC_TEXT_BYTES),
        }
    }

    pub fn identity(&self) -> &ContainmentIdentity {
        &self.identity
    }

    pub fn scope(&self) -> &ContainmentScope {
        &self.scope
    }

    pub fn backend(&self) -> ContainmentBackendKind {
        self.backend
    }

    pub fn root(&self) -> &ContainmentRootIdentity {
        &self.root
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

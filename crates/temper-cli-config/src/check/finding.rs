// SPDX-License-Identifier: MPL-2.0

/// Whether a finding came from static config inspection or an online probe.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum CheckPhase {
    Offline,
    Online,
}

impl CheckPhase {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Online => "online",
        }
    }
}

/// Machine-readable grouping for validation findings.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum CheckCategory {
    Auth,
    Config,
    Forge,
    Network,
    Path,
    Provider,
    Repository,
    Workflow,
}

impl CheckCategory {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Config => "config",
            Self::Forge => "forge",
            Self::Network => "network",
            Self::Path => "path",
            Self::Provider => "provider",
            Self::Repository => "repo",
            Self::Workflow => "workflow",
        }
    }
}

/// A `temper check` finding with compatibility fields plus online metadata.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct CheckFinding {
    /// `true` for a blocking problem, `false` for an advisory note.
    pub(super) error: bool,
    /// Human-readable message. Must never contain secret payloads.
    pub(super) message: String,
    /// Static/offline or live/online check phase.
    pub(super) check: CheckPhase,
    /// Component or sub-scope the finding applies to.
    pub(super) scope: String,
    /// Coarse category for automation.
    pub(super) category: CheckCategory,
}

impl CheckFinding {
    fn new(
        error: bool,
        check: CheckPhase,
        scope: impl Into<String>,
        category: CheckCategory,
        message: impl Into<String>,
    ) -> Self {
        Self {
            error,
            message: message.into(),
            check,
            scope: scope.into(),
            category,
        }
    }

    pub(super) fn offline_error(
        scope: impl Into<String>,
        category: CheckCategory,
        message: impl Into<String>,
    ) -> Self {
        Self::new(true, CheckPhase::Offline, scope, category, message)
    }

    pub(super) fn offline_note(
        scope: impl Into<String>,
        category: CheckCategory,
        message: impl Into<String>,
    ) -> Self {
        Self::new(false, CheckPhase::Offline, scope, category, message)
    }

    pub(super) fn online_error(
        scope: impl Into<String>,
        category: CheckCategory,
        message: impl Into<String>,
    ) -> Self {
        Self::new(true, CheckPhase::Online, scope, category, message)
    }

    pub(super) fn online_note(
        scope: impl Into<String>,
        category: CheckCategory,
        message: impl Into<String>,
    ) -> Self {
        Self::new(false, CheckPhase::Online, scope, category, message)
    }
}

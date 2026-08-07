// SPDX-License-Identifier: MPL-2.0

//! Raw codebase-memory tool and retention policy schema.

use serde::{Deserialize, Serialize};

/// `[agent.tools.codebase_memory]` — process-boundary settings for the
/// codebase-memory MCP toolset.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodebaseMemoryToolConfig {
    /// Tool mode: `off`, `auto`, or `required`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// MCP server command to spawn for the bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Additional command arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Workflow roles that receive this tool; `*` matches all roles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
    /// Indexing behavior: `off`, `background`, or `blocking`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    /// Startup timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_timeout_secs: Option<u64>,
    /// Indexing timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_timeout_secs: Option<u64>,
    /// Host-controlled retention and maintenance policy.
    #[serde(
        default,
        skip_serializing_if = "CodebaseMemoryRetentionConfig::is_empty"
    )]
    pub retention: CodebaseMemoryRetentionConfig,
}

/// `[agent.tools.codebase_memory.retention]` — bounded cleanup policy for
/// obsolete, Temper-owned path-keyed provider projects.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodebaseMemoryRetentionConfig {
    /// Enables worker-owned startup and periodic maintenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Maximum verified obsolete ephemeral projects retained by count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_obsolete_projects: Option<u32>,
    /// Maximum age of a verified obsolete ephemeral project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_days: Option<u32>,
    /// Delay between worker-owned maintenance passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_interval_secs: Option<u64>,
    /// Absolute provider-operation budget for one pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_timeout_secs: Option<u64>,
    /// Maximum records requested from one provider inventory page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_page_size: Option<u32>,
    /// Maximum inventory pages followed in one pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inventory_pages: Option<u32>,
    /// Maximum provider projects deleted in one pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_deletions_per_run: Option<u32>,
}

impl CodebaseMemoryRetentionConfig {
    pub(crate) fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.max_obsolete_projects.is_none()
            && self.max_age_days.is_none()
            && self.maintenance_interval_secs.is_none()
            && self.maintenance_timeout_secs.is_none()
            && self.inventory_page_size.is_none()
            && self.max_inventory_pages.is_none()
            && self.max_deletions_per_run.is_none()
    }
}

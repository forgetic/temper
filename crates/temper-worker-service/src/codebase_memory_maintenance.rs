//! Resolved-config adapter for worker-owned codebase-memory maintenance.

use std::collections::BTreeSet;
use std::time::Duration;

use temper_config::Resolved;
use temper_worker::{
    CodebaseMemoryMaintenanceConfig, CodebaseMemoryRetentionPolicy, CodebaseMemoryRetentionScope,
};

/// Projects resolved non-secret settings into the worker maintenance shape.
pub fn codebase_memory_maintenance_config(
    resolved: &Resolved,
) -> Option<CodebaseMemoryMaintenanceConfig> {
    let tool = resolved.agent.tools.codebase_memory.as_ref()?;
    let roles = resolved
        .worker
        .capabilities
        .iter()
        .map(|capability| capability.role.clone())
        .collect::<BTreeSet<_>>();
    let repository_dirs = resolved
        .worker
        .capabilities
        .iter()
        .filter_map(|capability| capability.repo.rsplit('/').next())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    Some(CodebaseMemoryMaintenanceConfig::new(
        tool.command.clone(),
        tool.args.clone(),
        Duration::from_secs(tool.startup_timeout_secs),
        CodebaseMemoryRetentionPolicy {
            enabled: tool.retention.enabled,
            max_obsolete_projects: tool.retention.max_obsolete_projects,
            max_age_days: tool.retention.max_age_days,
            maintenance_interval_secs: tool.retention.maintenance_interval_secs,
            maintenance_timeout_secs: tool.retention.maintenance_timeout_secs,
            inventory_page_size: tool.retention.inventory_page_size,
            max_inventory_pages: tool.retention.max_inventory_pages,
            max_deletions_per_run: tool.retention.max_deletions_per_run,
        },
        CodebaseMemoryRetentionScope {
            workspace_root: resolved.paths.workspace_dir.clone(),
            roles,
            repository_dirs,
        },
    ))
}

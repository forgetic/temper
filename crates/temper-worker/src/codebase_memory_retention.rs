//! Ownership-safe bounded retention for obsolete codebase-memory projects.
//!
//! This module is provider-neutral. A host adapter negotiates the destructive
//! provider API and supplies bounded inventory pages; this core classifies and
//! deletes records without ever making provider maintenance tools model-visible.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use temper_protocol_agent::CodebaseMemoryRetentionPolicy;

#[path = "codebase_memory_retention/report.rs"]
mod report;
pub use report::{
    CodebaseMemoryMaintenanceProvider, CodebaseMemoryProjectPage, CodebaseMemoryProjectRecord,
    CodebaseMemoryRetentionFailure, CodebaseMemoryRetentionOutcome,
    CodebaseMemoryRetentionRecordResult, CodebaseMemoryRetentionReport,
    CodebaseMemoryRetentionScope,
};

/// Runs one bounded retention pass.
///
/// `active_work` is checked before inventory, between pages, and before every
/// deletion. Worker composition supplies a registry-backed probe and suppresses
/// maintenance whenever any local assignment is active. Periodic maintenance
/// uses apply mode; operator dry-runs use the paired planner, which shares the
/// inventory and classifier but never calls provider deletion.
pub fn maintain_obsolete_codebase_memory_indexes(
    provider: &mut dyn CodebaseMemoryMaintenanceProvider,
    policy: CodebaseMemoryRetentionPolicy,
    scope: &CodebaseMemoryRetentionScope,
    now_unix_secs: u64,
    active_work: &dyn Fn() -> bool,
) -> CodebaseMemoryRetentionReport {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(policy.maintenance_timeout_secs);
    let mut report = maintain_obsolete_codebase_memory_indexes_until(
        provider,
        policy,
        scope,
        now_unix_secs,
        active_work,
        deadline,
    );
    report.duration_ms = duration_ms(started.elapsed());
    report
}

/// Produces the exact bounded retention plan without invoking deletion.
pub fn plan_obsolete_codebase_memory_indexes(
    provider: &mut dyn CodebaseMemoryMaintenanceProvider,
    policy: CodebaseMemoryRetentionPolicy,
    scope: &CodebaseMemoryRetentionScope,
    now_unix_secs: u64,
    active_work: &dyn Fn() -> bool,
) -> CodebaseMemoryRetentionReport {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(policy.maintenance_timeout_secs);
    let mut report = plan_obsolete_codebase_memory_indexes_until(
        provider,
        policy,
        scope,
        now_unix_secs,
        active_work,
        deadline,
    );
    report.duration_ms = duration_ms(started.elapsed());
    report
}

/// Deadline-injected form used by the provider adapter so process startup,
/// negotiation, inventory, and deletion share one absolute pass budget.
pub fn maintain_obsolete_codebase_memory_indexes_until(
    provider: &mut dyn CodebaseMemoryMaintenanceProvider,
    policy: CodebaseMemoryRetentionPolicy,
    scope: &CodebaseMemoryRetentionScope,
    now_unix_secs: u64,
    active_work: &dyn Fn() -> bool,
    deadline: Instant,
) -> CodebaseMemoryRetentionReport {
    retention_pass(
        provider,
        policy,
        scope,
        now_unix_secs,
        active_work,
        deadline,
        RetentionMode::Apply,
    )
}

/// Deadline-injected dry-run used by the explicit recovery command.
pub fn plan_obsolete_codebase_memory_indexes_until(
    provider: &mut dyn CodebaseMemoryMaintenanceProvider,
    policy: CodebaseMemoryRetentionPolicy,
    scope: &CodebaseMemoryRetentionScope,
    now_unix_secs: u64,
    active_work: &dyn Fn() -> bool,
    deadline: Instant,
) -> CodebaseMemoryRetentionReport {
    retention_pass(
        provider,
        policy,
        scope,
        now_unix_secs,
        active_work,
        deadline,
        RetentionMode::Plan,
    )
}

#[derive(Clone, Copy)]
enum RetentionMode {
    Plan,
    Apply,
}

fn retention_pass(
    provider: &mut dyn CodebaseMemoryMaintenanceProvider,
    policy: CodebaseMemoryRetentionPolicy,
    scope: &CodebaseMemoryRetentionScope,
    now_unix_secs: u64,
    active_work: &dyn Fn() -> bool,
    deadline: Instant,
    mode: RetentionMode,
) -> CodebaseMemoryRetentionReport {
    if !policy.enabled {
        return CodebaseMemoryRetentionReport::no_op_with_outcome(
            "retention policy is disabled",
            CodebaseMemoryRetentionOutcome::Disabled,
        )
        .with_policy(policy);
    }
    if active_work() {
        return CodebaseMemoryRetentionReport::no_op_with_outcome(
            "active worker assignments suppress provider maintenance",
            CodebaseMemoryRetentionOutcome::SuppressedActiveWork,
        )
        .with_policy(policy);
    }
    let workspace_root = match scope.workspace_root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            return CodebaseMemoryRetentionReport::no_op_with_outcome(
                format!("canonical workspace root is unavailable: {error}"),
                CodebaseMemoryRetentionOutcome::SafetyNoOp,
            )
            .with_policy(policy);
        }
    };
    let mut report = CodebaseMemoryRetentionReport {
        policy: Some(policy),
        inventory_attempted: true,
        dry_run: matches!(mode, RetentionMode::Plan),
        deleted_estimated_bytes: Some(0),
        ..CodebaseMemoryRetentionReport::default()
    };
    let mut records = Vec::new();
    let mut cursor = None;
    let mut seen_cursors = BTreeSet::new();
    let mut seen_projects = BTreeSet::new();
    let inventory_started = Instant::now();

    for _ in 0..policy.max_inventory_pages {
        report.inventory_duration_ms = duration_ms(inventory_started.elapsed());
        if active_work() {
            return inventory_no_op(
                report,
                records,
                "active worker assignments appeared during inventory",
                CodebaseMemoryRetentionOutcome::SuppressedActiveWork,
            );
        }
        if Instant::now() >= deadline {
            return inventory_no_op(
                report,
                records,
                "maintenance inventory deadline expired",
                CodebaseMemoryRetentionOutcome::TimedOut,
            );
        }
        let page = match provider.inventory_page(
            cursor.as_deref(),
            policy.inventory_page_size,
            deadline,
        ) {
            Ok(page) => page,
            Err(error) => {
                report.inventory_duration_ms = duration_ms(inventory_started.elapsed());
                return inventory_no_op(
                    report,
                    records,
                    format!("provider inventory was uncertain: {error}"),
                    CodebaseMemoryRetentionOutcome::DiscoveryFailed,
                );
            }
        };
        report.inventory_duration_ms = duration_ms(inventory_started.elapsed());
        if page.projects.len() > policy.inventory_page_size as usize {
            records.extend(page.projects);
            return inventory_no_op(
                report,
                records,
                "provider returned more records than the negotiated page bound",
                CodebaseMemoryRetentionOutcome::InventoryUncertain,
            );
        }
        match (report.cache_bytes, page.cache_bytes) {
            (Some(expected), Some(actual)) if expected != actual => {
                records.extend(page.projects);
                return inventory_no_op(
                    report,
                    records,
                    "provider cache byte estimate changed during inventory",
                    CodebaseMemoryRetentionOutcome::InventoryUncertain,
                );
            }
            (None, bytes) => report.cache_bytes = bytes,
            _ => {}
        }
        let Some(instance) = page.cache_instance_id.as_deref().and_then(nonempty) else {
            records.extend(page.projects);
            return inventory_no_op(
                report,
                records,
                "provider omitted its cache instance identity",
                CodebaseMemoryRetentionOutcome::InventoryUncertain,
            );
        };
        match report.cache_instance_id.as_deref() {
            Some(expected) if expected != instance => {
                records.extend(page.projects);
                return inventory_no_op(
                    report,
                    records,
                    "provider cache instance changed during inventory",
                    CodebaseMemoryRetentionOutcome::InventoryUncertain,
                );
            }
            None => report.cache_instance_id = Some(instance.to_string()),
            Some(_) => {}
        }
        for record in &page.projects {
            let Some(project) = record.project.as_deref().and_then(nonempty) else {
                records.extend(page.projects.clone());
                return inventory_no_op(
                    report,
                    records,
                    "provider returned incomplete project identity metadata",
                    CodebaseMemoryRetentionOutcome::InventoryUncertain,
                );
            };
            if record.repo_path.is_none() || record.updated_at_unix_secs.is_none() {
                records.extend(page.projects.clone());
                return inventory_no_op(
                    report,
                    records,
                    "provider returned incomplete project lifecycle metadata",
                    CodebaseMemoryRetentionOutcome::InventoryUncertain,
                );
            }
            if !seen_projects.insert(project.to_string()) {
                records.extend(page.projects.clone());
                return inventory_no_op(
                    report,
                    records,
                    "provider returned a duplicate project identity",
                    CodebaseMemoryRetentionOutcome::InventoryUncertain,
                );
            }
        }
        records.extend(page.projects);
        report.inventory_record_count = records.len();
        cursor = page.next_cursor.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        let Some(next) = cursor.as_deref() else {
            report.inventory_complete = true;
            break;
        };
        if !seen_cursors.insert(next.to_string()) {
            return inventory_no_op(
                report,
                records,
                "provider inventory cursor repeated",
                CodebaseMemoryRetentionOutcome::InventoryUncertain,
            );
        }
    }

    report.inventory_duration_ms = duration_ms(inventory_started.elapsed());
    if !report.inventory_complete {
        return inventory_no_op(
            report,
            records,
            "provider inventory exceeded the configured page bound",
            CodebaseMemoryRetentionOutcome::InventoryUncertain,
        );
    }
    if records
        .iter()
        .any(|record| record.indexing_active == Some(true))
    {
        return inventory_no_op(
            report,
            records,
            "provider reports active indexing; destructive maintenance requires quiescence",
            CodebaseMemoryRetentionOutcome::InventoryUncertain,
        );
    }
    if report.cache_bytes.is_none()
        && records
            .iter()
            .all(|record| record.estimated_bytes.is_some())
    {
        report.cache_bytes = records.iter().try_fold(0_u64, |total, record| {
            total.checked_add(record.estimated_bytes.expect("checked above"))
        });
    }

    report.inventory_record_count = records.len();
    let mut eligible = Vec::new();
    for record in records {
        match classify(&record, &workspace_root, scope) {
            Classification::Preserve(reason) => {
                report.preserved.push(record_result(&record, reason))
            }
            Classification::Eligible {
                project,
                path,
                updated,
                estimated_bytes,
            } => {
                if updated > now_unix_secs {
                    report.preserved.push(CodebaseMemoryRetentionRecordResult {
                        project,
                        repo_path: Some(path),
                        estimated_bytes,
                        reason: "project timestamp is in the future; lifecycle age is uncertain"
                            .to_string(),
                    });
                } else {
                    eligible.push(EligibleRecord {
                        project,
                        path,
                        updated,
                        estimated_bytes,
                    });
                }
            }
        }
    }

    // Newest deterministic records satisfy the count allowance. Age applies
    // independently, so an old record is reclaimed even when under the count.
    eligible.sort_by(|left, right| {
        right
            .updated
            .cmp(&left.updated)
            .then_with(|| left.project.cmp(&right.project))
    });
    let count_candidates = eligible
        .iter()
        .skip(policy.max_obsolete_projects as usize)
        .map(|record| record.project.clone())
        .collect::<BTreeSet<_>>();
    let max_age_secs = u64::from(policy.max_age_days).saturating_mul(24 * 60 * 60);
    let mut candidates = Vec::new();
    for eligible in eligible {
        let old = now_unix_secs
            .checked_sub(eligible.updated)
            .is_some_and(|age| age >= max_age_secs);
        let excess = count_candidates.contains(&eligible.project);
        if old || excess {
            let reason = match (old, excess) {
                (true, true) => "exceeds configured age and count bounds",
                (true, false) => "exceeds configured age bound",
                (false, true) => "exceeds configured count bound",
                (false, false) => unreachable!(),
            };
            candidates.push((eligible, reason));
        } else {
            report.preserved.push(CodebaseMemoryRetentionRecordResult {
                project: eligible.project,
                repo_path: Some(eligible.path),
                estimated_bytes: eligible.estimated_bytes,
                reason: "within configured age and count bounds".to_string(),
            });
        }
    }
    candidates.sort_by(|(left, _), (right, _)| {
        left.updated
            .cmp(&right.updated)
            .then_with(|| left.project.cmp(&right.project))
    });
    report.candidates = candidates
        .iter()
        .map(|(record, reason)| CodebaseMemoryRetentionRecordResult {
            project: record.project.clone(),
            repo_path: Some(record.path.clone()),
            estimated_bytes: record.estimated_bytes,
            reason: (*reason).to_string(),
        })
        .collect();

    for (position, (candidate, reason)) in candidates.into_iter().enumerate() {
        let result = CodebaseMemoryRetentionRecordResult {
            project: candidate.project.clone(),
            repo_path: Some(candidate.path),
            estimated_bytes: candidate.estimated_bytes,
            reason: reason.to_string(),
        };
        if position >= policy.max_deletions_per_run as usize {
            report.preserved.push(CodebaseMemoryRetentionRecordResult {
                reason: "deferred by the per-run deletion cap".to_string(),
                ..result
            });
            continue;
        }
        if matches!(mode, RetentionMode::Plan) {
            report.proposed.push(result);
            continue;
        }
        if active_work() {
            report.no_op_reason = Some(
                "active worker assignments appeared; remaining deletions were suppressed"
                    .to_string(),
            );
            report.outcome = CodebaseMemoryRetentionOutcome::SuppressedActiveWork;
            report.preserved.push(CodebaseMemoryRetentionRecordResult {
                reason: "active work appeared before deletion".to_string(),
                ..result
            });
            continue;
        }
        if Instant::now() >= deadline {
            report.no_op_reason =
                Some("maintenance deadline expired; remaining deletions were deferred".to_string());
            report.outcome = CodebaseMemoryRetentionOutcome::TimedOut;
            report.preserved.push(CodebaseMemoryRetentionRecordResult {
                reason: "deferred after the maintenance deadline".to_string(),
                ..result
            });
            continue;
        }
        match provider.delete_project(&candidate.project, deadline) {
            Ok(()) => {
                report.deleted_estimated_bytes =
                    match (report.deleted_estimated_bytes, result.estimated_bytes) {
                        (Some(total), Some(bytes)) => Some(total.saturating_add(bytes)),
                        _ => None,
                    };
                report.deleted.push(result);
            }
            Err(error) => report.failed.push(CodebaseMemoryRetentionFailure {
                record: result,
                error,
            }),
        }
    }
    report.outcome = if !report.failed.is_empty() {
        CodebaseMemoryRetentionOutcome::PartialFailure
    } else if report.no_op_reason.is_some() {
        report.outcome
    } else {
        CodebaseMemoryRetentionOutcome::Completed
    };
    report
}

/// Applies only the exact actions from a verified dry-run/preflight report.
///
/// The caller must compare an operator-review dry-run with a fresh preflight
/// generated by [`plan_obsolete_codebase_memory_indexes_until`] before calling
/// this function. No inventory is repeated here, so the deletion class cannot
/// silently expand after the comparison.
pub(crate) fn apply_verified_codebase_memory_plan(
    provider: &mut dyn CodebaseMemoryMaintenanceProvider,
    mut verified: CodebaseMemoryRetentionReport,
    active_work: &dyn Fn() -> bool,
    deadline: Instant,
) -> CodebaseMemoryRetentionReport {
    verified.dry_run = false;
    if !verified.inventory_complete
        || verified.cache_instance_id.is_none()
        || verified.no_op_reason.is_some()
    {
        verified.no_op_reason = Some(
            "destructive execution refused because the retention preflight was not verified"
                .to_string(),
        );
        verified.outcome = CodebaseMemoryRetentionOutcome::InventoryUncertain;
        verified.proposed.clear();
        return verified;
    }
    if active_work() {
        verified.no_op_reason = Some(
            "active worker assignments appeared after retention preflight; deletion refused"
                .to_string(),
        );
        verified.outcome = CodebaseMemoryRetentionOutcome::SuppressedActiveWork;
        return verified;
    }

    for record in verified.proposed.clone() {
        let active = active_work();
        let timed_out = Instant::now() >= deadline;
        if active || timed_out {
            verified.no_op_reason = Some(
                "retention apply lost quiescence or exceeded its deadline; remaining deletions were suppressed"
                    .to_string(),
            );
            verified.outcome = if timed_out {
                CodebaseMemoryRetentionOutcome::TimedOut
            } else {
                CodebaseMemoryRetentionOutcome::SuppressedActiveWork
            };
            break;
        }
        match provider.delete_project(&record.project, deadline) {
            Ok(()) => {
                verified.deleted_estimated_bytes =
                    match (verified.deleted_estimated_bytes, record.estimated_bytes) {
                        (Some(total), Some(bytes)) => Some(total.saturating_add(bytes)),
                        _ => None,
                    };
                verified.deleted.push(record);
            }
            Err(error) => verified
                .failed
                .push(CodebaseMemoryRetentionFailure { record, error }),
        }
    }
    verified.outcome = if !verified.failed.is_empty() {
        CodebaseMemoryRetentionOutcome::PartialFailure
    } else if verified.no_op_reason.is_some() {
        verified.outcome
    } else {
        CodebaseMemoryRetentionOutcome::Completed
    };
    verified
}

fn inventory_no_op(
    mut report: CodebaseMemoryRetentionReport,
    records: Vec<CodebaseMemoryProjectRecord>,
    reason: impl Into<String>,
    outcome: CodebaseMemoryRetentionOutcome,
) -> CodebaseMemoryRetentionReport {
    let reason = reason.into();
    report.no_op_reason = Some(reason.clone());
    report.outcome = outcome;
    report.inventory_complete = false;
    report.inventory_record_count = records.len();
    report.preserved.extend(records.iter().map(|record| {
        CodebaseMemoryRetentionRecordResult {
            project: record
                .project
                .clone()
                .unwrap_or_else(|| "<missing>".to_string()),
            repo_path: record.repo_path.clone(),
            estimated_bytes: record.estimated_bytes,
            reason: reason.clone(),
        }
    }));
    report
}

fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

enum Classification {
    Preserve(&'static str),
    Eligible {
        project: String,
        path: PathBuf,
        updated: u64,
        estimated_bytes: Option<u64>,
    },
}

struct EligibleRecord {
    project: String,
    path: PathBuf,
    updated: u64,
    estimated_bytes: Option<u64>,
}

fn classify(
    record: &CodebaseMemoryProjectRecord,
    workspace_root: &Path,
    scope: &CodebaseMemoryRetentionScope,
) -> Classification {
    let project = record
        .project
        .as_deref()
        .expect("inventory completeness checked");
    let path = record
        .repo_path
        .as_ref()
        .expect("inventory completeness checked");
    let updated = record
        .updated_at_unix_secs
        .expect("inventory completeness checked");

    if project.starts_with("temper-v1-") {
        return Classification::Preserve("stable logical repository project");
    }
    if record.indexing_active == Some(true) {
        return Classification::Preserve("provider reports active indexing");
    }
    if !safe_absolute_path(path) || !path.starts_with(workspace_root) {
        return Classification::Preserve("record is outside the canonical workspace root");
    }
    let relative = match path.strip_prefix(workspace_root) {
        Ok(relative) => relative,
        Err(_) => return Classification::Preserve("record path is ambiguous"),
    };
    let parts = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.len() != 3
        || !scope.roles.contains(parts[0])
        || parts[1].is_empty()
        || !scope.repository_dirs.contains(parts[2])
    {
        return Classification::Preserve(
            "record does not match a configured Temper workspace layout",
        );
    }
    if !safe_existing_ancestors(workspace_root, path) {
        return Classification::Preserve("record path has an ambiguous workspace ancestor");
    }
    let owned = match record.ownership.as_deref() {
        Some("temper") => true,
        Some(_) => false,
        None => project == path.to_string_lossy(),
    };
    if !owned {
        return Classification::Preserve("Temper ownership is not verified");
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Classification::Preserve("workspace still exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Classification::Preserve("workspace lifecycle evidence is uncertain"),
    }
    Classification::Eligible {
        project: project.to_string(),
        path: path.clone(),
        updated,
        estimated_bytes: record.estimated_bytes,
    }
}

fn safe_existing_ancestors(workspace_root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(workspace_root) else {
        return false;
    };
    let mut current = workspace_root.to_path_buf();
    for component in relative.components().take(2) {
        let Component::Normal(component) = component else {
            return false;
        };
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) => return false,
        }
    }
    true
}

fn safe_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::RootDir | Component::Prefix(_) | Component::Normal(_)
            )
        })
}

fn record_result(
    record: &CodebaseMemoryProjectRecord,
    reason: impl Into<String>,
) -> CodebaseMemoryRetentionRecordResult {
    CodebaseMemoryRetentionRecordResult {
        project: record
            .project
            .clone()
            .unwrap_or_else(|| "<missing>".to_string()),
        repo_path: record.repo_path.clone(),
        estimated_bytes: record.estimated_bytes,
        reason: reason.into(),
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "codebase_memory_retention/tests.rs"]
mod tests;

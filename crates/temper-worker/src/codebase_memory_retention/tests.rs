// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::collections::BTreeMap;

struct FakeProvider {
    records: BTreeMap<String, CodebaseMemoryProjectRecord>,
    fail: BTreeSet<String>,
    cache_instance: Option<String>,
    deletes: Vec<String>,
}

impl CodebaseMemoryMaintenanceProvider for FakeProvider {
    fn inventory_page(
        &mut self,
        _cursor: Option<&str>,
        _limit: u32,
        _deadline: Instant,
    ) -> Result<CodebaseMemoryProjectPage, String> {
        Ok(CodebaseMemoryProjectPage {
            cache_instance_id: self.cache_instance.clone(),
            cache_bytes: Some(55_000),
            projects: self.records.values().cloned().collect(),
            next_cursor: None,
        })
    }

    fn delete_project(&mut self, project: &str, _deadline: Instant) -> Result<(), String> {
        self.deletes.push(project.to_string());
        if self.fail.contains(project) {
            Err("injected delete failure".to_string())
        } else {
            self.records.remove(project);
            Ok(())
        }
    }
}

fn policy() -> CodebaseMemoryRetentionPolicy {
    CodebaseMemoryRetentionPolicy {
        enabled: true,
        max_obsolete_projects: 1,
        max_age_days: 10,
        maintenance_interval_secs: 60,
        maintenance_timeout_secs: 5,
        inventory_page_size: 50,
        max_inventory_pages: 2,
        max_deletions_per_run: 10,
    }
}

fn record(path: PathBuf, updated: u64) -> CodebaseMemoryProjectRecord {
    CodebaseMemoryProjectRecord {
        project: Some(path.display().to_string()),
        repo_path: Some(path),
        updated_at_unix_secs: Some(updated),
        ownership: None,
        estimated_bytes: Some(128),
    }
}

#[test]
fn retention_deletes_only_verified_obsolete_records_and_isolates_failures() {
    let root = tempfile::tempdir().expect("workspace root");
    let outside_root = tempfile::tempdir().expect("outside root");
    let root = root.path().canonicalize().unwrap();
    let old = root.join("engineer/old/temper");
    let failing = root.join("engineer/failing/temper");
    let newest = root.join("engineer/new/temper");
    let existing = root.join("engineer/active/temper");
    std::fs::create_dir_all(&existing).unwrap();
    let stable = CodebaseMemoryProjectRecord {
        project: Some("temper-v1-stable".to_string()),
        repo_path: Some(root.join("engineer/stable/temper")),
        updated_at_unix_secs: Some(1),
        ownership: Some("temper".to_string()),
        estimated_bytes: Some(256),
    };
    let unrelated = CodebaseMemoryProjectRecord {
        project: Some("other-project".to_string()),
        repo_path: Some(root.join("engineer/other/temper")),
        updated_at_unix_secs: Some(1),
        ownership: None,
        estimated_bytes: None,
    };
    let records = [
        record(old.clone(), 1),
        record(failing.clone(), 2),
        record(newest.clone(), 900_000),
        record(existing, 1),
        stable,
        unrelated,
        record(outside_root.path().join("engineer/old/temper"), 1),
    ]
    .into_iter()
    .map(|record| (record.project.clone().unwrap(), record))
    .collect();
    let mut provider = FakeProvider {
        records,
        fail: BTreeSet::from([failing.display().to_string()]),
        cache_instance: Some("cache-a".to_string()),
        deletes: Vec::new(),
    };
    let scope = CodebaseMemoryRetentionScope {
        workspace_root: root,
        roles: BTreeSet::from(["engineer".to_string()]),
        repository_dirs: BTreeSet::from(["temper".to_string()]),
    };

    let report = maintain_obsolete_codebase_memory_indexes(
        &mut provider,
        policy(),
        &scope,
        1_000_000,
        &|| false,
    );
    assert!(report.inventory_complete);
    assert_eq!(report.candidates.len(), 2);
    assert_eq!(report.deleted.len(), 1);
    assert_eq!(report.deleted_estimated_bytes, Some(128));
    assert_eq!(report.cache_bytes, Some(55_000));
    assert_eq!(
        report.outcome,
        CodebaseMemoryRetentionOutcome::PartialFailure
    );
    assert_eq!(report.deleted[0].project, old.display().to_string());
    assert_eq!(report.failed.len(), 1);
    assert!(
        report
            .preserved
            .iter()
            .any(|item| item.project == "temper-v1-stable")
    );
    assert!(
        report
            .preserved
            .iter()
            .any(|item| item.reason == "workspace still exists")
    );
    assert!(
        report
            .preserved
            .iter()
            .any(|item| item.reason == "Temper ownership is not verified")
    );
    assert!(
        report
            .preserved
            .iter()
            .any(|item| item.reason == "record is outside the canonical workspace root")
    );

    let rerun = maintain_obsolete_codebase_memory_indexes(
        &mut provider,
        policy(),
        &scope,
        1_000_000,
        &|| false,
    );
    assert!(rerun.deleted.is_empty(), "a prior deletion is not repeated");
    assert_eq!(
        rerun.outcome,
        CodebaseMemoryRetentionOutcome::PartialFailure
    );
    assert_eq!(
        rerun.failed.len(),
        1,
        "the isolated failure remains retryable"
    );
    assert_eq!(
        provider
            .deletes
            .iter()
            .filter(|project| *project == &old.display().to_string())
            .count(),
        1
    );
}

#[test]
fn deterministic_order_and_per_run_cap_defer_remaining_candidates() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let paths = [
        root.join("engineer/three/temper"),
        root.join("engineer/one/temper"),
        root.join("engineer/two/temper"),
    ];
    let records = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let record = record(path.clone(), (index + 1) as u64);
            (record.project.clone().unwrap(), record)
        })
        .collect();
    let mut provider = FakeProvider {
        records,
        fail: BTreeSet::new(),
        cache_instance: Some("cache-a".to_string()),
        deletes: Vec::new(),
    };
    let scope = CodebaseMemoryRetentionScope {
        workspace_root: root,
        roles: BTreeSet::from(["engineer".to_string()]),
        repository_dirs: BTreeSet::from(["temper".to_string()]),
    };
    let report = maintain_obsolete_codebase_memory_indexes(
        &mut provider,
        CodebaseMemoryRetentionPolicy {
            max_obsolete_projects: 0,
            max_deletions_per_run: 2,
            ..policy()
        },
        &scope,
        1_000_000,
        &|| false,
    );
    assert_eq!(report.candidates.len(), 3);
    assert_eq!(report.deleted.len(), 2);
    assert_eq!(
        provider.deletes,
        vec![
            paths[0].display().to_string(),
            paths[1].display().to_string()
        ]
    );
    assert!(
        report
            .preserved
            .iter()
            .any(|record| record.reason == "deferred by the per-run deletion cap")
    );
}

#[test]
fn uncertainty_or_active_work_produces_no_deletion() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let candidate = record(root.join("engineer/old/temper"), 1);
    let mut provider = FakeProvider {
        records: BTreeMap::from([(candidate.project.clone().unwrap(), candidate)]),
        fail: BTreeSet::new(),
        cache_instance: None,
        deletes: Vec::new(),
    };
    let scope = CodebaseMemoryRetentionScope {
        workspace_root: root,
        roles: BTreeSet::from(["engineer".to_string()]),
        repository_dirs: BTreeSet::from(["temper".to_string()]),
    };
    let uncertain = maintain_obsolete_codebase_memory_indexes(
        &mut provider,
        policy(),
        &scope,
        1_000_000,
        &|| false,
    );
    assert!(
        uncertain
            .no_op_reason
            .as_deref()
            .unwrap()
            .contains("cache instance")
    );
    assert!(provider.deletes.is_empty());

    provider.cache_instance = Some("cache-a".to_string());
    provider
        .records
        .values_mut()
        .next()
        .unwrap()
        .updated_at_unix_secs = None;
    let incomplete = maintain_obsolete_codebase_memory_indexes(
        &mut provider,
        policy(),
        &scope,
        1_000_000,
        &|| false,
    );
    assert!(
        incomplete
            .no_op_reason
            .as_deref()
            .unwrap()
            .contains("incomplete")
    );
    assert!(provider.deletes.is_empty());

    provider
        .records
        .values_mut()
        .next()
        .unwrap()
        .updated_at_unix_secs = Some(1);
    let active = maintain_obsolete_codebase_memory_indexes(
        &mut provider,
        policy(),
        &scope,
        1_000_000,
        &|| true,
    );
    assert!(active.no_op_reason.as_deref().unwrap().contains("active"));
    assert!(provider.deletes.is_empty());
}

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{CiJobConclusion, CiJobQuery, CiJobStatus, PullRequest, RepositoryId};

use super::{
    ActionsHistorySeedFixture, CiRequestEvidence, LiveActionsHistoryEvidence, RepoFixture,
};
use crate::forgejo_server::ForgejoServer;

pub(super) const TRANSPORT_CAP_BYTES: usize = 16 * 1024 * 1024;
const PROVIDER_PAGE_LIMIT: usize = 50;
const MAX_PROBE_PAGES: usize = 16;
const MIN_SEEDED_RUNS: usize = 51;
const MAX_SEEDED_RUNS: usize = 256;
const MIN_PAYLOAD_BYTES: usize = 64 * 1024;
const MAX_PAYLOAD_BYTES: usize = 96 * 1024;
const MAX_FIXTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
const PROBE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) struct ActionsHistoryCapture {
    pub evidence: LiveActionsHistoryEvidence,
    pub requests: Vec<CiRequestEvidence>,
}

pub(super) async fn exact_green_run(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    pull: &PullRequest,
) -> Result<u64, String> {
    let head_sha = pull
        .head_sha
        .as_deref()
        .ok_or_else(|| "implementation PR has no exact head SHA".to_string())?;
    let listing = forge
        .list_ci_jobs_with_presence(
            repository,
            CiJobQuery {
                pull_request_id: Some(pull.id.clone()),
                ..CiJobQuery::default()
            },
        )
        .await
        .map_err(|error| format!("list exact-head CI jobs: {error}"))?;
    if !listing.matching_ci_present() {
        return Err("the implementation PR has no matching provider run yet".to_string());
    }
    let jobs = listing.into_jobs();
    if jobs.is_empty()
        || jobs.iter().any(|job| {
            job.commit_sha != head_sha
                || job.status != CiJobStatus::Completed
                || job.conclusion != Some(CiJobConclusion::Success)
        })
    {
        return Err(
            "the implementation PR exact-head run is not green and complete yet".to_string(),
        );
    }
    let run_ids = jobs
        .iter()
        .filter_map(|job| job.run_id.as_deref())
        .map(str::parse::<u64>)
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| "exact-head CI returned a non-numeric provider run id".to_string())?;
    let run_ids = run_ids.into_iter().collect::<Vec<_>>();
    let [run_id] = run_ids.as_slice() else {
        return Err(format!(
            "exact-head CI must resolve to one provider run, observed {}",
            run_ids.len()
        ));
    };
    Ok(*run_id)
}

pub(super) fn seed_and_measure(
    server: &ForgejoServer,
    admin_token: &str,
    repo: &RepoFixture,
    target_run_id: u64,
    target_head_sha: &str,
    fixture: &ActionsHistorySeedFixture,
) -> Result<ActionsHistoryCapture, String> {
    validate_fixture(fixture)?;
    if !server.base_url().starts_with("http://127.0.0.1:") {
        return Err(
            "oversized Actions history is restricted to disposable loopback Forgejo".to_string(),
        );
    }
    let database = server.data_dir().join("data/forgejo.db");
    if !database.is_file() {
        return Err("disposable Forgejo SQLite database is unavailable".to_string());
    }
    seed_rows(
        &database,
        target_run_id,
        target_head_sha,
        fixture.seeded_runs,
        fixture.payload_bytes,
    )?;
    measure_inventory(server.base_url(), admin_token, repo, target_run_id, fixture)
}

fn validate_fixture(fixture: &ActionsHistorySeedFixture) -> Result<(), String> {
    let count_bounded = (MIN_SEEDED_RUNS..=MAX_SEEDED_RUNS).contains(&fixture.seeded_runs);
    let payload_bounded = (MIN_PAYLOAD_BYTES..=MAX_PAYLOAD_BYTES).contains(&fixture.payload_bytes);
    let inventory_bytes = fixture
        .seeded_runs
        .checked_mul(fixture.payload_bytes)
        .ok_or_else(|| "oversized Actions fixture byte bound overflow".to_string())?;
    if !count_bounded
        || !payload_bounded
        || fixture.timeout.is_zero()
        || fixture.timeout > MAX_FIXTURE_TIMEOUT
        || inventory_bytes <= TRANSPORT_CAP_BYTES
    {
        return Err("oversized Actions fixture parameters escaped their closed bounds".to_string());
    }
    Ok(())
}

fn seed_rows(
    database: &std::path::Path,
    target_run_id: u64,
    target_head_sha: &str,
    seeded_runs: usize,
    payload_bytes: usize,
) -> Result<(), String> {
    let target_run_id = i64::try_from(target_run_id)
        .map_err(|_| "exact-head Actions run id exceeds SQLite range".to_string())?;
    let mut connection = Connection::open(database)
        .map_err(|error| format!("open disposable Forgejo Actions database: {error}"))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("configure disposable Forgejo database timeout: {error}"))?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("begin bounded Actions history seed: {error}"))?;
    let target = transaction
        .query_row(
            "SELECT repo_id, commit_sha FROM action_run WHERE id = ?1",
            [target_run_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| format!("read exact-head Actions run fixture source: {error}"))?
        .ok_or_else(|| "exact-head Actions run disappeared before history seeding".to_string())?;
    if target.1 != target_head_sha {
        return Err("exact-head Actions run changed before history seeding".to_string());
    }
    let current_max = transaction
        .query_row(
            "SELECT COALESCE(MAX(`index`), 0) FROM action_run WHERE repo_id = ?1",
            [target.0],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("read Actions run index bound: {error}"))?;
    let payload = synthetic_payload(payload_bytes)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock predates the Unix epoch".to_string())?
        .as_secs();
    let now =
        i64::try_from(now).map_err(|_| "fixture timestamp exceeds SQLite range".to_string())?;
    let mut insert = transaction
        .prepare(
            "INSERT INTO action_run (title, repo_id, owner_id, workflow_id, workflow_directory, `index`, trigger_user_id, schedule_id, ref, commit_sha, event, event_payload, trigger_event, status, version, started, stopped, previous_duration, created, updated, notify_email, is_fork_pull_request, pull_request_poster_id, pull_request_id, need_approval, approved_by, concurrency_group, concurrency_type, pre_execution_error, pre_execution_error_code, pre_execution_error_details) \
             SELECT ?1, repo_id, owner_id, workflow_id, workflow_directory, ?2, trigger_user_id, 0, ?3, ?4, 'push', ?5, 'push', 1, 0, ?6, ?6, 0, ?6, ?6, 0, 0, 0, 0, 0, 0, ?7, 2, '', 0, 'null' FROM action_run WHERE id = ?8 AND repo_id = ?9",
        )
        .map_err(|error| format!("prepare bounded Actions history seed: {error}"))?;
    for offset in 1..=seeded_runs {
        let offset = i64::try_from(offset).expect("bounded seed count fits i64");
        let index = current_max
            .checked_add(offset)
            .ok_or_else(|| "Actions run fixture index overflow".to_string())?;
        let commit_sha = format!("{index:040x}");
        let inserted = insert
            .execute(params![
                "bounded oversized-history fixture",
                index,
                "refs/heads/temper-fixture-actions-history",
                commit_sha,
                payload,
                now,
                format!("temper_fixture_actions_history_{index}"),
                target_run_id,
                target.0,
            ])
            .map_err(|error| format!("insert bounded Actions history row: {error}"))?;
        if inserted != 1 {
            return Err("bounded Actions history seed did not insert exactly one row".to_string());
        }
    }
    drop(insert);
    let final_index = current_max
        .checked_add(i64::try_from(seeded_runs).expect("bounded seed count fits i64"))
        .ok_or_else(|| "Actions run fixture index overflow".to_string())?;
    let updated = transaction
        .execute(
            "UPDATE action_run_index SET max_index = ?1 WHERE group_id = ?2",
            params![final_index, target.0],
        )
        .map_err(|error| format!("advance disposable Actions run index: {error}"))?;
    if updated != 1 {
        return Err("disposable Actions run index row is unavailable".to_string());
    }
    transaction
        .commit()
        .map_err(|error| format!("commit bounded Actions history seed: {error}"))
}

fn synthetic_payload(payload_bytes: usize) -> Result<String, String> {
    const PREFIX: &str = "{\"fixture_padding\":\"";
    const SUFFIX: &str = "\"}";
    let padding = payload_bytes
        .checked_sub(PREFIX.len() + SUFFIX.len())
        .ok_or_else(|| "Actions fixture payload is too small".to_string())?;
    Ok(format!("{PREFIX}{}{SUFFIX}", "x".repeat(padding)))
}

fn measure_inventory(
    base_url: &str,
    admin_token: &str,
    repo: &RepoFixture,
    target_run_id: u64,
    fixture: &ActionsHistorySeedFixture,
) -> Result<ActionsHistoryCapture, String> {
    let client = temper_engine_io::http::BlockingJsonClient::new();
    let path = format!("/api/v1/repos/{}/actions/runs", repo.slug);
    let mut requests = Vec::new();
    let mut lower_bound = 0usize;
    let mut largest_page = 0usize;
    let mut target_page = None;
    let mut pages_observed = 0usize;
    for page in 1..=MAX_PROBE_PAGES {
        let endpoint = format!("{base_url}{path}?page={page}&limit={PROVIDER_PAGE_LIMIT}");
        let response = client
            .send_with_timeout(
                "GET",
                endpoint,
                Some(admin_token),
                None,
                PROBE_REQUEST_TIMEOUT,
            )
            .map_err(|error| format!("read bounded Actions inventory page {page}: {error}"))?;
        requests.push(CiRequestEvidence {
            method: "GET".to_string(),
            path: path.clone(),
            query_keys: vec!["page".to_string(), "limit".to_string()],
            authentication_present: true,
            authentication_scheme: Some("token".to_string()),
            accepts_json: true,
        });
        if response.status != 200 {
            return Err(format!(
                "bounded Actions inventory page {page} returned status {}",
                response.status
            ));
        }
        if response.body.len() >= TRANSPORT_CAP_BYTES {
            return Err(format!(
                "bounded Actions inventory page {page} reached the HTTP transport cap"
            ));
        }
        largest_page = largest_page.max(response.body.len());
        pages_observed = page;
        let document: Value = serde_json::from_slice(&response.body)
            .map_err(|_| format!("bounded Actions inventory page {page} was not JSON"))?;
        let rows = document
            .get("workflow_runs")
            .or_else(|| document.get("runs"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                format!("bounded Actions inventory page {page} omitted its run array")
            })?;
        if rows.len() > PROVIDER_PAGE_LIMIT {
            return Err(format!(
                "bounded Actions inventory page {page} exceeded {PROVIDER_PAGE_LIMIT} rows"
            ));
        }
        for row in rows {
            lower_bound = lower_bound
                .checked_add(
                    serde_json::to_vec(row)
                        .map_err(|_| "serialize bounded Actions row fact".to_string())?
                        .len(),
                )
                .ok_or_else(|| "Actions inventory lower-bound overflow".to_string())?;
            if row.get("id").and_then(Value::as_u64) == Some(target_run_id) {
                target_page = Some(page);
            }
        }
        if rows.len() < PROVIDER_PAGE_LIMIT {
            break;
        }
    }
    let target_run_page = target_page
        .ok_or_else(|| "bounded paged inventory did not retain the exact-head run".to_string())?;
    if pages_observed == MAX_PROBE_PAGES {
        return Err("bounded Actions inventory exhausted its page ceiling".to_string());
    }
    if lower_bound <= TRANSPORT_CAP_BYTES {
        return Err(format!(
            "full Actions inventory lower bound {lower_bound} did not exceed transport cap {TRANSPORT_CAP_BYTES}"
        ));
    }
    if target_run_page <= 1 {
        return Err("seeded history did not move the exact-head run beyond page one".to_string());
    }
    Ok(ActionsHistoryCapture {
        evidence: LiveActionsHistoryEvidence {
            seeded_run_count: fixture.seeded_runs,
            payload_bytes_per_run: fixture.payload_bytes,
            transport_cap_bytes: TRANSPORT_CAP_BYTES,
            full_inventory_lower_bound_bytes: lower_bound,
            largest_paged_response_bytes: largest_page,
            pages_observed,
            target_run_page,
            later_page_selection: true,
            provenance_drop_count: 0,
        },
        requests,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_payload_is_exactly_bounded_and_contains_no_provider_record() {
        let payload = synthetic_payload(90_000).expect("bounded payload");
        assert_eq!(payload.len(), 90_000);
        assert!(serde_json::from_str::<Value>(&payload).is_ok());
        for sentinel in ["secret-token", "Authorization", "provider-record"] {
            assert!(!payload.contains(sentinel));
        }
    }

    #[test]
    fn sqlite_seed_is_exactly_bounded_and_advances_the_provider_index() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = directory.path().join("forgejo.db");
        let connection = Connection::open(&database).expect("fixture database");
        connection
            .execute_batch(
                "CREATE TABLE action_run (id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, title TEXT NULL, repo_id INTEGER NULL, owner_id INTEGER NULL, workflow_id TEXT NULL, workflow_directory TEXT DEFAULT '.forgejo/workflows' NOT NULL, `index` INTEGER NULL, trigger_user_id INTEGER NULL, schedule_id INTEGER NULL, ref TEXT NULL, commit_sha TEXT NULL, event TEXT NULL, event_payload TEXT NULL, trigger_event TEXT NULL, status INTEGER NULL, version INTEGER DEFAULT 0 NULL, started INTEGER NULL, stopped INTEGER NULL, previous_duration INTEGER NULL, created INTEGER NULL, updated INTEGER NULL, notify_email INTEGER NULL, is_fork_pull_request INTEGER NULL, pull_request_poster_id INTEGER NULL, pull_request_id INTEGER NULL, need_approval INTEGER NULL, approved_by INTEGER NULL, concurrency_group TEXT NULL, concurrency_type INTEGER NULL, pre_execution_error TEXT NULL, pre_execution_error_code INTEGER NULL, pre_execution_error_details TEXT NULL); \
                 CREATE UNIQUE INDEX action_run_repo_index ON action_run (repo_id, `index`); \
                 CREATE TABLE action_run_index (group_id INTEGER PRIMARY KEY NOT NULL, max_index INTEGER NULL); \
                 INSERT INTO action_run (id, repo_id, owner_id, workflow_id, `index`, trigger_user_id, commit_sha) VALUES (7, 3, 2, 'ci.yml', 4, 1, 'exact-head'); \
                 INSERT INTO action_run_index (group_id, max_index) VALUES (3, 4);",
            )
            .expect("fixture schema");
        drop(connection);

        seed_rows(&database, 7, "exact-head", 3, 100).expect("bounded seed");

        let connection = Connection::open(database).expect("reopen fixture database");
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM action_run", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT max_index FROM action_run_index WHERE group_id = 3",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            7
        );
        let payload_lengths = connection
            .prepare("SELECT length(event_payload) FROM action_run WHERE id != 7 ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(payload_lengths, vec![100, 100, 100]);
    }

    #[test]
    fn runtime_fixture_bounds_reject_programmatic_escape() {
        let fixture = ActionsHistorySeedFixture {
            repo_id: "service".to_string(),
            source_issue_id: "intake".to_string(),
            seeded_runs: 51,
            payload_bytes: 64 * 1024,
            timeout: std::time::Duration::from_secs(181),
        };
        assert!(validate_fixture(&fixture).is_err());
    }
}

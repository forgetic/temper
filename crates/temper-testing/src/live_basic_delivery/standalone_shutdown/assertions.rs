use super::*;

pub(super) fn wait_for_attempt(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    different_from: Option<&str>,
    standalone: &mut ChildGuard,
    timeout: Duration,
) -> Result<AttemptIdentity, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = standalone.try_wait()? {
            return Err(format!(
                "{} exited while waiting for durable assignment with {status}\n{}",
                standalone.label,
                standalone.log_tail()
            ));
        }
        if let Some(attempt) = current_attempt(forge, repository, issue)? {
            if different_from.is_none_or(|old| attempt.attempt_id != old) {
                return Ok(attempt);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for durable assignment different_from={different_from:?}\n{}",
                standalone.log_tail()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn current_attempt(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
) -> Result<Option<AttemptIdentity>, String> {
    let issue = engine_block_on(forge.get_issue_by_number(repository, issue))
        .map_err(|error| format!("read assignment issue: {error}"))?
        .ok_or_else(|| "assignment issue disappeared".to_string())?;
    let metadata = parse_metadata_block(&issue.body)
        .map_err(|error| format!("parse assignment metadata: {error}"))?;
    let Some(assignment) = metadata.and_then(|metadata| metadata.assignment) else {
        return Ok(None);
    };
    let field = |value: Option<String>, name: &str| {
        value.ok_or_else(|| format!("durable assignment is missing {name}"))
    };
    Ok(Some(AttemptIdentity {
        worker_id: field(assignment.worker_id, "worker_id")?,
        job_id: field(assignment.job_id, "job_id")?,
        attempt_id: field(assignment.attempt_id, "attempt_id")?,
        daemon_boot_id: field(assignment.daemon_boot_id, "daemon_boot_id")?,
    }))
}

pub(super) fn forge_snapshot(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    number: ItemNumber,
) -> Result<ForgeSnapshot, String> {
    engine_block_on(async {
        let issue = forge
            .get_issue_by_number(repository, number)
            .await
            .map_err(|error| format!("read source issue: {error}"))?
            .ok_or_else(|| "source issue disappeared".to_string())?;
        let mut labels = issue.labels.clone();
        labels.sort();
        let mut assignees = issue
            .assignees
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assignees.sort();
        let comments = forge
            .list_issue_comments(&issue.id)
            .await
            .map_err(|error| format!("list source comments: {error}"))?
            .into_iter()
            .map(|comment| comment.body)
            .collect();
        let mut pull_requests = forge
            .list_pull_requests(repository, PullRequestQuery::default())
            .await
            .map_err(|error| format!("list pull requests: {error}"))?
            .into_iter()
            .map(|pull| pull.number.get())
            .collect::<Vec<_>>();
        pull_requests.sort_unstable();
        Ok(ForgeSnapshot {
            body: issue.body,
            labels,
            assignees,
            comments,
            pull_requests,
        })
    })
}

pub(super) fn assert_old_protocol_rejected(
    bind_port: u16,
    worker_token: &str,
    repo: &str,
    issue: ItemNumber,
    old: &AttemptIdentity,
) -> Result<(), String> {
    let result = WorkerProtocolMessage::Result(JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: old.worker_id.clone(),
        job_id: old.job_id.clone(),
        attempt_id: Some(old.attempt_id.clone()),
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: None,
        title: Some("late old-attempt result".to_string()),
        body: Some("this stale result must never be applied".to_string()),
        children: Vec::new(),
        failure: None,
        summary: Some("late old-attempt result".to_string()),
        details: None,
    });
    let result_reply = post_protocol(bind_port, worker_token, &result)?;
    let WorkerProtocolMessage::Release(release) = result_reply else {
        return Err(format!(
            "old result did not receive a release: {result_reply:?}"
        ));
    };
    if release.disposition != ReleaseDisposition::Superseded {
        return Err(format!(
            "old result release was {:?}, expected superseded",
            release.disposition
        ));
    }

    let context = WorkerProtocolMessage::FetchContext(FetchContext {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: old.worker_id.clone(),
        job_id: old.job_id.clone(),
        attempt_id: Some(old.attempt_id.clone()),
        operation: ForgeContextOperation::ForgeGetItem(ForgeGetItemOperation {
            repo: repo.to_string(),
            number: issue.get(),
            artifact_type: None,
            include_comments: true,
        }),
    });
    let context_reply = post_protocol(bind_port, worker_token, &context)?;
    let WorkerProtocolMessage::ContextResponse(context) = context_reply else {
        return Err(format!(
            "old context did not receive a context response: {context_reply:?}"
        ));
    };
    if context.outcome
        != (ContextOutcome::Error {
            code: ForgeContextErrorCode::NotAuthorized,
        })
    {
        return Err(format!(
            "old attempt context was not rejected: {:?}",
            context.outcome
        ));
    }
    Ok(())
}

fn post_protocol(
    bind_port: u16,
    worker_token: &str,
    message: &WorkerProtocolMessage,
) -> Result<WorkerProtocolMessage, String> {
    let auth = WorkerAuth::bearer(worker_token);
    let response = engine_block_on(temper_engine_io::http::http_call(
        &temper_engine_io::http::build_http_client(),
        temper_engine_io::http::HttpCall {
            method: "POST".to_string(),
            url: format!("http://127.0.0.1:{bind_port}/v1/message"),
            headers: vec![
                ("content-type".to_string(), "application/json".to_string()),
                (
                    WORKER_AUTHORIZATION_HEADER.to_string(),
                    auth.authorization_header_value(),
                ),
            ],
            body: serde_json::to_vec(message)
                .map_err(|error| format!("serialize protocol message: {error}"))?,
        },
    ))?;
    if response.status != 200 {
        return Err(format!(
            "protocol message returned HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ));
    }
    serde_json::from_slice(&response.body)
        .map_err(|error| format!("decode protocol response: {error}"))
}

pub(super) fn wait_for_replacement_pr(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    standalone: &mut ChildGuard,
    timeout: Duration,
) -> Result<usize, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = standalone.try_wait()? {
            return Err(format!(
                "replacement standalone exited before result application with {status}\n{}",
                standalone.log_tail()
            ));
        }
        let pulls =
            engine_block_on(forge.list_pull_requests(repository, PullRequestQuery::default()))
                .map_err(|error| format!("list replacement pull requests: {error}"))?;
        let implementation = pulls
            .iter()
            .filter(|pull| pull.labels.iter().any(|label| label == "implementation"))
            .collect::<Vec<_>>();
        if implementation.len() == 1 && implementation[0].body.contains(REPLACEMENT_SUMMARY) {
            let source = engine_block_on(forge.get_issue_by_number(repository, issue))
                .map_err(|error| format!("read source after replacement: {error}"))?
                .ok_or_else(|| "source issue disappeared after replacement".to_string())?;
            let assignment = parse_metadata_block(&source.body)
                .map_err(|error| format!("parse source after replacement: {error}"))?
                .and_then(|metadata| metadata.assignment);
            if assignment.is_none() {
                return Ok(implementation.len());
            }
        }
        if implementation.len() > 1 {
            return Err(format!(
                "startup recovery created {} implementation PRs, expected one",
                implementation.len()
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for sole replacement PR\n{}",
                standalone.log_tail()
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub(super) fn shutdown_blocker(
    log: &Path,
    old: &AttemptIdentity,
    expected_root_pid: u32,
    obstruction_interval: Duration,
) -> Result<ShutdownBlockerEvidence, String> {
    let events = json_log_events(log)?;
    let (summary_index, summary) = events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, fields)| field_string(fields, "event") == Some("standalone.shutdown.summary"))
        .ok_or_else(|| {
            format!(
                "missing standalone shutdown summary\n{}",
                read_tail(log, 200)
            )
        })?;
    let disposition = required_string(summary, "disposition")?;
    if disposition != "bounded_crash_handoff" {
        return Err(format!(
            "shutdown disposition was {disposition:?}, expected bounded_crash_handoff"
        ));
    }
    // Select the final matching detail event before the terminal summary. This
    // avoids accepting an earlier throttled observation when shutdown emitted
    // a newer age, phase, or escalation stage for the same process owner.
    let fields = events[..summary_index]
        .iter()
        .rev()
        .find(|fields| {
            field_string(fields, "event") == Some("standalone.shutdown.blocker")
                && field_string(fields, "blocker_kind") == Some("containment")
                && field_string(fields, "attempt_id") == Some(old.attempt_id.as_str())
        })
        .ok_or_else(|| format!("missing exact containment blocker\n{}", read_tail(log, 240)))?;
    let evidence = ShutdownBlockerEvidence {
        worker_id: required_string(fields, "worker_id")?,
        job_id: required_string(fields, "job_id")?,
        attempt_id: required_string(fields, "attempt_id")?,
        kind: required_string(fields, "blocker_kind")?,
        owner_scope: required_string(fields, "owner_scope")?,
        owner_name: required_string(fields, "owner_name")?,
        owner_root: required_string(fields, "owner_root")?,
        root_pid: required_u64(fields, "root_pid")?
            .try_into()
            .map_err(|_| "blocker root_pid exceeds u32".to_string())?,
        containment_phase: required_string(fields, "containment_phase")?,
        first_seen_millis: required_u64(fields, "first_seen_millis")?,
        age_millis: required_u64(fields, "age_millis")?,
        escalation_stage: required_string(fields, "escalation_stage")?,
        deadline_remaining_millis: required_u64(fields, "deadline_remaining_millis")?,
        disposition,
    };
    if evidence.worker_id != old.worker_id
        || evidence.job_id != old.job_id
        || evidence.attempt_id != old.attempt_id
    {
        return Err(format!("shutdown blocker identity mismatch: {evidence:?}"));
    }
    if evidence.owner_scope == "unknown"
        || evidence.owner_name == "unknown"
        || evidence.owner_root == "unknown"
    {
        return Err(format!(
            "shutdown blocker omitted its process owner: {evidence:?}"
        ));
    }
    if evidence.root_pid != expected_root_pid {
        return Err(format!(
            "shutdown blocker root PID {} != recorded Temper supervisor {}",
            evidence.root_pid, expected_root_pid
        ));
    }
    // The Linux supervisor's owner-side discovery request includes the
    // helper's recursive-empty proof, so stopping the helper may retain either
    // `discover` or `verify_empty` depending on which side reached the
    // obstruction first. It must never erase that live phase as `unknown`.
    if !["discover", "term", "grace", "kill", "reap", "verify_empty"]
        .contains(&evidence.containment_phase.as_str())
    {
        return Err(format!(
            "obstructed recursive-empty blocker reported unknown phase {:?}",
            evidence.containment_phase
        ));
    }
    let maximum_obstruction_age = u64::try_from(
        obstruction_interval
            .saturating_add(Duration::from_millis(500))
            .as_millis(),
    )
    .unwrap_or(u64::MAX);
    if evidence.first_seen_millis == 0
        || evidence.age_millis == 0
        || evidence.age_millis > maximum_obstruction_age
        || evidence.age_millis > SHUTDOWN_BUDGET.as_millis() as u64
        || evidence.escalation_stage != "emergency_kill"
        || evidence.deadline_remaining_millis == 0
        || evidence.deadline_remaining_millis > SHUTDOWN_BUDGET.as_millis() as u64
    {
        return Err(format!(
            "shutdown blocker timing/escalation is invalid: {evidence:?}"
        ));
    }
    assert_summary_contains_blocker(summary, &evidence)?;
    Ok(evidence)
}

fn assert_summary_contains_blocker(
    summary: &JsonValue,
    expected: &ShutdownBlockerEvidence,
) -> Result<(), String> {
    let encoded = required_string(summary, "blockers")?;
    let blockers: Vec<JsonValue> = serde_json::from_str(&encoded)
        .map_err(|error| format!("decode shutdown summary blockers: {error}"))?;
    let included = blockers.iter().any(|blocker| {
        field_string(blocker, "kind") == Some(expected.kind.as_str())
            && field_string(blocker, "worker_id") == Some(expected.worker_id.as_str())
            && field_string(blocker, "job_id") == Some(expected.job_id.as_str())
            && field_string(blocker, "attempt_id") == Some(expected.attempt_id.as_str())
            && blocker.get("root_pid").and_then(JsonValue::as_u64)
                == Some(u64::from(expected.root_pid))
            && field_string(blocker, "containment_phase")
                == Some(expected.containment_phase.as_str())
            && blocker.get("first_seen_millis").and_then(JsonValue::as_u64)
                == Some(expected.first_seen_millis)
            && blocker.get("age_millis").and_then(JsonValue::as_u64) == Some(expected.age_millis)
    });
    included.then_some(()).ok_or_else(|| {
        format!("terminal shutdown summary omitted the asserted containment blocker: {summary}")
    })
}

fn json_log_events(path: &Path) -> Result<Vec<JsonValue>, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("read JSON log {}: {error}", path.display()))?;
    Ok(source
        .lines()
        .filter_map(|line| serde_json::from_str::<JsonValue>(line).ok())
        .filter_map(|event| event.get("fields").cloned())
        .collect())
}

fn field_string<'a>(fields: &'a JsonValue, name: &str) -> Option<&'a str> {
    fields.get(name).and_then(JsonValue::as_str)
}

fn required_string(fields: &JsonValue, name: &str) -> Result<String, String> {
    field_string(fields, name)
        .map(str::to_string)
        .ok_or_else(|| format!("structured shutdown event is missing string `{name}`: {fields}"))
}

fn required_u64(fields: &JsonValue, name: &str) -> Result<u64, String> {
    fields
        .get(name)
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| format!("structured shutdown event is missing integer `{name}`: {fields}"))
}

pub(super) fn wait_for_path(
    path: &Path,
    timeout: Duration,
    description: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(format!(
        "timed out waiting for {description} marker {}",
        path.display()
    ))
}

pub(super) fn require_executable(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{label} {} is not a file", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(path)
            .map_err(|error| format!("inspect {label} {}: {error}", path.display()))?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(format!("{label} {} is not executable", path.display()));
        }
    }
    Ok(())
}

pub(super) fn signal_pid(pid: u32, signal: &str) -> Result<(), String> {
    let status = std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()
        .map_err(|error| format!("send {signal} to PID {pid}: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("send {signal} to PID {pid} failed with {status}"))
}

pub(super) fn exit_status(status: &ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        match (status.code(), status.signal()) {
            (Some(code), _) => format!("exit:{code}"),
            (_, Some(signal)) => format!("signal:{signal}"),
            _ => status.to_string(),
        }
    }
    #[cfg(not(unix))]
    status.to_string()
}

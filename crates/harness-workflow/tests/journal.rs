//! Tests for command journaling (Phase 7).
//!
//! These cover the in-memory journal contract, the simulated-restart behaviour
//! that makes interrupted commands recoverable, and the executor's journaled
//! lifecycle against the deterministic in-memory backend.

mod support;

use harness_workflow::{
    ArtifactSource, CommandId, CommandJournal, CommandRecord, CommandState, ExecutionError,
    Executor, InMemoryJournal, RoleId, TransitionId, WorkflowEffect,
};
use support::{block_on, create_issue, new_repo, ts, workflow, TestRoot};

fn planned(id: &str, number: u64) -> CommandRecord {
    CommandRecord::planned(
        CommandId::new(id),
        ArtifactSource::Issue {
            number: harness_forge::ItemNumber::new(number),
        },
        TransitionId::new("claim_code"),
        RoleId::new("engineer"),
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ],
        ts("2026-05-29T00:00:00Z"),
    )
}

#[test]
fn append_get_and_list_preserve_order() {
    let journal = InMemoryJournal::new();
    block_on(journal.append(planned("cmd-1", 1))).expect("first append");
    block_on(journal.append(planned("cmd-2", 2))).expect("second append");

    let listed = block_on(journal.list()).expect("list");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, CommandId::new("cmd-1"));
    assert_eq!(listed[1].id, CommandId::new("cmd-2"));

    let fetched = block_on(journal.get(&CommandId::new("cmd-2")))
        .expect("get")
        .expect("cmd-2 exists");
    assert_eq!(fetched.transition, Some(TransitionId::new("claim_code")));
}

#[test]
fn append_is_idempotent_on_command_id() {
    let journal = InMemoryJournal::new();
    block_on(journal.append(planned("cmd-1", 1))).expect("first append");
    // Move it forward, then re-append the same id: the original record stands.
    block_on(journal.transition_state(
        &CommandId::new("cmd-1"),
        CommandState::Applying,
        None,
        ts("2026-05-29T00:01:00Z"),
    ))
    .expect("advance to applying");
    block_on(journal.append(planned("cmd-1", 1))).expect("re-append is a no-op");

    let listed = block_on(journal.list()).expect("list");
    assert_eq!(listed.len(), 1, "no duplicate record was created");
    assert_eq!(
        listed[0].state,
        CommandState::Applying,
        "state was not reset"
    );
}

#[test]
fn transition_state_updates_state_detail_and_timestamp() {
    let journal = InMemoryJournal::new();
    block_on(journal.append(planned("cmd-1", 1))).expect("append");

    block_on(journal.transition_state(
        &CommandId::new("cmd-1"),
        CommandState::Failed,
        Some("backend exploded".into()),
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("transition");

    let record = block_on(journal.get(&CommandId::new("cmd-1")))
        .expect("get")
        .expect("exists");
    assert_eq!(record.state, CommandState::Failed);
    assert_eq!(record.detail.as_deref(), Some("backend exploded"));
    assert_eq!(record.updated_at, ts("2026-05-29T00:05:00Z"));
    assert_eq!(record.created_at, ts("2026-05-29T00:00:00Z"));
}

#[test]
fn transition_state_on_unknown_command_is_not_found() {
    let journal = InMemoryJournal::new();
    let error = block_on(journal.transition_state(
        &CommandId::new("ghost"),
        CommandState::Completed,
        None,
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect_err("unknown command");
    assert!(matches!(
        error,
        harness_workflow::JournalError::NotFound { .. }
    ));
}

#[test]
fn incomplete_command_is_recognized_after_restart() {
    // A worker journals a command and starts applying it, then "crashes": no
    // terminal state is recorded.
    let journal = InMemoryJournal::new();
    block_on(journal.append(planned("cmd-1", 1))).expect("append");
    block_on(journal.transition_state(
        &CommandId::new("cmd-1"),
        CommandState::Applying,
        None,
        ts("2026-05-29T00:01:00Z"),
    ))
    .expect("advance to applying");
    block_on(journal.append(planned("cmd-2", 2))).expect("a second command");
    block_on(journal.transition_state(
        &CommandId::new("cmd-2"),
        CommandState::Completed,
        None,
        ts("2026-05-29T00:02:00Z"),
    ))
    .expect("complete the second");

    // Reconstruction: a fresh handle attaches to the same durable store.
    let restarted = journal.clone();
    let incomplete = block_on(restarted.incomplete()).expect("incomplete");
    assert_eq!(
        incomplete.len(),
        1,
        "only the mid-flight command is incomplete"
    );
    assert_eq!(incomplete[0].id, CommandId::new("cmd-1"));
    assert_eq!(incomplete[0].state, CommandState::Applying);
    assert_eq!(
        incomplete[0].effects,
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ],
        "the recorded intent survives the restart"
    );
}

#[test]
fn execute_journaled_records_a_completed_lifecycle() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "");
    let journal = InMemoryJournal::new();
    let executor = Executor::new(&workflow, &forge);

    let report = block_on(executor.execute_journaled(
        &journal,
        CommandId::new("claim-1"),
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("claim_code"),
        &RoleId::new("engineer"),
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("a ready code issue can be claimed");
    assert_eq!(report.transition, TransitionId::new("claim_code"));

    let record = block_on(journal.get(&CommandId::new("claim-1")))
        .expect("get")
        .expect("the command was journaled");
    assert_eq!(record.state, CommandState::Completed);
    assert_eq!(
        record.effects,
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ],
        "the journal recorded the intended effects before applying"
    );
}

#[test]
fn execute_journaled_does_not_journal_planning_failures() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    // Already in progress: claiming is a stale precondition, so planning fails.
    let number = create_issue(&forge, &repo, &["code", "in-progress"], "");
    let journal = InMemoryJournal::new();
    let executor = Executor::new(&workflow, &forge);

    let error = block_on(executor.execute_journaled(
        &journal,
        CommandId::new("claim-1"),
        &repo,
        ArtifactSource::Issue { number },
        &TransitionId::new("claim_code"),
        &RoleId::new("engineer"),
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect_err("a stale claim cannot be planned");
    assert!(matches!(error, ExecutionError::Precondition { .. }));

    // Nothing was attempted, so nothing was journaled: there is no command to
    // recover.
    let listed = block_on(journal.list()).expect("list");
    assert!(listed.is_empty());
}

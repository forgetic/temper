//! Behavior tests for the deterministic **basic-delivery** fake agents.
//!
//! basic-delivery is the minimal, no-human-in-the-loop shape: the architect
//! rewrites an `untriaged` intake issue into a `code` + `ready` issue with a
//! crisp body (a single `triage_intake_to_code` outcome — no design/breakdown
//! branch, no fan-out), and the engineer turns that ready code issue into an
//! open `implementation` PR through a single `open_pr` transition (no explicit
//! `claim_code`/`request_review` sequence). No reviewer/owner/human participate.
//!
//! These mirror `temper-runner/tests/fake_agents.rs` at basic-delivery scope and
//! run against the in-memory backend, so they assert the fakes' Forge effects
//! without a live Forgejo run.

use std::sync::Arc;
use temper_forge_memory::MemoryForge;
use temper_forge_model::{Forge, IssueState, PullRequestState, UserId};
use temper_runner::{Progress, RoleWorker, Worker};
use temper_workflow::{RoleId, parse_metadata_block};

use temper_testing::agents::{BasicArchitect, BasicEngineer};
use temper_testing::{
    actor_user, basic_delivery_runner_config, basic_delivery_workflow, block_on, create_issue,
    labels, new_repo, ts,
};

#[test]
fn basic_architect_triages_intake_into_ready_code_with_body() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let intake = create_issue(
        &forge,
        &repo,
        &["untriaged"],
        "ship the widget",
        "Users want a widget on the dashboard.",
    );
    let workflow = basic_delivery_workflow();
    let compiled = workflow.compile();
    let architect_forge = forge.as_user(actor_user("architect"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &architect_forge,
        &repo,
        RoleId::new("architect"),
        Arc::new(BasicArchitect),
        basic_delivery_runner_config().execution_context(&RoleId::new("architect")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    let issue = block_on(forge.get_issue_by_number(&repo, intake))
        .expect("lookup succeeds")
        .expect("issue exists");
    // The single `triage_intake_to_code` outcome drops `untriaged` and stamps
    // `code` + `ready`.
    assert_eq!(labels(issue.labels), vec!["code", "ready"]);
    assert_eq!(issue.state, IssueState::Open);
    // The `set_body` effect rewrote the body to a non-empty crisp spec.
    assert!(
        !issue.body.trim().is_empty(),
        "triaged body must not be empty: {:?}",
        issue.body
    );
    assert!(
        issue.body.contains("ship the widget"),
        "triaged body should reference the intake title: {:?}",
        issue.body
    );
}

#[test]
fn basic_architect_is_idempotent_after_triage() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    create_issue(&forge, &repo, &["untriaged"], "one shot", "do the thing");
    let workflow = basic_delivery_workflow();
    let compiled = workflow.compile();
    let architect_forge = forge.as_user(actor_user("architect"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &architect_forge,
        &repo,
        RoleId::new("architect"),
        Arc::new(BasicArchitect),
        basic_delivery_runner_config().execution_context(&RoleId::new("architect")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );
    // Once triaged, the issue leaves the `triage` queue, so a second tick is a
    // quiet no-op.
    assert_eq!(
        tick(&worker),
        Progress {
            changed: false,
            actions: 0
        }
    );
}

#[test]
fn basic_engineer_opens_implementation_pr_via_open_pr() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let code = create_issue(
        &forge,
        &repo,
        &["code", "ready"],
        "build the widget",
        "## Code spec\n\nImplement the widget.",
    );
    let workflow = basic_delivery_workflow();
    let compiled = workflow.compile();
    let engineer_forge = forge.as_user(actor_user("engineer"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &engineer_forge,
        &repo,
        RoleId::new("engineer"),
        Arc::new(BasicEngineer),
        basic_delivery_runner_config().execution_context(&RoleId::new("engineer")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    // `open_pr` drops `ready`, adds `in-progress`, and assigns the engineer.
    let issue = block_on(forge.get_issue_by_number(&repo, code))
        .expect("lookup succeeds")
        .expect("issue exists");
    assert_eq!(labels(issue.labels), vec!["code", "in-progress"]);
    assert_eq!(issue.assignees, vec![UserId::new("engineer")]);

    // The `create_pull_request` effect opened exactly one `implementation` PR,
    // which (no review gate) lands straight in the `landing` queue.
    let pull_requests =
        block_on(forge.list_pull_requests(&repo, Default::default())).expect("list succeeds");
    assert_eq!(pull_requests.len(), 1);
    let pr = &pull_requests[0];
    assert_eq!(pr.state, PullRequestState::Open);
    assert_eq!(labels(pr.labels.clone()), vec!["implementation"]);
    // The PR records its parent code issue in workflow metadata.
    let metadata = parse_metadata_block(&pr.body)
        .expect("PR metadata parses")
        .expect("PR metadata exists");
    assert!(
        metadata
            .parents
            .iter()
            .any(|parent| parent.is_same_repo() && parent.number == code)
    );
}

#[test]
fn basic_engineer_ignores_unready_code() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    // A `code` issue without `ready` is not in the `code_ready` queue; the
    // engineer must not open a PR for it.
    create_issue(&forge, &repo, &["code", "in-progress"], "in flight", "");
    let workflow = basic_delivery_workflow();
    let compiled = workflow.compile();
    let engineer_forge = forge.as_user(actor_user("engineer"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &engineer_forge,
        &repo,
        RoleId::new("engineer"),
        Arc::new(BasicEngineer),
        basic_delivery_runner_config().execution_context(&RoleId::new("engineer")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: false,
            actions: 0
        }
    );
    assert_eq!(
        block_on(forge.list_pull_requests(&repo, Default::default()))
            .expect("list succeeds")
            .len(),
        0
    );
}

fn tick(worker: &impl Worker) -> Progress {
    block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds")
}

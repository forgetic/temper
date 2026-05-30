//! Tests for the fake CI producer seam.

use harness_forge::{CiJobConclusion, CiJobQuery, Forge};
use harness_forge_memory::MemoryForge;
use harness_runner::{CiWorker, Progress, Worker};
use harness_workflow::ArtifactSource;

use harness_testing::ci::{FixedCiPolicy, MemoryCiSink};
use harness_testing::{block_on, create_pr, new_repo, ts};

#[test]
fn ci_worker_default_policy_records_passing_native_job() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"], "impl", "");
    let worker = CiWorker::new(&forge, &repo, MemoryCiSink::new(forge.clone()));

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    let jobs = block_on(forge.list_ci_jobs(&repo, CiJobQuery::default())).expect("jobs list");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Success));
    let signals = ci_signals(&forge, &repo, number);
    assert!(signals.ci().is_passed());
    assert!(!signals.ci().is_failed());
}

#[test]
fn ci_worker_injected_fail_policy_records_failing_native_job() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"], "impl", "");
    let worker = CiWorker::with_policy(
        &forge,
        &repo,
        MemoryCiSink::new(forge.clone()),
        FixedCiPolicy::fail(),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    let jobs = block_on(forge.list_ci_jobs(&repo, CiJobQuery::default())).expect("jobs list");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].conclusion, Some(CiJobConclusion::Failure));
    let signals = ci_signals(&forge, &repo, number);
    assert!(signals.ci().is_failed());
    assert!(!signals.ci().is_passed());
}

fn ci_signals(
    forge: &MemoryForge,
    repo: &harness_forge::RepositoryId,
    number: harness_forge::ItemNumber,
) -> harness_workflow::GateSignals {
    let workflow = harness_testing::workflow();
    block_on(
        workflow
            .executor(forge)
            .read_gate_signals(repo, ArtifactSource::PullRequest { number }),
    )
    .expect("signals read")
}

fn tick(worker: &impl Worker) -> Progress {
    block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds")
}

//! The referee of the agent's top-level world holds the worker and the agent
//! to what they owe each other, from what the fakes see: here it is fed
//! observations by hand, as the world would feed them, to show that it fails
//! a run that breaks an expectation, and says why.

use std::collections::BTreeMap;

use temper_agent_model::run::outcome::{Change, Declared};
use temper_agent_model::run::{Answer, Spend};
use temper_agent_model_tests::channel;
use temper_agent_model_tests::referee::{Meeting, Report, Repository, Seen};
use temper_checkout_fake::git::Tree;
use temper_lib::{Duration, Time, Token};
use temper_worker_model::host::{Failure, RunFailure};
use temper_world::{Referee, Verdict};

const ATTEMPT: Token = Token::new(1);
const PROCESS: u64 = 4;

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn tree(content: &[u8]) -> Tree {
    BTreeMap::from([(b"src/lib.rs".to_vec(), content.to_vec())])
}

fn change() -> Declared {
    Declared::Change(Change { title: b"Fix the answer".as_slice().into(), body: Box::new([]) })
}

/// A referee that has seen the run of `ATTEMPT` start in `PROCESS`, on a
/// repository it may write, ask to push `pushed`, and accept a change.
fn started(pushed: &[u8]) -> Referee<Meeting> {
    let mut referee = Referee::new(Meeting::new(Duration::from_secs(60)));
    let repository = Repository { name: b"app".to_vec(), remote: b"forge/app".to_vec(), push: Some(b"fix".to_vec()) };
    referee.observe(at(0), Seen::Assigned { attempt: ATTEMPT, repositories: vec![repository] }, &mut Vec::new());
    referee.observe(at(1), Seen::Started { process: PROCESS, attempt: ATTEMPT }, &mut Vec::new());
    referee.observe(
        at(2),
        Seen::Asked { process: PROCESS, trees: vec![(b"app".to_vec(), tree(pushed))] },
        &mut Vec::new(),
    );
    referee.observe(
        at(3),
        Seen::Committed { process: PROCESS, repository: b"app".to_vec(), tree: tree(pushed) },
        &mut Vec::new(),
    );
    referee.observe(
        at(4),
        Seen::Answered { process: PROCESS, answer: Answer::Accepted { outcome: change(), spent: Spend::ZERO } },
        &mut Vec::new(),
    );
    referee
}

/// The worker's report that the run ended with its change, landed as
/// `commit`.
fn ended(commit: u64) -> Seen {
    let report = Report::Ended { outcome: channel::outcome(&change()) };
    Seen::Reported { attempt: ATTEMPT, kind: "ended", report, landed: vec![(0, commit)] }
}

fn why(referee: &Referee<Meeting>) -> String {
    let Verdict::Failed(failure) = referee.verdict() else { panic!("the referee failed the run") };
    failure.why
}

/// The forge moves `branch` to `tip`, bringing `brought`.
fn moved(branch: &[u8], tip: u64, brought: &[u64], content: &[u8]) -> Seen {
    let (remote, branch) = (b"forge/app".to_vec(), branch.to_vec());
    Seen::Moved { remote, branch, tip, brought: brought.to_vec(), tree: tree(content) }
}

#[test]
fn a_change_landed_on_its_branch_as_its_agent_left_it_passes() {
    let mut referee = started(b"43");
    referee.observe(at(5), moved(b"fix", 9, &[9, 1], b"43"), &mut Vec::new());
    // Another party moves the branch on before the engine hears of it: what
    // landed is still an ancestor of its tip.
    referee.observe(at(6), moved(b"fix", 10, &[10], b"43, and more"), &mut Vec::new());
    referee.observe(at(7), ended(9), &mut Vec::new());
    assert_eq!(referee.verdict(), Verdict::Passed);
}

#[test]
fn a_commit_of_another_tree_than_its_agent_left_fails_the_run() {
    let mut referee = started(b"43");
    let committed = Seen::Committed { process: PROCESS, repository: b"app".to_vec(), tree: tree(b"42") };
    referee.observe(at(5), committed, &mut Vec::new());
    assert_eq!(
        why(&referee),
        r#"the worker commits exactly the tree its agent left: { "src/lib.rs": "42" } committed, { "src/lib.rs": "43" } left"#
    );
}

#[test]
fn a_change_said_to_land_where_the_forge_never_took_it_fails_the_run() {
    let mut referee = started(b"43");
    referee.observe(at(5), moved(b"main", 9, &[9], b"43"), &mut Vec::new());
    referee.observe(at(6), moved(b"fix", 8, &[8], b"other"), &mut Vec::new());
    referee.observe(at(7), ended(9), &mut Vec::new());
    assert_eq!(why(&referee), "what landed is on its branch: 9 is not an ancestor of fix's tip, Some(8)");
}

#[test]
fn a_run_that_accepted_its_change_reported_as_failed_fails_the_run() {
    let mut referee = started(b"43");
    let report = Report::Failed(Failure::Run(RunFailure::Model));
    referee.observe(
        at(5),
        Seen::Reported { attempt: ATTEMPT, kind: "run model", report, landed: Vec::new() },
        &mut Vec::new(),
    );
    assert_eq!(why(&referee), "a run that accepted its outcome ends with it, not run model");
}

#[test]
fn an_assignment_left_unanswered_past_its_bound_fails_listing_it() {
    let mut referee = started(b"43");
    assert_eq!(referee.next_deadline(), Some(at(60)), "the answer is due a minute after the assignment");
    referee.fire(at(60), &mut Vec::new());
    assert_eq!(why(&referee), "Answer(1) was not met by 60.000000000s");
}

//! The referee of the agent's top-level world holds the worker and the agent
//! to what they owe each other, from what the fakes see: here it is fed
//! observations by hand, as the world would feed them, to show that it fails
//! a run that breaks an expectation, and says why.

use std::collections::BTreeMap;

use skein_lib::{Duration, Time, Token};
use temper_agent_domain::run::outcome::{Change, Declared};
use temper_agent_domain::run::{Answer, Spend};
use temper_agent_domain_world::desk::{self, CODING, Hand, Work};
use temper_agent_domain_world::referee::{Meeting, POSTED, Report, Repository, STORY, Seen};
use temper_agent_domain_world::{CALM, Job, channel, protocol};
use temper_fake_checkout::git::Tree;
use temper_fake_forge_domain::Observation;
use temper_legacy_engine_domain::forge::Position;
use temper_legacy_engine_domain::work::{Class, Hold, Phase};
use temper_legacy_engine_domain::{Item, Outcome, Posted};
use temper_legacy_engine_domain_world::codec;
use temper_legacy_engine_domain_world::deployment::{ENGINE, REPOSITORIES};
use temper_legacy_engine_forge_world::translate::recorded;
use temper_worker_domain::host::{Failure, RunFailure};
use temper_world::{Referee, Verdict};

/// The first attempt of the issue #7 of the deployment's first repository.
const ITEM: Item = Item { repository: 0, number: 7 };
const ATTEMPT: Token = Token::new(7 << 32 | 1);
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
    let mut referee = Referee::new(Meeting::new(Duration::from_secs(60), false));
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

/// The engine posts `outcome` for the item's attempt `attempt`, as the
/// comment `id`.
fn posted(id: u64, attempt: u64, outcome: Outcome) -> Seen {
    let posted = Posted { attempt, outcome, head: None };
    let body = [b"<!-- temper:key outcome -->\n".as_slice(), &codec::posted_block(&posted)].concat();
    let repository = REPOSITORIES[0].into();
    Seen::Forge(Observation::Commented { repository, number: ITEM.number, id, body: body.into(), by: ENGINE })
}

/// The engine's outcome for the change the run accepted.
fn the_change() -> Outcome {
    channel::engine_outcome(&change())
}

fn why(referee: &Referee<Meeting>) -> String {
    let Verdict::Failed(failure) = referee.verdict() else { panic!("the referee failed the run") };
    failure.why
}

/// The forge moves `branch` to `tip`, bringing `brought`.
fn moved(branch: &[u8], tip: u64, brought: &[u64], content: &[u8]) -> Seen {
    let (remote, branch) = (b"forge/app".to_vec(), branch.to_vec());
    Seen::Moved { remote, branch, tip, brought: brought.to_vec(), tree: tree(content), other: false }
}

#[test]
fn a_change_landed_on_its_branch_as_its_agent_left_it_passes() {
    let mut referee = started(b"43");
    referee.observe(at(5), moved(b"fix", 9, &[9, 1], b"43"), &mut Vec::new());
    // Another party moves the branch on before the engine hears of it: what
    // landed is still an ancestor of its tip.
    referee.observe(at(6), moved(b"fix", 10, &[10], b"43, and more"), &mut Vec::new());
    referee.observe(at(7), ended(9), &mut Vec::new());
    referee.observe(at(8), posted(30, 1, the_change()), &mut Vec::new());
    assert_eq!(referee.verdict(), Verdict::Passed);
}

#[test]
fn the_names_here_are_the_channels() {
    assert_eq!(protocol::attempt(ITEM, 1), ATTEMPT);
}

#[test]
fn an_outcome_the_run_ended_with_left_unposted_past_its_bound_fails_listing_it() {
    let mut referee = started(b"43");
    referee.observe(at(5), moved(b"fix", 9, &[9, 1], b"43"), &mut Vec::new());
    referee.observe(at(7), ended(9), &mut Vec::new());
    let due = at(7).saturating_add(POSTED);
    assert_eq!(referee.next_deadline(), Some(due));
    referee.fire(due, &mut Vec::new());
    assert_eq!(why(&referee), format!("Posted({}) was not met by 3607.000000000s", ATTEMPT.raw()));
}

#[test]
fn an_outcome_posted_for_an_attempt_not_answered_as_ended_fails_the_run() {
    let mut referee = started(b"43");
    referee.observe(at(5), posted(30, 1, the_change()), &mut Vec::new());
    assert_eq!(
        why(&referee),
        "the engine posts an outcome only for an attempt answered as ended: Item { repository: 0, number: 7 }#1, \
         Change { message: [70, 105, 120, 32, 116, 104, 101, 32, 97, 110, 115, 119, 101, 114] }"
    );
}

#[test]
fn an_outcome_posted_unlike_the_one_the_run_ended_with_fails_the_run() {
    let mut referee = started(b"43");
    referee.observe(at(5), moved(b"fix", 9, &[9, 1], b"43"), &mut Vec::new());
    referee.observe(at(7), ended(9), &mut Vec::new());
    referee.observe(at(8), posted(30, 1, Outcome::Report { text: Box::new([]) }), &mut Vec::new());
    assert!(why(&referee).starts_with("the engine posts the outcome the run ended with: "), "{}", why(&referee));
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
    assert_eq!(why(&referee), format!("Answer({}) was not met by 60.000000000s", ATTEMPT.raw()));
}

/// The forge merges `head` into main, with `merged` where the head changed
/// the code.
fn merged(head: u64, merged: &[u8]) -> Seen {
    Seen::Merged {
        remote: b"forge/app".to_vec(),
        base: b"main".to_vec(),
        head,
        changed: tree(b"43"),
        merged: tree(merged),
    }
}

#[test]
fn a_merge_of_what_a_run_landed_keeping_its_change_passes() {
    let mut referee = started(b"43");
    referee.observe(at(5), moved(b"fix", 9, &[9, 1], b"43"), &mut Vec::new());
    referee.observe(at(6), ended(9), &mut Vec::new());
    referee.observe(at(7), merged(9, b"43"), &mut Vec::new());
    referee.observe(at(8), posted(30, 1, the_change()), &mut Vec::new());
    assert_eq!(referee.verdict(), Verdict::Passed);
}

#[test]
fn a_merge_of_a_head_no_run_landed_fails_the_run() {
    let mut referee = started(b"43");
    referee.observe(at(5), moved(b"fix", 9, &[9, 1], b"43"), &mut Vec::new());
    referee.observe(at(6), ended(9), &mut Vec::new());
    referee.observe(at(7), merged(8, b"43"), &mut Vec::new());
    assert_eq!(
        why(&referee),
        "the engine merges into forge/app's main only what a run landed or another party made, not 8"
    );
}

#[test]
fn a_merge_that_loses_what_the_run_changed_fails_the_run() {
    let mut referee = started(b"43");
    referee.observe(at(5), moved(b"fix", 9, &[9, 1], b"43"), &mut Vec::new());
    referee.observe(at(6), ended(9), &mut Vec::new());
    referee.observe(at(7), merged(9, b"42"), &mut Vec::new());
    assert_eq!(why(&referee), r#"a merge keeps src/lib.rs as the head 9 has it: { "src/lib.rs": "42" } merged"#);
}

/// The engine records `ITEM` held for `why`, as it edits its record.
fn held(why: Hold) -> Seen {
    let hand = Hand {
        at: Duration::ZERO,
        repository: ITEM.repository,
        job: Job::Coding,
        work: Work::Agent,
        grants: CODING,
        budget: CALM,
    };
    let mut record = desk::record(&hand, Time::ZERO);
    record.lifecycle.phase = Phase::Held { why, outcome: None };
    let body = recorded(Position::START, 0, &codec::record_block(&record));
    let repository = REPOSITORIES[0].into();
    Seen::Forge(Observation::Edited { repository, number: ITEM.number, id: 3, body: body.into(), by: ENGINE })
}

/// A referee that has seen `ITEM` handed in, the forge or the store failing
/// if `faults`.
fn handed(faults: bool) -> Referee<Meeting> {
    let mut referee = Referee::new(Meeting::new(Duration::from_secs(60), faults));
    referee.observe(at(0), Seen::Handed { item: ITEM }, &mut Vec::new());
    referee
}

#[test]
fn an_issue_held_for_its_runs_failures_or_closed_ends_its_story() {
    let mut referee = handed(false);
    referee.observe(at(5), held(Hold::Failures(Class::Run)), &mut Vec::new());
    assert_eq!(referee.verdict(), Verdict::Passed);
    let mut referee = handed(false);
    let closed = Observation::Closed { repository: REPOSITORIES[0].into(), number: ITEM.number, by: ENGINE };
    referee.observe(at(5), Seen::Forge(closed), &mut Vec::new());
    assert_eq!(referee.verdict(), Verdict::Passed);
}

#[test]
fn an_issue_held_for_its_writes_where_nothing_fails_fails_the_run() {
    let mut referee = handed(true);
    referee.observe(at(5), held(Hold::Writes), &mut Vec::new());
    assert_eq!(referee.verdict(), Verdict::Passed, "the forge failed the engine's writes");
    let mut referee = handed(false);
    referee.observe(at(5), held(Hold::Writes), &mut Vec::new());
    assert!(why(&referee).starts_with(
        "the engine holds Item { repository: 0, number: 7 } only for what the scenario makes happen, not Writes"
    ));
}

#[test]
fn an_issue_neither_closed_nor_held_past_its_bound_fails_listing_it() {
    let mut referee = handed(false);
    assert_eq!(referee.next_deadline(), Some(at(0).saturating_add(STORY)));
    referee.fire(at(0).saturating_add(STORY), &mut Vec::new());
    assert_eq!(why(&referee), "Story(Item { repository: 0, number: 7 }) was not met by 43200.000000000s");
}

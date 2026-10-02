//! The codecs read back what they write, and refuse what a person mangled.

use temper_engine_model::brief::{self, Body, Section, Unread};
use temper_engine_model::forge::{Ci, Position};
use temper_engine_model::notes::{Author, Page, Reference};
use temper_engine_model::plan::{
    self, AgentSpec, Batch, Budget, ChangeSpec, Commit, Decided, Decision, Envelope, Finish, Gate, Goal, Grants,
    Growth, Plan, Progress, Repair, Repository, Resume, Review, Reviewed, SessionSpec, Sources, Step, Target, WaitSpec,
    Wake, Why, Work,
};
use temper_engine_model::rules::Permission;
use temper_engine_model::views::{Capture, Policy};
use temper_engine_model::work::{Class, Failures, Hold, Lifecycle, Phase};
use temper_engine_model::{Charter, Decoded, Item, Outcome, Posted, Record, Related, Relations};
use temper_engine_model_forge_tests::translate;
use temper_engine_model_tests::codec;
use temper_lib::{Duration, Time};

const GRANTS: Grants = Grants { modify: true, shell: false, forge: true, subagents: false, note: true };
const BUDGET: Budget = Budget { tokens: 1_000, turns: 9, time: Duration::from_secs(60) };

fn charter(instructions: &[u8]) -> plan::Charter {
    plan::Charter {
        instructions: instructions.into(),
        template: Some(b"t".as_slice().into()),
        grants: GRANTS,
        budget: BUDGET,
    }
}

fn steps() -> Box<[Step]> {
    let wake = Wake {
        on: Sources { own: true, related: false, subscribed: true, messages: true },
        every: Some(Duration::from_secs(5)),
        batch: Batch { count: 3, age: None },
    };
    Box::new([
        Step {
            name: b"spike".as_slice().into(),
            repository: Repository(0),
            work: Work::Agent(AgentSpec { charter: charter(b"try"), grows: true }),
            after: Box::new([]),
            gates: Box::new([Gate::Accepted]),
        },
        Step {
            name: b"build".as_slice().into(),
            repository: Repository(1),
            work: Work::Change(ChangeSpec {
                base: b"main".as_slice().into(),
                produce: charter(b"make"),
                checks: true,
                review: Review::Agent(charter(b"look")),
            }),
            after: Box::new([b"spike".as_slice().into()]),
            gates: Box::new([Gate::Approvals(2)]),
        },
        Step {
            name: b"wait".as_slice().into(),
            repository: Repository(0),
            work: Work::Wait(WaitSpec::Time(Duration::from_secs(9))),
            after: Box::new([]),
            gates: Box::new([]),
        },
        Step {
            name: b"talk".as_slice().into(),
            repository: Repository(0),
            work: Work::Session(SessionSpec { charter: charter(b"chat"), resume: Resume::Always, wake }),
            after: Box::new([]),
            gates: Box::new([]),
        },
    ])
}

fn envelope() -> Envelope {
    Envelope {
        agents: 1,
        changes: 5,
        waits: 0,
        sessions: 0,
        repositories: Box::new([Repository(0), Repository(1)]),
        into: Box::new([Target { repository: Repository(1), base: b"feat".as_slice().into() }]),
    }
}

fn record() -> Record {
    let goal = Goal {
        steps: Box::new([plan::Entry {
            name: b"spike".as_slice().into(),
            after: Box::new([b"x".as_slice().into()]),
            parent: Some(3),
            run: 2,
        }]),
        envelope: envelope(),
        budget: 3_000_000,
        estimate: 12,
        growth: Growth { agents: 1, changes: 2, waits: 0, sessions: 0 },
    };
    let progress = Progress {
        finished: false,
        running: Some(Why::Repair(Repair::BaseMoved)),
        last_run: Some(Time::from_nanos(77)),
        runs: 3,
        repairs: 1,
        rebases: 2,
        rejections: 0,
        review: Some(Reviewed { head: Commit(translate::commit(5)), verdict: plan::Verdict::Changes }),
        released: None,
    };
    let step = steps()[1].clone();
    Record {
        lifecycle: Lifecycle {
            phase: Phase::Held { why: Hold::Failures(Class::Agent), outcome: Some(41) },
            attempts: 4,
            failures: Failures { transient: 1, permanent: 0, run: 2, agent: 3, lost: 0, invalid: 9 },
        },
        step: plan::Record { step, progress, goal: Some(goal) },
        relations: Relations {
            created: Time::from_nanos(1_000),
            goal: Some(Item { repository: 0, number: 3 }),
            parent: None,
            pull: Some(12),
            branch: Some(translate::commit(8)),
            dependencies: Box::new([Related {
                name: b"spike".as_slice().into(),
                item: Item { repository: 1, number: 4 },
                done: Some(Time::from_nanos(5)),
            }]),
            children: Box::new([]),
            decision: Some(Decided { decision: Decision::Rejected, at: Time::from_nanos(6) }),
            accepted: Some(Permission::Admin),
            snapshot: true,
            spent: 900,
        },
    }
}

fn outcomes() -> Vec<Outcome> {
    let text: Box<[u8]> = b"words".as_slice().into();
    vec![
        Outcome::Change { message: text.clone() },
        Outcome::Verdict { verdict: plan::Verdict::Approve, text: text.clone() },
        Outcome::Report { text: text.clone() },
        Outcome::Plan { plan: Plan { steps: steps(), envelope: envelope(), budget: 99 }, text: text.clone() },
        Outcome::Steps { steps: steps(), text: text.clone() },
        Outcome::Tasks { tasks: steps(), text: text.clone() },
        Outcome::Reply { text: text.clone() },
        Outcome::Finished { text: text.clone() },
        Outcome::Release { step: b"spike".as_slice().into(), text: text.clone() },
        Outcome::Escalation { text },
    ]
}

#[test]
fn a_record_reads_back_after_its_head() {
    let position =
        Position { comment: 3, pull_comment: 4, reviews: 5, head: Some(translate::commit(9)), ci: Ci::Failed };
    let body = translate::recorded(position, 17, &codec::record_block(&record()));
    let mark = translate::mark(&body);
    assert_eq!(
        mark,
        temper_engine_model::forge::api::Mark::Record { position, nonce: 17 },
        "the head reads as written"
    );
    assert_eq!(codec::comment(8, &body), Some(Decoded::Record { comment: 8, record: Box::new(record()) }));
    assert!(codec::is_whole_record(&body), "a record as written is whole");
}

#[test]
fn a_mangled_record_does_not_decode() {
    let body = translate::recorded(Position::START, 1, &codec::record_block(&record()));
    let last = body.len() - 1;
    let mut mangled = body.clone();
    mangled[last] = if mangled[last] == b'0' { b'1' } else { b'0' };
    assert_eq!(codec::comment(1, &mangled), None, "an edited block's digest fails");
    assert!(!codec::is_whole_record(&mangled), "a mangled record is not whole");
    let mut cut = body;
    cut.truncate(cut.len() - 2);
    assert_eq!(codec::comment(1, &cut), None, "a cut block does not decode");
}

#[test]
fn every_outcome_reads_back_from_its_comment_and_from_a_channel() {
    for (attempt, outcome) in (1..).zip(outcomes()) {
        let posted = Posted { attempt, outcome: outcome.clone(), head: Some(translate::commit(attempt)) };
        let body = translate::keyed(b"k1", &codec::posted_block(&posted));
        assert_eq!(codec::comment(5, &body), Some(Decoded::Outcome { comment: 5, posted: Box::new(posted) }));
        assert_eq!(codec::outcome_of(&codec::outcome(&outcome)), Some(outcome), "it crosses a channel");
    }
    let plain = translate::keyed(b"k2", b"a person's words");
    assert_eq!(codec::comment(5, &plain), None, "a keyed comment holds no outcome unless it says so");
}

#[test]
fn a_page_reads_back_with_or_without_its_nonce() {
    let page = Page {
        description: b"a flaky test".as_slice().into(),
        author: Author::Run { repository: 1, number: 7 },
        references: Box::new([Reference { repository: 0, number: 2 }, Reference { repository: 1, number: 9 }]),
        body: b"retry it\nonce".as_slice().into(),
    };
    let text = codec::page(&page);
    assert_eq!(codec::page_of(&text), Some(page.clone()));
    assert_eq!(codec::page_of(&translate::paged(4, &text)), Some(page));
    let person = Page {
        description: b"d".as_slice().into(),
        author: Author::Person(3),
        references: Box::new([]),
        body: Box::new([]),
    };
    assert_eq!(codec::page_of(&codec::page(&person)), Some(person));
    assert_eq!(codec::page_of(b"just a line"), None, "a page that is not a note's");
}

#[test]
fn a_charter_crosses_a_channel() {
    let charter = Charter {
        why: Why::Review { head: Commit(translate::commit(3)) },
        brief: Box::new([
            Section { kind: brief::Kind::Item, body: Body::Text(b"the item".as_slice().into()) },
            Section { kind: brief::Kind::Dependencies, body: Body::Missing(Unread::Late) },
        ]),
        instructions: b"look".as_slice().into(),
        grants: GRANTS,
        finish: Finish::Turn { supervising: true },
        budget: BUDGET,
        models: b"m".as_slice().into(),
        policy: Policy {
            text: Capture::Content,
            progress: Capture::Shape,
            calls: Capture::Nothing,
            tools: Capture::Shape,
            usage: Capture::Content,
        },
    };
    let bytes = codec::charter(&charter);
    assert_eq!(codec::charter_of(&bytes), Some(charter));
    let mut longer = bytes;
    longer.push(0);
    assert_eq!(codec::charter_of(&longer), None, "trailing bytes are refused");
}

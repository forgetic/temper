use crate::*;
use alloc::boxed::Box;
use skein_lib::{Duration, Env, Queue, Time, Token, Wall};
use temper_engine_domain_forge_client as client;

const REPO: client::api::Repository = client::api::Repository { forge: 1, repository: 2 };
const CLIENT: client::Limits = client::Limits {
    pending: 4,
    calls: 2,
    rate: 100,
    reserve: 0,
    window: Duration::from_secs(10),
    op_bytes: 512,
    answer_bytes: 4096,
    rows: 8,
    inbox: 16,
    read_attempts: 3,
    backoff: Duration::from_secs(1),
    backoff_max: Duration::from_secs(4),
    entries: 8,
    write_attempts: 3,
    lifetime: Duration::from_secs(10),
    resources: 8,
    repositories: 2,
    poll: Duration::from_secs(5),
    poll_max: Duration::from_secs(30),
    hinted: Duration::from_secs(1),
    slow: Duration::from_secs(40),
    facts: 16,
};
const LIMITS: Limits = Limits {
    repositories: 2,
    tasks: 8,
    holds: 8,
    subscriptions: 8,
    entries: 8,
    changes: 8,
    issues: 8,
    resources_per_task: 4,
    paths_per_subscription: 4,
    name_bytes: 128,
    output: 64,
    facts: 16,
    adoptions: 2,
    collaborators: 8,
    landings: 8,
    brief_sections: 32,
    brief_bytes: 4096,
    issue_policy: temper_engine_domain_forge_issues::Limits {
        plan_items: 8,
        milestones: 8,
        title_bytes: 128,
        body_bytes: 512,
        comment_bytes: 256,
        interval: Duration::from_secs(5),
    },
    change_policy: temper_engine_domain_forge_change::Limits {
        repairs: 3,
        resolutions: 3,
        updates: 3,
        stall: Duration::from_secs(30),
        gates: 8,
        clean_heads: 8,
    },
    queue_window: Duration::from_secs(30),
    client: CLIENT,
};

fn env() -> Env<Limits> {
    Env { now: Time::from_nanos(100_000_000_000), wall: Wall::from_nanos(100_000_000_000), limits: LIMITS }
}
fn domain() -> Domain {
    Domain::new(
        &LIMITS,
        4,
        client::Config {
            namespace: Box::from(&b"test"[..]),
            writers: Box::new([client::Writer { forge: 1, author: 7 }]),
        },
    )
    .expect("valid connector")
}
fn outputs(d: &mut Domain, event: Event) -> Box<[Request]> {
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    step(d, &env(), event, &mut out);
    let mut values = skein_lib::List::with_capacity(out.len());
    for _ in 0..out.len() {
        values.push(out.pop().expect("counted output")).expect("sized from queue");
    }
    values.into_boxed()
}
fn ready(d: &mut Domain) -> Box<[Request]> {
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    resume(d, &env(), &mut out);
    let mut values = skein_lib::List::with_capacity(out.len());
    for _ in 0..out.len() {
        values.push(out.pop().expect("counted output")).expect("sized from queue");
    }
    values.into_boxed()
}
fn adopted() -> Repository {
    Repository {
        project: 5,
        home: true,
        provider: REPO,
        host: Box::from(&b"forge.example"[..]),
        owner: Box::from(&b"org"[..]),
        name: Box::from(&b"repo"[..]),
        prefix: Box::from(&b"temper/"[..]),
        role: Role::Owned,
        ci: true,
        checks: Box::new([]),
        kinds: Kinds {
            read: true,
            push: true,
            open: true,
            land: true,
            review: true,
            status: true,
            comment: true,
            issue: true,
            branch: true,
        },
        protection: Protection::Absent,
        settings: client::api::Settings {
            default_branch: Box::from(&b"main"[..]),
            merge: true,
            squash: true,
            rebase: false,
        },
    }
}
fn branch() -> Name {
    Name { forge: 1, repository: 2, what: What::Branch(Box::new([Box::from(&b"temper"[..]), Box::from(&b"7"[..])])) }
}

#[test]
fn a_hold_and_writer_slot_are_exclusive_until_answered() {
    let mut d = domain();
    outputs(&mut d, Event::Restore { record: Stored::Repository(adopted()) });
    let name = branch();
    let first = outputs(&mut d, Event::Hold { task: 7, resource: name.clone(), from: None });
    assert_eq!(first.len(), 1);
    assert_eq!(first[0], Request::Save { record: Stored::Hold(Hold { name: name.clone(), task: 7, writer: None }) });
    assert_eq!(
        outputs(&mut d, Event::Hold { task: 8, resource: name.clone(), from: None }).as_ref(),
        &[Request::Taken { task: 8, resource: name.clone(), by: 7 }]
    );
    outputs(&mut d, Event::Claim { task: 9, attempt: 1, writes: Box::new([name.clone()]), holders: Box::new([7]) });
    assert_eq!(
        outputs(
            &mut d,
            Event::Claim { task: 10, attempt: 1, writes: Box::new([name.clone()]), holders: Box::new([7]) }
        )
        .as_ref(),
        &[Request::Taken { task: 10, resource: name.clone(), by: 9 }]
    );
    outputs(&mut d, Event::Answered { task: 9, attempt: 1, pushed: Box::new([(name.clone(), [2; 32])]) });
    let claimed = outputs(
        &mut d,
        Event::Claim { task: 10, attempt: 1, writes: Box::new([name.clone()]), holders: Box::new([7]) },
    );
    assert_eq!(
        claimed.as_ref(),
        &[Request::Save {
            record: Stored::Hold(Hold { name, task: 7, writer: Some(Writer::Run { task: 10, attempt: 1 }) })
        }]
    );
}

#[test]
fn an_outbox_entry_waits_for_its_commit() {
    let mut d = domain();
    outputs(&mut d, Event::Restore { record: Stored::Repository(adopted()) });
    outputs(&mut d, Event::Restored { clock: client::RecoveryClock::Monotonic });
    let entry = client::Entry {
        number: 11,
        task: 7,
        repository: REPO,
        effect: client::Effect {
            write: client::api::Write::Status {
                commit: [3; 32],
                context: Box::from(&b"test"[..]),
                check: client::api::Check::Passed,
            },
            condition: client::Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
    };
    assert_eq!(
        outputs(&mut d, Event::Enqueue { entry: entry.clone() }).as_ref(),
        &[Request::Save { record: Stored::Entry(entry) }]
    );
    assert!(ready(&mut d).is_empty());
    outputs(&mut d, Event::Committed { entry: 11 });
    assert!(d.is_ready());
}

#[test]
#[expect(clippy::too_many_lines, reason = "one adoption test names every connector output explicitly")]
fn adoption_reads_permission_before_committing_its_role() {
    let mut d = domain();
    let request = Adoption {
        project: 5,
        home: true,
        provider: REPO,
        host: Box::from(&b"forge.example"[..]),
        owner: Box::from(&b"org"[..]),
        name: Box::from(&b"repo"[..]),
        prefix: Box::from(&b"temper/"[..]),
        role: Role::Owned,
        landing: Box::from(&b"main"[..]),
        ci: true,
        checks: Box::new([]),
    };
    outputs(&mut d, Event::Adopt { reply_to: Token::new(4), adoption: request });
    let sent = ready(&mut d);
    let (call, op) = match &sent[0] {
        Request::Call { call, op, .. } => (*call, op.clone()),
        Request::Adopted { .. }
        | Request::Save { .. }
        | Request::Erase { .. }
        | Request::Taken { .. }
        | Request::Refused { .. }
        | Request::Outcome { .. }
        | Request::ContinueRelease { .. }
        | Request::Released { .. }
        | Request::ReleaseFailed { .. }
        | Request::News { .. }
        | Request::Drift { .. }
        | Request::ProjectAfter { .. }
        | Request::ProjectionFailed { .. }
        | Request::ChangeDecision { .. }
        | Request::Read { .. }
        | Request::BriefClient { .. }
        | Request::BriefReady { .. } => panic!("permission call"),
    };
    assert_eq!(op, client::api::Op::Read(client::api::Read::Permission { user: 7 }));
    let result = outputs(
        &mut d,
        Event::Client(client::Event::Answered {
            call,
            cost: 1,
            result: Ok(client::api::Answer::Permission(client::api::Permission::Write)),
        }),
    );
    assert!(result.is_empty());
    let next = ready(&mut d);
    assert_eq!(next.len(), 1);
    let branches = match &next[0] {
        Request::Call { call, op: client::api::Op::Read(client::api::Read::Branches), .. } => *call,
        Request::Call { .. }
        | Request::Adopted { .. }
        | Request::Save { .. }
        | Request::Erase { .. }
        | Request::Taken { .. }
        | Request::Refused { .. }
        | Request::Outcome { .. }
        | Request::ContinueRelease { .. }
        | Request::Released { .. }
        | Request::ReleaseFailed { .. }
        | Request::News { .. }
        | Request::Drift { .. }
        | Request::ProjectAfter { .. }
        | Request::ProjectionFailed { .. }
        | Request::ChangeDecision { .. }
        | Request::Read { .. }
        | Request::BriefClient { .. }
        | Request::BriefReady { .. } => panic!("branches call"),
    };
    assert!(
        outputs(
            &mut d,
            Event::Client(client::Event::Answered {
                call: branches,
                cost: 1,
                result: Ok(client::api::Answer::Branches(Box::new([Box::from(&b"main"[..])]))),
            })
        )
        .is_empty()
    );
    let next = ready(&mut d);
    match &next[0] {
        Request::Call { op: client::api::Op::Read(client::api::Read::Settings), .. } => {}
        Request::Call { .. }
        | Request::Adopted { .. }
        | Request::Save { .. }
        | Request::Erase { .. }
        | Request::Taken { .. }
        | Request::Refused { .. }
        | Request::Outcome { .. }
        | Request::ContinueRelease { .. }
        | Request::Released { .. }
        | Request::ReleaseFailed { .. }
        | Request::News { .. }
        | Request::Drift { .. }
        | Request::ProjectAfter { .. }
        | Request::ProjectionFailed { .. }
        | Request::ChangeDecision { .. }
        | Request::Read { .. }
        | Request::BriefClient { .. }
        | Request::BriefReady { .. } => panic!("settings follows collision read"),
    }
}

#[test]
fn adoption_without_ci_refuses_an_empty_check_policy() {
    let mut d = domain();
    let request = Adoption {
        project: 5,
        home: true,
        provider: REPO,
        host: Box::from(&b"forge.example"[..]),
        owner: Box::from(&b"org"[..]),
        name: Box::from(&b"repo"[..]),
        prefix: Box::from(&b"temper/"[..]),
        role: Role::Owned,
        landing: Box::from(&b"main"[..]),
        ci: false,
        checks: Box::new([]),
    };
    assert_eq!(
        outputs(&mut d, Event::Adopt { reply_to: Token::new(5), adoption: request }).as_ref(),
        &[Request::Adopted { reply_to: Token::new(5), result: Err(client::api::Error::Refused) }]
    );
    assert!(ready(&mut d).is_empty());
}

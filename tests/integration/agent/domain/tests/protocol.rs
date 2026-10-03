//! The channel between the engine and the worker, as the protocol layers on
//! both sides translate it: names packed into tokens and back, an assignment
//! with its workspace and charter, and every answer and failure.

use skein_lib::{Duration, Token};
use temper_agent_domain_tests::protocol::{self, IDENTITY};
use temper_engine_domain::brief::{Body, Kind, Section};
use temper_engine_domain::plan::{self, Finish, Grants, Why};
use temper_engine_domain::views::{Capture, Policy};
use temper_engine_domain::{self as engine, Assignment, Charter, Checkout, Item, Outcome, Start, Workspace};
use temper_engine_domain_tests::codec;
use temper_worker_domain::{self as worker, host};

fn bytes(text: &[u8]) -> Box<[u8]> {
    text.into()
}

const ITEM: Item = Item { repository: 1, number: 42 };

fn charter() -> Charter {
    Charter {
        why: Why::Produce,
        brief: Box::new([Section { kind: Kind::Item, body: Body::Text(bytes(b"Fix it.")) }]),
        instructions: bytes(b"@coding"),
        grants: Grants { modify: true, shell: true, forge: false, subagents: false, note: false },
        finish: Finish::Change { checks: true },
        budget: plan::Budget { tokens: 100, turns: 3, time: Duration::from_secs(60) },
        models: bytes(b"fake-1"),
        policy: Policy {
            text: Capture::Content,
            progress: Capture::Shape,
            calls: Capture::Shape,
            tools: Capture::Shape,
            usage: Capture::Shape,
        },
    }
}

#[test]
fn every_attempt_of_every_item_has_a_name_of_its_own() {
    let items = [ITEM, Item { repository: 0, number: 42 }, Item { repository: 1, number: 43 }];
    let mut names = Vec::new();
    for item in items {
        for attempt in [1, 2, 7] {
            let name = protocol::attempt(item, attempt);
            assert_eq!(protocol::attempt_of(name), (item, attempt));
            names.push(name);
        }
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 9);
    assert_ne!(protocol::run(items[0]), protocol::run(items[1]));
}

#[test]
fn an_assignment_reaches_the_worker_with_its_workspace_and_its_charter_framed() {
    let assignment = Assignment {
        item: ITEM,
        attempt: 3,
        workspace: Workspace {
            key: bytes(b"acme/two#42"),
            repositories: Box::new([Checkout {
                repository: 1,
                start: Start::Branch { branch: bytes(b"temper/42") },
                push: Some(bytes(b"temper/42")),
            }]),
        },
        save: Some(bytes(b"temper-saved/42")),
        charter: charter(),
        snapshot: None,
    };
    let (assignment, places) = protocol::assignment(assignment);
    assert_eq!(places, [1]);
    assert_eq!((assignment.run, assignment.attempt), (protocol::run(ITEM), protocol::attempt(ITEM, 3)));
    let repository = host::Repository {
        name: bytes(b"two"),
        remote: bytes(b"acme/two"),
        start: host::Start::Branch { branch: bytes(b"temper/42") },
        access: host::Access::Writable { push: bytes(b"temper/42") },
        identity: bytes(IDENTITY),
    };
    assert_eq!(*assignment.workspace.repositories, [repository]);
    assert_eq!(assignment.save.as_deref(), Some(&b"temper-saved/42"[..]));
    // The charter goes as the engine's codec writes it, behind the name of
    // the attempt it is for.
    let (attempt, encoded) = protocol::unframed(&assignment.charter);
    assert_eq!(attempt, assignment.attempt);
    assert_eq!(codec::charter_of(encoded), Some(charter()));
}

#[test]
fn every_answer_reaches_the_engine_as_its_protocol_layer_decodes_it() {
    let commit = [9; 32];
    let work = |landed: &[u32]| host::Work {
        landed: landed.iter().map(|place| host::Landed { repository: *place, commit }).collect(),
        saved: None,
    };
    let outcome = Outcome::Change { message: bytes(b"Fix it") };
    let ended = host::Answer::Ended { outcome: codec::outcome(&outcome).into(), work: work(&[0]) };
    let landed = engine::Work { landed: Box::new([engine::Landed { repository: 1, commit }]) };
    assert_eq!(protocol::answer(ended, &[1]), engine::Answer::Ended { outcome, work: landed });
    assert_eq!(protocol::answer(host::Answer::Refused(host::Refusal::Busy), &[1]), engine::Answer::Busy);
    let invalid = host::Answer::Refused(host::Refusal::Invalid(host::Invalid::Charter));
    assert_eq!(protocol::answer(invalid, &[1]), engine::Answer::Invalid);

    let classes = [
        (host::Failure::Unprepared(host::Preparation::Transient), engine::Failure::Transient),
        (
            host::Failure::Unprepared(host::Preparation::Missing { repository: 0, missing: host::Missing::Branch }),
            engine::Failure::Permanent,
        ),
        (host::Failure::Unprepared(host::Preparation::Refused { repository: 0 }), engine::Failure::Permanent),
        (host::Failure::Run(host::RunFailure::Stale), engine::Failure::Run),
        (host::Failure::Agent(host::AgentFailure::NoProgress), engine::Failure::Agent),
        (host::Failure::Cancelled(host::Reason::Engine), engine::Failure::Transient),
        (host::Failure::Cancelled(host::Reason::Contact), engine::Failure::Transient),
    ];
    for (failure, class) in classes {
        let failed = host::Answer::Failed { failure, detail: bytes(b"for operators"), work: work(&[]) };
        let expected = engine::Answer::Failed { failure: class, work: engine::Work { landed: Box::new([]) } };
        assert_eq!(protocol::answer(failed, &[1]), expected, "{failure:?}");
    }
}

#[test]
fn a_hello_names_each_run_hosted_by_its_item_and_attempt() {
    let hosted =
        worker::Hosted { run: protocol::run(ITEM), attempt: protocol::attempt(ITEM, 2), phase: worker::Phase::Active };
    let hello = worker::Hello { slots: 3, workstreams: Box::new([bytes(b"acme/two#42")]), hosting: Box::new([hosted]) };
    let expected = engine::Hello {
        slots: 3,
        workstreams: Box::new([bytes(b"acme/two#42")]),
        hosting: Box::new([engine::Hosted { item: ITEM, attempt: 2, phase: engine::fleet::Phase::Active }]),
    };
    assert_eq!(protocol::hello(hello), expected);
}

#[test]
fn what_the_engine_sends_a_run_reaches_the_worker_under_the_runs_names() {
    let channel = Token::new(5);
    let cancel = engine::Request::Cancel { channel, item: ITEM, attempt: 2 };
    let expected = worker::Event::Cancel { run: protocol::run(ITEM), attempt: protocol::attempt(ITEM, 2) };
    assert_eq!(protocol::down(&cancel), Some(expected));
    let acknowledge = engine::Request::Acknowledge { channel, item: ITEM, attempt: 2 };
    let expected = worker::Event::Acknowledged { run: protocol::run(ITEM), attempt: protocol::attempt(ITEM, 2) };
    assert_eq!(protocol::down(&acknowledge), Some(expected));
    let finished = engine::Inbound::Finished { item: ITEM };
    let inbound = engine::Request::Inbound { channel, item: ITEM, attempt: 2, event: finished };
    let expected = worker::Event::Inbound {
        run: protocol::run(ITEM),
        attempt: protocol::attempt(ITEM, 2),
        event: bytes(b"finished"),
    };
    assert_eq!(protocol::down(&inbound), Some(expected));
    assert_eq!(protocol::down(&engine::Request::Refuse { channel }), None);
}

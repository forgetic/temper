//! Faults injected into independent boundary referees for each promise in
//! domain/core.md, section 10. The same referees observe the passing worlds.

use jig_core_people::Role;
use jig_people_world::referee::{People, Seen as PersonSeen};
use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use temper_engine_domain::{Key, Record};
use temper_engine_domain_tasks::Party;
use temper_engine_domain_world::walking::{Settings, World};
use temper_engine_domain_world::walking_referee::REPORT;
use temper_engine_forge_world::fake_config;
use temper_engine_tasks_world::referee::{Seen as TaskSeen, Tasks};
use temper_fake_forge_domain::{self as fake, Observation, api as raw};
use temper_world::{Referee, Verdict};

fn task_rejects(observations: &[TaskSeen]) {
    let mut referee = Referee::new(Tasks::default());
    for observation in observations {
        referee.observe(Time::ZERO, observation.clone(), &mut Vec::new());
    }
    assert!(matches!(referee.verdict(), Verdict::Failed(_)));
}

fn settled_chat() -> World {
    let mut world = World::new(Settings::calm(9821));
    world.run();
    assert!(world.referee.done(), "the real root supplied the good control case");
    world
}

#[test]
fn authority_referee_rejects_a_person_without_a_grant() {
    let mut referee = Referee::new(People::default());
    referee.observe(Time::ZERO, PersonSeen::Routed { role: Role::Observer }, &mut Vec::new());
    assert!(matches!(referee.verdict(), Verdict::Failed(_)));
}

#[test]
fn once_referee_rejects_a_second_result_from_the_same_durable_task() {
    let world = settled_chat();
    let (person, task) = world
        .store
        .rows
        .values()
        .find_map(|row| {
            let Record::Tasks(temper_engine_domain_tasks::Stored::Ended(record)) = row else { return None };
            let Party::Person(person) = record.requester else { return None };
            Some((person, record.number))
        })
        .expect("one ended chat");
    let mut referee = world.referee.clone();
    assert_eq!(referee.result(&world.store.rows, person, task, REPORT), Err("person received result twice"));
}

#[test]
fn commit_referee_rejects_an_ack_if_its_transcript_was_lost() {
    let world = settled_chat();
    let mut rows = world.store.rows.clone();
    let task = rows
        .values()
        .find_map(|row| {
            let Record::Tasks(temper_engine_domain_tasks::Stored::Ended(record)) = row else { return None };
            Some((record.number, record.attempt))
        })
        .expect("ended task");
    rows.remove(&Key::Turn { task: task.0, attempt: task.1, turn: 1 });
    assert_eq!(world.referee.clone().turn_ack(&rows, task.0, task.1, 1), Err("turn ACK before durable transcript"));
}

#[test]
fn order_referee_rejects_an_early_dependency_and_overlapping_runs() {
    let made = TaskSeen::Made { task: 1, parent: Party::Person(1), dependencies: vec![], depth: 0 };
    let dependent = TaskSeen::Made { task: 2, parent: Party::Person(1), dependencies: vec![1], depth: 0 };
    task_rejects(&[made.clone(), dependent, TaskSeen::Assigned { task: 2, attempt: 1, after: 0, adopted: false }]);
    task_rejects(&[
        made,
        TaskSeen::Assigned { task: 1, attempt: 1, after: 0, adopted: false },
        TaskSeen::Assigned { task: 1, attempt: 2, after: 0, adopted: false },
    ]);
}

#[test]
fn no_loss_referee_rejects_an_unfinished_cancelled_task() {
    task_rejects(&[
        TaskSeen::Made { task: 1, parent: Party::Person(1), dependencies: vec![], depth: 0 },
        TaskSeen::Cancelled { task: 1 },
        TaskSeen::Finished,
    ]);
}

#[test]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "the outside observation test selects only move and rejected-push evidence"
)]
fn no_overwrite_referee_observes_and_rejects_a_divergent_push() {
    let mut config = fake_config();
    config.latency_min = Duration::ZERO;
    config.latency_max = Duration::ZERO;
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config };
    let mut forge = fake::Domain::new(&config, 98);
    let first = fake::repository(
        &mut forge,
        &config,
        raw::Setup {
            name: Box::from(&b"org/repo"[..]),
            default: Box::from(&b"main"[..]),
            tree: Box::new([]),
            labels: Box::new([]),
            checks: raw::Checks {
                contexts: Box::new([]),
                latency_min: Duration::ZERO,
                latency_max: Duration::ZERO,
                silent: 1000,
                passes: 1000,
                reruns: 0,
                cue: None,
            },
            protection: None,
            hooked: false,
        },
    );
    fake::grant(&mut forge, b"org/repo", 1, raw::Permission::Write);
    let aside = fake::commit(
        &mut forge,
        &config,
        first,
        Box::new([raw::File { path: Box::from(&b"file"[..]), content: Box::from(&b"aside"[..]) }]),
        b"aside",
    )
    .expect("bounded commit")
    .expect("new tree");
    let landed = fake::advance(&mut forge, &env, b"org/repo", b"main", b"file", b"landed", 2)
        .expect("other party advanced main");
    let mut out = Queue::with_capacity(fake::MAX_OUT);
    fake::step(
        &mut forge,
        &env,
        fake::Event::Call {
            reply_to: ReplyTo::new(Token::new(1)),
            user: 1,
            repository: Box::from(&b"org/repo"[..]),
            op: raw::Op::Git(raw::Git::Push { branch: Box::from(&b"main"[..]), commit: aside, expected: None }),
        },
        &mut out,
    );
    if forge.is_due(env.now) {
        fake::fire(&mut forge, &env, &mut out);
    }
    assert!(matches!(
        out.pop(),
        Some(fake::Request::Reply { result: Ok(raw::Answer::Pushed(raw::Pushed::Rejected)), .. })
    ));
    let mut rejected = false;
    let mut overwritten = false;
    while let Some(observation) = forge.pop_observation() {
        match observation {
            Observation::Rejected { by: 1, commit, .. } if commit == aside => rejected = true,
            Observation::Moved { by: 1, .. } => overwritten = true,
            _ => {}
        }
    }
    assert!(rejected && !overwritten, "the outside observer caught the attempted overwrite and saw no move");
    let mut read = Queue::with_capacity(fake::MAX_OUT);
    fake::step(
        &mut forge,
        &env,
        fake::Event::Call {
            reply_to: ReplyTo::new(Token::new(2)),
            user: 1,
            repository: Box::from(&b"org/repo"[..]),
            op: raw::Op::Read(raw::Read::Branch { branch: Box::from(&b"main"[..]) }),
        },
        &mut read,
    );
    if forge.is_due(env.now) {
        fake::fire(&mut forge, &env, &mut read);
    }
    assert!(
        matches!(read.pop(), Some(fake::Request::Reply { result: Ok(raw::Answer::Commit(head)), .. }) if head == landed)
    );
}

#[test]
fn bounded_referee_rejects_more_live_tasks_than_the_world_allows() {
    task_rejects(&[TaskSeen::Limit { live: 3, cap: 2 }]);
    task_rejects(&[
        TaskSeen::Made { task: 1, parent: Party::Person(1), dependencies: vec![], depth: 0 },
        TaskSeen::Cancelled { task: 1 },
        TaskSeen::Finished,
    ]);
}

use super::resume_fixture::root;
use crate::{Domain, Event, Limits, Request, StoreInput};
use skein_lib::{Env, List, Queue, Time, Wall};

// More than the service's two empty release passes, at one injected instant.
const RESUMES: u32 = 4;
// Initial instant, then an independent change of each injected clock.
const CLOCK_PAIRS: u32 = 3;
const WATCHER: skein_lib::Token = skein_lib::Token::new(30);
const SIGN_IN: u64 = 10;

#[test]
fn reconciliation_can_start_once_the_connectors_restore_and_stays_open() {
    use jig_domain_core::{RestartRequest, RestartStep};
    let limits = super::resume_fixture::limits();
    let mut domain = Domain::new(super::resume_fixture::config(7), &limits).unwrap();
    assert!(!domain.connectors_restored());
    let mut request = domain.core.restart_begin();
    // False through the last connector restore, true from the next step on.
    let (mut restoring, mut after) = (false, false);
    for _ in 0_u32..16 {
        match request {
            RestartRequest::Step(step) => {
                match step {
                    RestartStep::RestoreConnector { .. } => restoring = true,
                    RestartStep::LoadCore
                    | RestartStep::LoadInbox
                    | RestartStep::AdoptRuns
                    | RestartStep::ReadAfresh { .. }
                    | RestartStep::SettleOutbox { .. }
                    | RestartStep::Open => after = after || restoring,
                }
                assert_eq!(domain.connectors_restored(), after, "{step:?}");
                assert!(!domain.ready());
                request = domain.core.restart_done(step);
            }
            RestartRequest::Idle => {
                assert!(after, "a step followed the connectors' restore");
                assert!(domain.connectors_restored() && domain.ready());
                return;
            }
            RestartRequest::Settings { failure, changed } => {
                panic!("empty settings restore: {failure:?}, changed {changed}")
            }
            RestartRequest::Refused { step } => panic!("empty restore: {step:?}"),
        }
    }
    panic!("restart did not open");
}

fn resume(domain: &mut Domain, env: &Env<Limits>) {
    for _ in 0..RESUMES {
        crate::try_step(domain, env, Event::Resume).unwrap();
    }
}

#[test]
fn settled_resumes_skip_all_empty_decisions_across_clock_pairs() {
    let (mut domain, mut env) = root();
    let mut out = Queue::with_capacity(crate::max_out(&env.limits));
    let mut counts = List::with_capacity(CLOCK_PAIRS);
    for (now, wall) in [(0, 0), (1, 0), (1, 1)] {
        env.now = Time::from_nanos(now);
        env.wall = Wall::from_nanos(wall);
        resume(&mut domain, &env);
        crate::release(&mut domain, &env, &mut out);
        counts.push(domain.empty_resumes).unwrap();
    }
    assert_eq!(counts.as_slice(), [0, 0, 0]);
    assert!(out.is_empty());
    assert_eq!(domain.deployment().commits, 0);
}

#[test]
fn inputs_fire_and_reclaim_preserve_derived_quiet_readiness() {
    let (mut domain, env) = root();
    resume(&mut domain, &env);
    // An inert input, fire and reclaim must not invent actionable finishing.
    let mut counts = List::with_capacity(3);
    crate::try_step(&mut domain, &env, Event::Party(crate::PartyInput::Unwatch { watcher: skein_lib::Token::new(1) }))
        .unwrap();
    resume(&mut domain, &env);
    counts.push(domain.empty_resumes).unwrap();
    crate::fire(&mut domain, &env);
    resume(&mut domain, &env);
    counts.push(domain.empty_resumes).unwrap();
    domain.reclaim();
    resume(&mut domain, &env);
    counts.push(domain.empty_resumes).unwrap();
    assert_eq!(counts.as_slice(), [0, 0, 0]);
    assert_eq!(domain.deployment().commits, 0);
}

#[test]
fn an_automatic_header_commit_cannot_mark_the_pass_quiet() {
    let (mut domain, env) = root();
    crate::fresh(&mut domain.core.counters, crate::Family::Message).unwrap();
    crate::try_step(&mut domain, &env, Event::Resume).unwrap();
    assert_eq!(domain.empty_resumes, 0);
    assert_eq!(domain.settled, None);
    assert_eq!(domain.deployment().commits, 1);
}

#[test]
fn a_commit_completion_preserves_quiet_without_an_empty_pass() {
    let (mut domain, env) = root();
    crate::fresh(&mut domain.core.counters, crate::Family::Message).unwrap();
    resume(&mut domain, &env);
    let mut out = Queue::with_capacity(crate::max_out(&env.limits));
    crate::release(&mut domain, &env, &mut out);
    let Some(Request::Store(crate::StoreRequest::Commit { number, .. })) = out.pop() else { panic!("header commit") };
    crate::try_step(&mut domain, &env, Event::Store(StoreInput::Committed { number, fill: 0 })).unwrap();
    resume(&mut domain, &env);
    assert_eq!(domain.empty_resumes, 0);
    assert_eq!(domain.deployment().commits, 1);
}

#[test]
fn released_callbacks_and_restart_preserve_derived_quiet_readiness() {
    let (mut domain, env) = root();
    resume(&mut domain, &env);
    let released = crate::Released(crate::Delivery::Fleet(jig_domain_fleet::Event::Acknowledge {
        run: skein_lib::Token::new(1),
        attempt: skein_lib::Token::new(1),
    }));
    crate::try_step(&mut domain, &env, Event::Released(released)).unwrap();
    resume(&mut domain, &env);
    let after_callback = domain.empty_resumes;
    crate::try_step(&mut domain, &env, Event::Restart).unwrap();
    resume(&mut domain, &env);
    assert_eq!((after_callback, domain.empty_resumes), (0, 0));
}

#[test]
fn finishing_frees_retained_text_without_any_watcher() {
    let (mut domain, env) = root();
    let run = skein_lib::Token::new(1);
    let mut out = Queue::with_capacity(1);
    jig_domain_views::step(
        &mut domain.core.views,
        &Env { now: env.now, wall: env.wall, limits: crate::VIEWS },
        jig_domain_views::Event::Text {
            run,
            turn: jig_domain_views::TurnName { attempt: 1, turn: 1, retry: 0 },
            offset: 0,
            text: b"provisional".as_slice().into(),
            call: None,
        },
        &mut out,
    );
    let retained = domain.core.views.retained_runs().len();
    resume(&mut domain, &env);
    assert_eq!((retained, domain.core.views.retained_runs().len(), domain.empty_resumes), (1, 0, 1));
    assert!(out.is_empty());
}

fn watched_root() -> (Domain, Env<Limits>) {
    use jig_domain_people as people;
    let (mut domain, env) = root();
    let mut out = Queue::with_capacity(people::max_out(&env.limits.people));
    domain.core.people = people::Domain::new(&env.limits.people, Box::new([]), 1, Box::new([]));
    for record in [
        people::Stored::Person {
            number: 1,
            identity: people::Identity {
                key: people::IdentityKey { provider: 0, subject: b"owner".as_slice().into() },
                login: Box::new([]),
                name: Box::new([]),
            },
        },
        people::Stored::SignIn { number: SIGN_IN, person: 1, expires: Wall::from_nanos(100) },
        people::Stored::Roles {
            project: 1,
            holdings: Box::new([people::Holding { person: 1, role: people::Role::Owner }]),
        },
    ] {
        people::step(
            &mut domain.core.people,
            &Env { now: env.now, wall: env.wall, limits: env.limits.people },
            people::Event::Restore { record },
            &mut out,
        );
    }
    people::step(
        &mut domain.core.people,
        &Env { now: env.now, wall: env.wall, limits: env.limits.people },
        people::Event::Restored,
        &mut out,
    );
    crate::try_step(
        &mut domain,
        &env,
        Event::Party(crate::PartyInput::Watch {
            watcher: WATCHER,
            sign_in: SIGN_IN,
            subject: jig_domain_core::documents::Watched::Frame,
            window: jig_domain_core::documents::Window { rows: 1, after: None },
        }),
    )
    .unwrap();
    let mut released = Queue::with_capacity(crate::max_out(&env.limits));
    crate::release(&mut domain, &env, &mut released);
    crate::release(&mut domain, &env, &mut released);
    crate::try_step(&mut domain, &env, Event::Party(crate::PartyInput::Delivered { watcher: WATCHER })).unwrap();
    resume(&mut domain, &env);
    (domain, env)
}

#[test]
fn a_projection_feed_then_committed_resume_delivers_the_watch() {
    let (mut domain, env) = watched_root();
    // Restore a changed projection through jig's existing row seam. The next
    // ordinary input finishes its feed before the deferred watch pass.
    crate::fresh(&mut domain.core.counters, crate::Family::Task).unwrap();
    let restored = domain.core.restore_core(
        jig_domain_core::CoreRecord::Projection(jig_domain_core::ProjectionRecord::Goal {
            goal: jig_domain_core::ProjectionGoal {
                number: 1,
                project: 1,
                priority: 1,
                words: b"goal".as_slice().into(),
                phase: jig_domain_tasks::Phase::Waiting,
            },
            pending: Box::new([]),
            changed: true,
            fed_at: None,
        }),
        &crate::limits::core_limits(&env.limits),
    );
    crate::try_step(
        &mut domain,
        &env,
        Event::Party(crate::PartyInput::Period { project: 1, period: 1, budget: 100, next: None }),
    )
    .unwrap();
    let mut out = Queue::with_capacity(crate::max_out(&env.limits));
    crate::release(&mut domain, &env, &mut out);
    let Some(Request::Store(crate::StoreRequest::Commit { number, writes })) = out.pop() else {
        panic!("projection and period commit")
    };
    let mut projection_written = false;
    for write in &writes {
        projection_written |= write.key()
            == crate::Key::Core(jig_domain_core::Key::Core(jig_domain_core::CoreKey::Projection(
                jig_domain_core::ProjectionKey::Goal(1),
            )));
    }
    let before_committed = out.len();
    crate::try_step(&mut domain, &env, Event::Store(StoreInput::Committed { number, fill: 0 })).unwrap();
    crate::try_step(&mut domain, &env, Event::Resume).unwrap();
    crate::release(&mut domain, &env, &mut out);
    crate::release(&mut domain, &env, &mut out);
    let Some(Request::Party(crate::PartyRequest::Deliver(crate::Delivery::Document { watcher, .. }))) = out.pop()
    else {
        panic!("deferred watch document: {out:?}")
    };
    assert_eq!(restored, jig_domain_core::Restored::Live);
    assert!(projection_written);
    assert_eq!((before_committed, watcher, domain.empty_resumes), (0, WATCHER, 0));
    assert_eq!(domain.deployment().commits, 1);
    assert!(out.is_empty());
}

#[test]
fn advancing_wall_time_ends_a_settled_watch() {
    let (mut domain, mut env) = watched_root();
    env.wall = Wall::from_nanos(100);
    crate::try_step(&mut domain, &env, Event::Resume).unwrap();
    let mut out = Queue::with_capacity(crate::max_out(&env.limits));
    crate::release(&mut domain, &env, &mut out);
    crate::release(&mut domain, &env, &mut out);
    assert_eq!(
        out.pop(),
        Some(Request::Party(crate::PartyRequest::Deliver(crate::Delivery::WatchEnded {
            watcher: WATCHER,
            why: jig_domain_views::End::SignedOut,
        })))
    );
    assert!(out.is_empty());
    assert_eq!(domain.core.people.sign_in_person(SIGN_IN), Some(1));
    assert_eq!(domain.deployment().commits, 0);
    assert_eq!(domain.next_deadline(&env), Some(env.now));
}

#[test]
fn stop_from_settled_still_releases_the_drain_terminal() {
    let (mut domain, env) = root();
    resume(&mut domain, &env);
    crate::try_step(&mut domain, &env, Event::Stop).unwrap();
    let mut out = Queue::with_capacity(crate::max_out(&env.limits));
    crate::release(&mut domain, &env, &mut out);
    crate::release(&mut domain, &env, &mut out);
    assert_eq!(out.pop(), Some(Request::Drained));
    assert!(out.is_empty());
    assert_eq!(domain.settled, None);
}

#[test]
#[expect(clippy::too_many_lines, reason = "the exhaustive output rejection includes public forge requests")]
fn stopping_continues_projection_deferred_finishing_without_new_commits() {
    use jig_domain_core as core;
    use jig_domain_tasks as tasks;
    let (mut domain, env) = watched_root();
    let goal = crate::fresh(&mut domain.core.counters, crate::Family::Task).unwrap();
    let mut decision = crate::limits::route_decision(&mut domain, &env.limits).unwrap();
    let core_env = crate::limits::environment_core(&env);
    let mut authority = core::task_authority(&domain.core.authority.policy(1).unwrap().ceiling);
    authority.budget.spend = 100;
    for event in [
        tasks::Event::Restored,
        tasks::Event::OpenPeriod {
            reply_to: skein_lib::ReplyTo::new(skein_lib::Token::new(u64::MAX - 2)),
            project: 1,
            period: 1,
            budget: 100,
        },
        tasks::Event::Make {
            reply_to: skein_lib::ReplyTo::new(skein_lib::Token::new(u64::MAX - 3)),
            creator: tasks::Party::Deployment { project: 1 },
            batch: Box::new([tasks::New {
                number: goal,
                project: 1,
                label: 0,
                flags: tasks::Flags::PLAIN,
                executor: tasks::Executor::Agent { charter: 1 },
                llm: None,
                spec: tasks::Spec {
                    words: b"queued goal before Stop".as_slice().into(),
                    parameters: Box::new([]),
                    inputs: Box::new([]),
                },
                contract: tasks::Contract::Report { words: 128 },
                deadline: tasks::Deadline::Named,
                authority,
                numbers: tasks::Numbers { budget: 100, spent: 0, spent_below: 0, reserved: 0 },
                funder: tasks::Funder::Period { project: 1, period: 1 },
                dependencies: Box::new([]),
                holdings: Box::new([]),
                wake: tasks::WakePolicy::DEFAULT,
                recurring: None,
                tracked: Some(0),
            }]),
        },
    ] {
        let requests = core::step(&mut domain.core, &core_env, core::Event::Tasks(event));
        crate::route::route_core_requests(&mut domain, &env, &mut decision, requests);
    }
    assert!(domain.core.tasks.task(goal).is_some(), "real goal before Stop");
    let requests = core::step(&mut domain.core, &core_env, core::Event::Stopping);
    crate::route::route_core_requests(&mut domain, &env, &mut decision, requests);
    crate::route::route_into(&mut domain, &env, &mut decision);
    let mut out = Queue::with_capacity(crate::max_out(&env.limits));
    crate::domain::close(&mut domain, &env, decision, &mut out);
    assert!(domain.core.finish_owed(), "projection defers watch finishing");
    crate::release(&mut domain, &env, &mut out);
    let Some(Request::Store(crate::StoreRequest::Commit { number, .. })) = out.pop() else {
        panic!("goal and projection commit")
    };
    assert!(out.is_empty(), "no watch or drain before the previous commit");
    let commits = domain.deployment().commits;
    // Even finishing before Committed must keep both terminals behind it.
    resume(&mut domain, &env);
    crate::release(&mut domain, &env, &mut out);
    assert!(out.is_empty(), "finishing remains behind the last commit");
    crate::try_step(&mut domain, &env, Event::Store(StoreInput::Committed { number, fill: 0 })).unwrap();
    let mut endings = 0_u32;
    let mut drains = 0_u32;
    for _ in 0_u32..12 {
        crate::try_step(&mut domain, &env, Event::Resume).unwrap();
        crate::release(&mut domain, &env, &mut out);
        crate::release(&mut domain, &env, &mut out);
        while let Some(request) = out.pop() {
            match request {
                Request::Party(crate::PartyRequest::Deliver(crate::Delivery::WatchEnded { watcher, why })) => {
                    assert_eq!((watcher, why), (WATCHER, jig_domain_views::End::Stopping));
                    endings += 1;
                }
                Request::Drained => drains += 1,
                Request::Party(crate::PartyRequest::Deliver(crate::Delivery::WebReply { .. })) => {}
                other @ (Request::Setup { .. }
                | Request::StopGroup { .. }
                | Request::SetUpGroup { .. }
                | Request::Connect { .. }
                | Request::Manifest { .. }
                | Request::Reachable { .. }
                | Request::Install { .. }
                | Request::EraseCarried { .. }
                | Request::SetUp { .. }
                | Request::Bound { .. }
                | Request::Return { .. }
                | Request::Store(_)
                | Request::Worker(_)
                | Request::Party(_)
                | Request::Forge { .. }
                | Request::Account(_)
                | Request::ToChild(_)
                | Request::Busy(_)
                | Request::Stop) => panic!("unexpected stopped continuation: {other:?}"),
            }
        }
    }
    assert_eq!((endings, drains), (1, 1));
    assert_eq!(domain.deployment().commits, commits, "zero new commits");
    assert!(!domain.core.finish_owed());
    resume(&mut domain, &env);
    crate::release(&mut domain, &env, &mut out);
    assert!(out.is_empty(), "repeated stopped Resume stays quiet");
}

#[test]
fn people_deadlines_use_both_injected_clocks_and_keep_the_original_alarm() {
    let (domain, mut env) = watched_root();
    for (now, wall, expected) in [(1, 50, 51), (2, 0, 100), (3, 100, 3), (100, 0, 100)] {
        env.now = Time::from_nanos(now);
        env.wall = Wall::from_nanos(wall);
        assert_eq!(domain.next_deadline(&env), Some(Time::from_nanos(expected)));
    }
}

#[test]
fn ordinary_root_fire_expires_sign_ins_once_by_either_clock() {
    for (now, wall) in [(1, 100), (100, 0)] {
        let (mut domain, mut env) = watched_root();
        env.now = Time::from_nanos(now);
        env.wall = Wall::from_nanos(wall);
        assert_eq!(domain.core.people.sign_in_person(SIGN_IN), Some(1));
        assert_eq!(domain.next_deadline(&env), Some(env.now));
        crate::fire(&mut domain, &env);
        assert_eq!(domain.core.people.sign_in_person(SIGN_IN), None);
        assert_eq!(domain.core.people.next_wall_expiry(), None);
        assert_eq!(domain.deployment().commits, 1);
        crate::fire(&mut domain, &env);
        resume(&mut domain, &env);
        assert_eq!(domain.deployment().commits, 1);
    }
}

#[test]
fn refused_people_timer_stays_due_without_child_mutation_until_room_returns() {
    let (mut domain, mut env) = watched_root();
    // Three actual header decisions fill this fixture's three-commit journal.
    for _ in 0..env.limits.journal.commits {
        crate::fresh(&mut domain.core.counters, crate::Family::Message).unwrap();
        crate::try_step(&mut domain, &env, Event::Resume).unwrap();
    }
    assert!(!crate::domain::admits(&domain, &env.limits));
    env.now = Time::from_nanos(1);
    env.wall = Wall::from_nanos(100);
    let before = domain.deployment();
    let empty = domain.empty_resumes;
    crate::fire(&mut domain, &env);
    resume(&mut domain, &env);
    assert_eq!(domain.deployment(), before);
    assert_eq!(domain.empty_resumes, empty);
    assert_eq!(domain.core.people.sign_in_person(SIGN_IN), Some(1));
    assert_eq!(domain.next_deadline(&env), Some(env.now));
    let mut out = Queue::with_capacity(crate::max_out(&env.limits));
    crate::release(&mut domain, &env, &mut out);
    let Some(Request::Store(crate::StoreRequest::Commit { number, .. })) = out.pop() else {
        panic!("first admitted commit")
    };
    crate::try_step(&mut domain, &env, Event::Store(StoreInput::Committed { number, fill: 0 })).unwrap();
    assert!(crate::domain::admits(&domain, &env.limits));
    crate::fire(&mut domain, &env);
    assert_eq!(domain.core.people.sign_in_person(SIGN_IN), None);
    assert_eq!(domain.core.people.next_wall_expiry(), None);
    assert_eq!(domain.deployment().commits, before.commits + 1);
}

#[test]
fn authenticated_read_bytes_are_configured_and_counted() {
    let (_, env) = root();
    let mut limits = env.limits;
    limits.loads.read_bytes = 65_536;
    let before = crate::worst_case(&limits).expect("valid read bound");
    limits.loads.read_bytes += 65_536;
    assert_eq!(crate::limits::core_limits(&limits).load_bytes, 131_072);
    assert!(crate::worst_case(&limits).expect("larger read bound") > before);
    limits.loads.read_bytes = 0;
    assert!(crate::worst_case(&limits).is_none());
}

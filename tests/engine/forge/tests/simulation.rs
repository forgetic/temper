use skein_lib::Wall;
use temper_engine_domain_forge as top;
use temper_engine_domain_forge_change as change;
use temper_engine_domain_forge_client as client;
use temper_engine_domain_forge_issues as issues;
use temper_engine_forge_world::{REPO, World};

#[test]
fn an_existing_repository_is_adopted_after_its_permission_and_collaborators_are_read() {
    let mut world = World::new(3);
    world.adopt();
    let row = world.stored().get(&top::Key::Repository(REPO));
    assert!(row.is_some());
    let mut adopted = false;
    for event in world.seen() {
        if let top::Request::Adopted { result: Ok(value), .. } = event {
            assert_eq!(value.repository.project, 5);
            assert!(!value.collaborators.is_empty());
            adopted = true;
        }
    }
    assert!(adopted);
    assert_eq!(world.writes(), 0);
}

#[test]
fn a_repository_with_another_deployments_branch_prefix_is_refused() {
    let mut world = World::new(4);
    world.produce(b"temper/other");
    world.adopt();
    assert!(
        world
            .seen()
            .iter()
            .any(|request| matches!(request, top::Request::Adopted { result: Err(client::api::Error::Refused), .. }))
    );
    assert!(world.stored().get(&top::Key::Repository(REPO)).is_none());
}

#[test]
fn a_context_repository_cannot_receive_a_write() {
    let mut world = World::new(5);
    world.adopt_as(top::Role::Context);
    world.event(top::Event::Enqueue {
        entry: client::Entry {
            number: 1,
            task: 9,
            repository: REPO,
            effect: client::Effect {
                write: client::api::Write::CreateIssue {
                    key: Box::from(&b"context"[..]),
                    title: Box::from(&b"No"[..]),
                    body: Box::from(&b"write"[..]),
                },
                condition: client::Condition::None,
            },
            start: None,
            attempt: None,
            failures: 0,
        },
    });
    world.event(top::Event::Committed { entry: 1 });
    world.run_for(2);
    assert!(world.seen().iter().any(|request| matches!(request, top::Request::Refused { task: 9 })));
    assert_eq!(world.writes(), 0);
}

fn change_row() -> top::ChangeRow {
    top::ChangeRow {
        task: 51,
        repository: REPO,
        branch: Box::from(&b"temper/51"[..]),
        base: Box::from(&b"main"[..]),
        title: Box::from(&b"Small fix"[..]),
        body: Box::from(&b"Fix the file"[..]),
        priority: 1,
        change: change::Change {
            task: 51,
            state: change::State::Producing { requested: false },
            gates: Box::new([]),
            clean: Box::new([]),
            repairs: 0,
            resolutions: 0,
            updates: 0,
            since: Wall::from_nanos(100_000_000_000),
            ready_since: None,
            owns_turn: false,
            last_head: None,
        },
        pull: None,
        pending: None,
        effect: change::EffectResult::None,
        delegate: None,
        delegate_status: change::Status::Unknown,
        verdicts: Box::new([]),
        drift: None,
        base_repair: false,
        queue_repair: None,
        queue_repairs: 0,
    }
}

fn change_step(
    world: &mut World,
    entry: u64,
    heard: change::Status,
    gates: Box<[change::GateReport]>,
) -> change::Decision {
    change_step_with_release(world, entry, heard, gates, false)
}

fn change_step_with_release(
    world: &mut World,
    entry: u64,
    heard: change::Status,
    gates: Box<[change::GateReport]>,
    released: bool,
) -> change::Decision {
    change_step_task(world, 51, entry, heard, gates, released)
}

fn change_step_task(
    world: &mut World,
    task: u64,
    entry: u64,
    heard: change::Status,
    gates: Box<[change::GateReport]>,
    released: bool,
) -> change::Decision {
    world.take_seen();
    world.event(top::Event::StepChange {
        task,
        entry,
        heard: change::Heard { delegate: heard, effect: change::EffectResult::None, released, cancelled: false },
        gates,
        queue_repair_active: false,
    });
    world.run_for(1);
    let mut decision = None;
    let mut committed = None;
    for event in world.take_seen() {
        if let top::Request::ChangeDecision { task: decided, decision: current, entry, .. } = event
            && decided == task
        {
            decision = Some(current);
            committed = entry;
        }
    }
    if let Some(entry) = committed {
        world.event(top::Event::Committed { entry });
        world.run_for(1);
    }
    decision.expect("change step made a decision")
}

fn reviewed_change(queued: bool, freshness: change::Freshness) -> (World, client::api::Commit) {
    let mut world = World::new(17);
    world.adopt();
    world.produce(b"temper/51");
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Passed);
    let pull = world.open_pull(b"temper/51", b"main");
    let head = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    let mut row = change_row();
    row.change.state = if queued {
        change::State::Queued { head, ready_since: Wall::from_nanos(100_000_000_000) }
    } else {
        change::State::Gating { head, asked: Box::new([]) }
    };
    row.change.last_head = Some(head);
    row.pull = Some(pull);
    row.change.gates =
        Box::new([change::Gate { number: 9, kind: change::GateKind::Agent, blocking: true, freshness, eager: false }]);
    world.event(top::Event::Change { row });
    (world, head)
}

#[test]
fn a_review_asking_for_changes_repeats_at_the_repaired_head() {
    let (mut world, head) = reviewed_change(false, change::Freshness::Exact);
    assert_eq!(
        change_step(&mut world, 1, change::Status::Unknown, Box::new([])),
        change::Decision::Delegate(change::Delegate::Gate { number: 9, head })
    );
    assert_eq!(
        change_step(
            &mut world,
            2,
            change::Status::Passed,
            Box::new([change::GateReport { number: 9, head, status: change::Status::Failed }])
        ),
        change::Decision::Delegate(change::Delegate::Repair(change::Repair::Gate(9)))
    );
    world.push(b"temper/51", b"review repair");
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Passed);
    let new_head = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    assert_eq!(
        change_step(&mut world, 3, change::Status::Passed, Box::new([])),
        change::Decision::Delegate(change::Delegate::Gate { number: 9, head: new_head })
    );
    let verdict = Box::new([change::GateReport { number: 9, head: new_head, status: change::Status::Passed }]);
    let mut finished = false;
    for entry in 4..18 {
        if let change::Decision::Finish { .. } = change_step(&mut world, entry, change::Status::Passed, verdict.clone())
        {
            finished = true;
            break;
        }
    }
    assert!(finished);
    assert_eq!(world.writes(), 1);
}

#[test]
fn a_gate_added_while_queued_takes_the_change_out_until_approved() {
    let (mut world, head) = reviewed_change(true, change::Freshness::Exact);
    assert_eq!(
        change_step(&mut world, 1, change::Status::Passed, Box::new([])),
        change::Decision::Delegate(change::Delegate::Gate { number: 9, head })
    );
    assert_eq!(world.writes(), 0);
    let verdict = Box::new([change::GateReport { number: 9, head, status: change::Status::Passed }]);
    let mut finished = false;
    for entry in 2..15 {
        if let change::Decision::Finish { .. } = change_step(&mut world, entry, change::Status::Passed, verdict.clone())
        {
            finished = true;
            break;
        }
    }
    assert!(finished);
    assert_eq!(world.writes(), 1);
}

#[test]
fn a_clean_update_keeps_its_review_and_lands_the_new_head() {
    let (mut world, head) = reviewed_change(true, change::Freshness::Clean);
    world.push_file(b"main", b"other", b"base moved");
    let verdict = Box::new([change::GateReport { number: 9, head, status: change::Status::Passed }]);
    assert!(matches!(
        change_step(&mut world, 1, change::Status::Passed, verdict.clone()),
        change::Decision::Effect(change::Effect::Update { .. })
    ));
    let updated = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    assert_ne!(updated, head);
    let mut finished = false;
    for entry in 2..16 {
        let decided = change_step(&mut world, entry, change::Status::Passed, verdict.clone());
        assert!(!matches!(decided, change::Decision::Delegate(change::Delegate::Gate { .. })));
        if let change::Decision::Finish { .. } = decided {
            finished = true;
            break;
        }
    }
    assert!(finished);
    assert_eq!(world.writes(), 2);
}

#[test]
fn an_exact_head_approval_is_asked_again_after_a_clean_update() {
    let (mut world, head) = reviewed_change(true, change::Freshness::Exact);
    world.push_file(b"main", b"other", b"base moved");
    let verdict = Box::new([change::GateReport { number: 9, head, status: change::Status::Passed }]);
    assert!(matches!(
        change_step(&mut world, 1, change::Status::Passed, verdict.clone()),
        change::Decision::Effect(change::Effect::Update { .. })
    ));
    let updated = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    assert_ne!(updated, head);
    assert_eq!(
        change_step(&mut world, 2, change::Status::Passed, verdict),
        change::Decision::Delegate(change::Delegate::Gate { number: 9, head: updated })
    );
    assert_eq!(world.writes(), 1);
}

#[test]
fn a_conflicting_update_is_resolved_from_a_merge_in_progress() {
    let (mut world, head) = reviewed_change(true, change::Freshness::Clean);
    world.push(b"main", b"conflicting base");
    let verdict = Box::new([change::GateReport { number: 9, head, status: change::Status::Passed }]);
    assert!(matches!(
        change_step(&mut world, 1, change::Status::Passed, verdict),
        change::Decision::Effect(change::Effect::Update { .. })
    ));
    let base = temper_engine_forge_world::translate::commit(world.branch(b"main"));
    assert_eq!(
        change_step(&mut world, 2, change::Status::Passed, Box::new([])),
        change::Decision::Delegate(change::Delegate::Resolve { base })
    );
    assert_eq!(world.writes(), 1);
    world.resolve(b"temper/51", b"resolved content");
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Passed);
    let resolved = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    assert_eq!(
        change_step(&mut world, 3, change::Status::Passed, Box::new([])),
        change::Decision::Delegate(change::Delegate::Gate { number: 9, head: resolved })
    );
    let verdict = Box::new([change::GateReport { number: 9, head: resolved, status: change::Status::Passed }]);
    let mut finished = false;
    for entry in 4..18 {
        if let change::Decision::Finish { .. } = change_step(&mut world, entry, change::Status::Passed, verdict.clone())
        {
            finished = true;
            break;
        }
    }
    assert!(finished);
    assert_eq!(world.writes(), 2);
}

#[test]
fn a_clean_update_with_broken_ci_is_repaired_as_a_semantic_conflict() {
    let (mut world, head) = reviewed_change(true, change::Freshness::Clean);
    world.push_file(b"main", b"other", b"base moved");
    let verdict = Box::new([change::GateReport { number: 9, head, status: change::Status::Passed }]);
    assert!(matches!(
        change_step(&mut world, 1, change::Status::Passed, verdict.clone()),
        change::Decision::Effect(change::Effect::Update { .. })
    ));
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Failed);
    assert_eq!(
        change_step(&mut world, 2, change::Status::Passed, verdict),
        change::Decision::Delegate(change::Delegate::Repair(change::Repair::Semantic))
    );
    assert_eq!(world.writes(), 1);
    world.push(b"temper/51", b"semantic repair");
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Passed);
    let repaired = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    assert_eq!(
        change_step(&mut world, 3, change::Status::Passed, Box::new([])),
        change::Decision::Delegate(change::Delegate::Gate { number: 9, head: repaired })
    );
}

#[test]
fn a_pull_closed_by_someone_else_is_held_then_reopened_on_release() {
    let (mut world, head) = reviewed_change(false, change::Freshness::Exact);
    let pull = match world.stored().get(&top::Key::Change(51)) {
        Some(top::Stored::Change(row)) => row.pull.expect("fixture pull"),
        Some(_) | None => panic!("fixture change"),
    };
    world.close(pull);
    assert_eq!(
        change_step(&mut world, 1, change::Status::Passed, Box::new([])),
        change::Decision::Hold(change::Hold::PullClosed)
    );
    assert_eq!(world.writes(), 0);
    world.event(top::Event::ReleaseChange { task: 51 });
    assert_eq!(
        change_step_with_release(&mut world, 2, change::Status::Passed, Box::new([]), true),
        change::Decision::Effect(change::Effect::Reopen)
    );
    assert_eq!(world.writes(), 1);
    assert_eq!(
        change_step(&mut world, 3, change::Status::Passed, Box::new([])),
        change::Decision::Delegate(change::Delegate::Gate { number: 9, head })
    );
}

#[test]
fn a_broken_landing_branch_pauses_the_queue_for_one_repair() {
    let (mut world, head) = reviewed_change(true, change::Freshness::Clean);
    world.status(b"main", temper_fake_forge_domain::api::Check::Failed);
    let report = Box::new([change::GateReport { number: 9, head, status: change::Status::Passed }]);
    assert_eq!(change_step(&mut world, 1, change::Status::Passed, report.clone()), change::Decision::QueueRepair);
    assert_eq!(world.writes(), 0);
    world.take_seen();
    world.event(top::Event::StepChange {
        task: 51,
        entry: 2,
        heard: change::Heard {
            delegate: change::Status::Passed,
            effect: change::EffectResult::None,
            released: false,
            cancelled: false,
        },
        gates: report,
        queue_repair_active: true,
    });
    world.run_for(1);
    assert!(world.seen().iter().any(|event| matches!(
        event,
        top::Request::ChangeDecision { task: 51, decision: change::Decision::Wait { .. }, .. }
    )));
    assert_eq!(world.writes(), 0);
}

#[test]
fn an_aged_low_priority_change_takes_its_turn_ahead_of_later_work() {
    let mut world = World::new(83);
    world.adopt();
    world.produce(b"temper/51");
    world.produce_file(b"temper/52", b"other", b"second change");
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Passed);
    world.status(b"temper/52", temper_fake_forge_domain::api::Check::Passed);
    let pull51 = world.open_pull(b"temper/51", b"main");
    let pull52 = world.open_pull(b"temper/52", b"main");
    let head51 = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    let head52 = temper_engine_forge_world::translate::commit(world.branch(b"temper/52"));
    let mut low = change_row();
    low.pull = Some(pull51);
    low.priority = 0;
    low.change.state = change::State::Queued { head: head51, ready_since: Wall::EPOCH };
    low.change.last_head = Some(head51);
    world.event(top::Event::Change { row: low });
    let mut high = change_row();
    high.task = 52;
    high.change.task = 52;
    high.branch = Box::from(&b"temper/52"[..]);
    high.pull = Some(pull52);
    high.priority = 10;
    high.change.state = change::State::Queued { head: head52, ready_since: Wall::from_nanos(100_000_000_000) };
    high.change.last_head = Some(head52);
    high.change.gates = Box::new([change::Gate {
        number: 7,
        kind: change::GateKind::Agent,
        blocking: true,
        freshness: change::Freshness::Clean,
        eager: false,
    }]);
    world.event(top::Event::Change { row: high });
    let review = Box::new([change::GateReport { number: 7, head: head52, status: change::Status::Passed }]);
    assert_eq!(
        change_step_task(&mut world, 52, 1, change::Status::Passed, review.clone(), false),
        change::Decision::Wait { until: None }
    );
    assert_eq!(world.writes(), 0);
    assert!(matches!(
        change_step(&mut world, 2, change::Status::Passed, Box::new([])),
        change::Decision::Effect(change::Effect::Merge { .. })
    ));
    assert_eq!(world.writes(), 1);
    assert!(matches!(
        change_step_task(&mut world, 52, 3, change::Status::Passed, review.clone(), false),
        change::Decision::Effect(change::Effect::Update { .. })
    ));
    let new_head = temper_engine_forge_world::translate::commit(world.branch(b"temper/52"));
    assert_ne!(new_head, head52);
    let mut finished = false;
    for entry in 4..16 {
        let decided = change_step_task(&mut world, 52, entry, change::Status::Passed, review.clone(), false);
        assert!(!matches!(decided, change::Decision::Delegate(change::Delegate::Gate { .. })));
        if let change::Decision::Finish { .. } = decided {
            finished = true;
            break;
        }
    }
    assert!(finished);
    assert_eq!(world.writes(), 3);
}

#[test]
fn a_failed_ci_head_is_repaired_then_checked_again() {
    let mut world = World::new(13);
    world.adopt();
    world.event(top::Event::Change { row: change_row() });
    assert_eq!(
        change_step(&mut world, 1, change::Status::Unknown, Box::new([])),
        change::Decision::Delegate(change::Delegate::Produce)
    );
    world.produce(b"temper/51");
    assert_eq!(
        change_step(&mut world, 2, change::Status::Passed, Box::new([])),
        change::Decision::Effect(change::Effect::Open)
    );
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Failed);
    assert_eq!(
        change_step(&mut world, 3, change::Status::Passed, Box::new([])),
        change::Decision::Delegate(change::Delegate::Repair(change::Repair::Ci))
    );
    let before = world.writes();
    world.push(b"temper/51", b"repaired");
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Passed);
    let mut finished = false;
    for entry in 4..20 {
        if let change::Decision::Finish { .. } = change_step(&mut world, entry, change::Status::Passed, Box::new([])) {
            finished = true;
            break;
        }
    }
    assert!(finished);
    assert_eq!(world.writes(), before + 1); // only the merge is a connector write
}

#[test]
fn a_change_produced_opened_checked_queued_and_landed() {
    let mut world = World::new(11);
    world.adopt();
    world.take_seen();
    world.event(top::Event::Change { row: change_row() });
    world.event(top::Event::StepChange {
        task: 51,
        entry: 2,
        heard: change::Heard {
            delegate: change::Status::Unknown,
            effect: change::EffectResult::None,
            released: false,
            cancelled: false,
        },
        gates: Box::new([]),
        queue_repair_active: false,
    });
    world.run_for(1);
    assert!(world.seen().iter().any(|request| matches!(
        request,
        top::Request::ChangeDecision { task: 51, decision: change::Decision::Delegate(change::Delegate::Produce), .. }
    )));
    world.produce(b"temper/51");
    world.take_seen();
    world.event(top::Event::StepChange {
        task: 51,
        entry: 3,
        heard: change::Heard {
            delegate: change::Status::Passed,
            effect: change::EffectResult::None,
            released: false,
            cancelled: false,
        },
        gates: Box::new([]),
        queue_repair_active: false,
    });
    world.run_for(1);
    assert!(world.seen().iter().any(|request| matches!(
        request,
        top::Request::ChangeDecision {
            task: 51,
            decision: change::Decision::Effect(change::Effect::Open),
            entry: Some(3),
            ..
        }
    )));
    assert_eq!(world.writes(), 0);
    world.event(top::Event::Committed { entry: 3 });
    world.run_for(2);
    assert_eq!(world.writes(), 1);
    let mut finished = false;
    for entry in 4..24 {
        world.take_seen();
        world.event(top::Event::StepChange {
            task: 51,
            entry,
            heard: change::Heard {
                delegate: change::Status::Passed,
                effect: change::EffectResult::None,
                released: false,
                cancelled: false,
            },
            gates: Box::new([]),
            queue_repair_active: false,
        });
        world.run_for(1);
        let replies = world.take_seen();
        for reply in replies {
            match reply {
                top::Request::ChangeDecision { decision: change::Decision::Effect(_), entry: Some(number), .. } => {
                    world.event(top::Event::Committed { entry: number });
                    world.run_for(1);
                }
                top::Request::ChangeDecision { decision: change::Decision::Finish { .. }, .. } => finished = true,
                top::Request::ChangeDecision { decision: change::Decision::Delegate(_), .. } => {
                    panic!("unexpected delegate")
                }
                top::Request::ChangeDecision { .. }
                | top::Request::Adopted { .. }
                | top::Request::Save { .. }
                | top::Request::Erase { .. }
                | top::Request::Taken { .. }
                | top::Request::Refused { .. }
                | top::Request::Outcome { .. }
                | top::Request::ContinueRelease { .. }
                | top::Request::Released { .. }
                | top::Request::ReleaseFailed { .. }
                | top::Request::ProjectAfter { .. }
                | top::Request::ProjectionFailed { .. }
                | top::Request::News { .. }
                | top::Request::Drift { .. }
                | top::Request::Read { .. }
                | top::Request::Call { .. } => {}
            }
        }
        if finished {
            break;
        }
    }
    assert!(finished, "change should land");
    assert_eq!(world.writes(), 2);
}

#[test]
fn a_goal_issue_is_created_only_after_its_outbox_commit() {
    let mut world = World::new(7);
    world.adopt();
    let view = issues::GoalView {
        goal: 42,
        repository: u64::from(REPO.repository),
        title: Box::from("Goal"),
        goal_text: Box::from("Plan"),
        plan: Box::new([]),
        milestones: Box::new([]),
        finished: None,
    };
    world.event(top::Event::Project { entry: 1, repository: REPO, view });
    world.run_for(1);
    assert_eq!(world.writes(), 0);
    world.event(top::Event::Committed { entry: 1 });
    world.run_for(2);
    assert_eq!(world.writes(), 1);
    let mut made = false;
    for event in world.seen() {
        if let top::Request::Outcome { entry: 1, outcome: client::Outcome::Made { .. }, .. } = event {
            made = true;
        }
    }
    assert!(made);
    assert!(world.stored().get(&top::Key::Entry(1)).is_none());
    match world.stored().get(&top::Key::Issue(42)) {
        Some(top::Stored::Issue(row)) => {
            assert!(row.number.is_some());
            assert_eq!(row.pending, None);
        }
        Some(_) | None => panic!("committed issue projection"),
    }
}

#[test]
fn a_goals_issue_is_revised_and_closed_as_the_goal_finishes() {
    let mut world = World::new(71);
    world.adopt();
    let mut view = issues::GoalView {
        goal: 42,
        repository: u64::from(REPO.repository),
        title: Box::from("Goal"),
        goal_text: Box::from("Plan"),
        plan: Box::new([]),
        milestones: Box::new([]),
        finished: None,
    };
    world.event(top::Event::Project { entry: 1, repository: REPO, view: view.clone() });
    world.event(top::Event::Committed { entry: 1 });
    world.run_for(2);
    view.goal_text = Box::from("Revised plan");
    world.run_for(6);
    world.event(top::Event::Project { entry: 2, repository: REPO, view: view.clone() });
    world.event(top::Event::Committed { entry: 2 });
    world.run_for(2);
    assert_eq!(world.writes(), 2);
    view.finished = Some(Box::from("Accepted"));
    let mut closed = false;
    for entry in 3..10 {
        world.run_for(6);
        world.event(top::Event::Project { entry, repository: REPO, view: view.clone() });
        if world.stored().get(&top::Key::Entry(entry)).is_some() {
            world.event(top::Event::Committed { entry });
            world.run_for(2);
        }
        if let Some(top::Stored::Issue(row)) = world.stored().get(&top::Key::Issue(42))
            && row.state.closed
        {
            closed = true;
            break;
        }
    }
    assert!(closed);
    assert!(world.writes() >= 3);
}

#[test]
fn a_created_issue_with_its_answer_lost_is_found_after_restart() {
    let mut world = World::new(23);
    world.adopt();
    world.slow_calls(skein_lib::Duration::from_secs(1));
    let view = issues::GoalView {
        goal: 77,
        repository: u64::from(REPO.repository),
        title: Box::from("Goal"),
        goal_text: Box::from("Plan"),
        plan: Box::new([]),
        milestones: Box::new([]),
        finished: None,
    };
    world.event(top::Event::Project { entry: 1, repository: REPO, view });
    world.event(top::Event::Committed { entry: 1 });
    world.run_for(1);
    assert_eq!(world.writes(), 1);
    world.restart();
    world.run_for(20);
    assert_eq!(world.writes(), 1);
    assert!(world.stored().get(&top::Key::Entry(1)).is_none());
    match world.stored().get(&top::Key::Issue(77)) {
        Some(top::Stored::Issue(row)) => assert!(row.number.is_some()),
        Some(_) | None => panic!("recovered issue"),
    }
}

#[test]
fn a_merge_with_its_answer_lost_is_found_after_restart_without_merging_twice() {
    let mut world = World::new(89);
    world.adopt();
    world.produce(b"temper/51");
    let pull = world.open_pull(b"temper/51", b"main");
    let head = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    world.slow_calls(skein_lib::Duration::from_secs(5));
    world.event(top::Event::Enqueue {
        entry: client::Entry {
            number: 1,
            task: 51,
            repository: REPO,
            effect: client::Effect {
                write: client::api::Write::Merge { number: pull, head },
                condition: client::Condition::Merge { base: Box::from(&b"main"[..]) },
            },
            start: None,
            attempt: None,
            failures: 0,
        },
    });
    world.event(top::Event::Committed { entry: 1 });
    world.run_for(11);
    assert_eq!(world.writes(), 1);
    assert!(world.stored().get(&top::Key::Entry(1)).is_some());
    let landed = world.branch(b"main");
    world.restart();
    world.run_for(30);
    assert_eq!(world.writes(), 1);
    assert_eq!(world.branch(b"main"), landed);
    assert!(world.stored().get(&top::Key::Entry(1)).is_none());
}

#[test]
fn a_merge_decided_for_an_old_head_cannot_land_the_new_head() {
    let mut world = World::new(90);
    world.adopt();
    world.produce(b"temper/51");
    let pull = world.open_pull(b"temper/51", b"main");
    let old_head = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    let landing = world.branch(b"main");
    world.push(b"temper/51", b"later work");
    world.event(top::Event::Enqueue {
        entry: client::Entry {
            number: 1,
            task: 51,
            repository: REPO,
            effect: client::Effect {
                write: client::api::Write::Merge { number: pull, head: old_head },
                condition: client::Condition::Merge { base: Box::from(&b"main"[..]) },
            },
            start: None,
            attempt: None,
            failures: 0,
        },
    });
    world.event(top::Event::Committed { entry: 1 });
    world.run_for(10);
    assert_eq!(world.branch(b"main"), landing);
    assert_eq!(world.writes(), 0);
}

#[test]
fn a_reported_worker_push_is_not_taken_for_outside_drift() {
    let mut world = World::new(91);
    world.adopt();
    let name = top::Name {
        forge: REPO.forge,
        repository: REPO.repository,
        what: top::What::Branch(Box::new([Box::from(&b"temper"[..]), Box::from(&b"51"[..])])),
    };
    world.event(top::Event::Names { task: 51, resources: Box::new([name.clone()]) });
    world.event(top::Event::Hold { task: 51, resource: name.clone(), from: None });
    world.event(top::Event::Claim { task: 51, attempt: 1, writes: Box::new([name.clone()]), holders: Box::new([]) });
    world.produce(b"temper/51");
    let pushed = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    world.event(top::Event::Answered { task: 51, attempt: 1, pushed: Box::new([(name, pushed)]) });
    world.run_for(1);
    world.take_seen();
    world.event(top::Event::Hint {
        hint: client::api::Hint {
            repository: REPO,
            change: client::api::Change::Branch(Box::from(&b"temper/51"[..])),
            key: None,
        },
    });
    world.run_for(5);
    assert!(!world.seen().iter().any(|event| matches!(event, top::Request::Drift { task: 51, .. })));
}

#[test]
fn closing_a_landed_change_deletes_its_branch_before_releasing_the_task() {
    let mut world = World::new(92);
    world.adopt();
    world.produce(b"temper/51");
    let name = top::Name {
        forge: REPO.forge,
        repository: REPO.repository,
        what: top::What::Branch(Box::new([Box::from(&b"temper"[..]), Box::from(&b"51"[..])])),
    };
    world.event(top::Event::Hold { task: 51, resource: name.clone(), from: None });
    world.take_seen();
    world.event(top::Event::Release { task: 51, root: 51, ending: top::ReleaseEnding::Done, entry: 10 });
    assert!(world.stored().contains_key(&top::Key::Release(51)));
    assert!(world.stored().contains_key(&top::Key::Entry(10)));
    assert!(!world.seen().iter().any(|request| matches!(request, top::Request::Released { task: 51 })));
    world.event(top::Event::Committed { entry: 10 });
    world.run_for(5);
    assert!(world.seen().iter().any(|request| matches!(request, top::Request::ContinueRelease { task: 51 })));
    world.event(top::Event::ContinueRelease { task: 51, entry: 11 });
    assert!(world.seen().iter().any(|request| matches!(request, top::Request::Released { task: 51 })));
    assert!(!world.stored().contains_key(&top::Key::Hold(name)));
    assert!(!world.stored().contains_key(&top::Key::Release(51)));
    assert_eq!(world.writes(), 1);
}

#[test]
fn a_ci_subscription_reads_its_head_and_restarts_without_repeating_old_news() {
    let mut world = World::new(94);
    world.adopt();
    world.produce(b"temper/51");
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Passed);
    let head = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    let topic = top::Topic::Ci { repository: REPO, head };
    world.event(top::Event::Subscribe {
        subscription: top::Subscriber { task: 51, number: 1, topic, own_change: None, paths: Box::new([]) },
    });
    world.run_for(2);
    assert!(world.seen().iter().any(|request| matches!(request, top::Request::News { task: 51, news: top::News::Ci { head: seen, .. }, .. } if *seen == head)));
    world.take_seen();
    world.restart();
    world.run_for(2);
    assert!(
        !world
            .seen()
            .iter()
            .any(|request| matches!(request, top::Request::News { task: 51, news: top::News::Ci { .. }, .. }))
    );
    world.status(b"temper/51", temper_fake_forge_domain::api::Check::Failed);
    world.event(top::Event::Hint {
        hint: client::api::Hint { repository: REPO, change: client::api::Change::Commit(head), key: None },
    });
    world.run_for(2);
    assert!(world.seen().iter().any(|request| matches!(
        request,
        top::Request::News { task: 51, news: top::News::Ci { status: client::api::Ci::Failed, .. }, .. }
    )));
    assert!(matches!(
        world.stored().get(&top::Key::Ci { repository: REPO, head }),
        Some(top::Stored::Ci(top::CiState { status: client::api::Ci::Failed, .. }))
    ));
}

#[test]
fn cancelling_a_change_closes_its_pull_before_deleting_its_branch() {
    let mut world = World::new(93);
    world.adopt();
    world.produce(b"temper/51");
    let pull = world.open_pull(b"temper/51", b"main");
    let mut row = change_row();
    row.pull = Some(pull);
    world.event(top::Event::Change { row });
    let name = top::Name {
        forge: REPO.forge,
        repository: REPO.repository,
        what: top::What::Branch(Box::new([Box::from(&b"temper"[..]), Box::from(&b"51"[..])])),
    };
    world.event(top::Event::Hold { task: 51, resource: name, from: None });
    world.take_seen();
    world.event(top::Event::Release { task: 51, root: 51, ending: top::ReleaseEnding::Cancelled, entry: 10 });
    assert!(
        matches!(world.stored().get(&top::Key::Entry(10)), Some(top::Stored::Entry(client::Entry { effect: client::Effect { write: client::api::Write::Close { number }, .. }, .. })) if *number == pull)
    );
    world.event(top::Event::Committed { entry: 10 });
    world.run_for(5);
    world.event(top::Event::ContinueRelease { task: 51, entry: 11 });
    assert!(matches!(
        world.stored().get(&top::Key::Entry(11)),
        Some(top::Stored::Entry(client::Entry {
            effect: client::Effect { write: client::api::Write::DeleteBranch { .. }, .. },
            ..
        }))
    ));
    world.event(top::Event::Committed { entry: 11 });
    world.run_for(5);
    world.event(top::Event::ContinueRelease { task: 51, entry: 12 });
    assert!(world.seen().iter().any(|request| matches!(request, top::Request::Released { task: 51 })));
    assert_eq!(world.writes(), 2);
}

#[test]
fn an_external_move_of_a_held_branch_is_reported_as_drift() {
    let mut world = World::new(31);
    world.adopt();
    let name = top::Name {
        forge: REPO.forge,
        repository: REPO.repository,
        what: top::What::Branch(Box::new([Box::from(&b"temper"[..]), Box::from(&b"51"[..])])),
    };
    world.event(top::Event::Change { row: change_row() });
    world.event(top::Event::Names { task: 51, resources: Box::new([name.clone()]) });
    world.event(top::Event::Hold { task: 51, resource: name.clone(), from: None });
    world.run_for(1);
    world.event(top::Event::Claim { task: 51, attempt: 1, writes: Box::new([name.clone()]), holders: Box::new([]) });
    world.produce(b"temper/51");
    let pushed = temper_engine_forge_world::translate::commit(world.branch(b"temper/51"));
    world.event(top::Event::Answered { task: 51, attempt: 1, pushed: Box::new([(name.clone(), pushed)]) });
    world.run_for(1);
    world.take_seen();
    world.push(b"temper/51", b"outside");
    world.event(top::Event::Hint {
        hint: client::api::Hint {
            repository: REPO,
            change: client::api::Change::Branch(Box::from(&b"temper/51"[..])),
            key: None,
        },
    });
    world.run_for(5);
    assert!(world.seen().iter().any(|event| matches!(event,
        top::Request::Drift { task: 51, resource } if resource == &name)));
    world.take_seen();
    world.event(top::Event::StepChange {
        task: 51,
        entry: 10,
        heard: change::Heard {
            delegate: change::Status::Passed,
            effect: change::EffectResult::None,
            released: false,
            cancelled: false,
        },
        gates: Box::new([]),
        queue_repair_active: false,
    });
    world.run_for(1);
    assert!(world.seen().iter().any(|event| matches!(
        event,
        top::Request::ChangeDecision { task: 51, decision: change::Decision::Hold(change::Hold::BranchMoved), .. }
    )));
    world.event(top::Event::ReleaseChange { task: 51 });
    world.take_seen();
    world.event(top::Event::StepChange {
        task: 51,
        entry: 11,
        heard: change::Heard {
            delegate: change::Status::Passed,
            effect: change::EffectResult::None,
            released: true,
            cancelled: false,
        },
        gates: Box::new([]),
        queue_repair_active: false,
    });
    world.run_for(1);
    assert!(!world.seen().iter().any(|event| matches!(
        event,
        top::Request::ChangeDecision { task: 51, decision: change::Decision::Hold(change::Hold::BranchMoved), .. }
    )));
}

#[test]
fn idle_forge_call_cost_does_not_follow_old_issue_history() {
    let mut small = World::new(47);
    small.preload_history(2);
    small.adopt();
    small.event(top::Event::Names {
        task: 1,
        resources: Box::new([top::Name {
            forge: REPO.forge,
            repository: REPO.repository,
            what: top::What::Branch(Box::new([Box::from(&b"main"[..])])),
        }]),
    });
    small.run_for(60);
    let before = small.calls();
    small.run_for(60);
    let small_cost = small.calls() - before;

    let mut large = World::new(47);
    large.preload_history(20);
    large.adopt();
    large.event(top::Event::Names {
        task: 1,
        resources: Box::new([top::Name {
            forge: REPO.forge,
            repository: REPO.repository,
            what: top::What::Branch(Box::new([Box::from(&b"main"[..])])),
        }]),
    });
    large.run_for(60);
    let before = large.calls();
    large.run_for(60);
    assert_eq!(large.calls() - before, small_cost);
}

#[test]
fn a_landing_wakes_overlapping_work_and_keeps_unrelated_news() {
    let mut world = World::new(53);
    world.adopt();
    let topic = top::Topic::Landings { repository: REPO, branch: Box::from(&b"main"[..]) };
    let name = top::Name {
        forge: REPO.forge,
        repository: REPO.repository,
        what: top::What::Branch(Box::new([Box::from(&b"main"[..])])),
    };
    world.event(top::Event::Names { task: 1, resources: Box::new([name]) });
    world.event(top::Event::Subscribe {
        subscription: top::Subscriber {
            task: 1,
            number: 1,
            topic: topic.clone(),
            own_change: None,
            paths: Box::new([Box::from(&b"file"[..])]),
        },
    });
    world.event(top::Event::Subscribe {
        subscription: top::Subscriber {
            task: 2,
            number: 2,
            topic,
            own_change: None,
            paths: Box::new([Box::from(&b"elsewhere"[..])]),
        },
    });
    world.run_for(1);
    world.take_seen();
    world.push(b"main", b"landed");
    world.event(top::Event::Hint {
        hint: client::api::Hint {
            repository: REPO,
            change: client::api::Change::Branch(Box::from(&b"main"[..])),
            key: None,
        },
    });
    world.run_for(5);
    assert!(world.seen().iter().any(|request| matches!(
        request,
        top::Request::News { task: 1, news: top::News::Landing { .. }, class: top::Class::Wakes, .. }
    )));
    assert!(world.seen().iter().any(|request| matches!(
        request,
        top::Request::News { task: 2, news: top::News::Landing { .. }, class: top::Class::Kept, .. }
    )));
}

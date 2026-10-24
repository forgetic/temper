//! Panels through the real engine root where the client's fixed fixture cannot reach the state.
use skein_lib::{ReplyTo, Token};
use temper_engine_domain as engine;
use temper_engine_domain_forge as forge;
use temper_engine_domain_world::{direct::Driver, fixtures};
use temper_engine_forge_world::changes::change_row;

fn config() -> engine::Config {
    use jig_domain_authority as a;
    let mut config = temper_engine_domain_world::walking::config(71);
    config.core.connectors = Box::new([1]);
    for role in &mut config.core.seed_projects[0].policy.permissions {
        role.connector = 1;
    }
    let mut rules = config.core.defaults.rules.clone();
    for grant in &mut rules.ceiling.grants {
        grant.connector = 1;
    }
    let mut policy = config.core.seed_projects[0].policy.policy.clone();
    for grant in &mut policy.ceiling.grants {
        grant.connector = 1;
    }
    for grant in &mut policy.projections {
        grant.connector = 1;
    }
    for role in &mut policy.roles {
        for grant in &mut role.authority.grants {
            grant.connector = 1;
        }
    }
    let mut authority =
        a::Domain::new(rules, temper_engine_domain_world::walking::limits().authority).expect("forge-number policy");
    let mut out = skein_lib::Queue::with_capacity(a::POLICY_MAX_OUT);
    a::step(&mut authority, a::Event::Policy { project: 1, policy }, &mut out);
    assert_eq!(out.pop(), Some(a::PolicyFact::Added { project: 1 }));
    config.core.seed_projects[0].policy.policy = authority.policy(1).expect("configured seed policy").clone();
    config.core.defaults.rules = authority.rules().clone();
    config
}

fn driver() -> (Driver, u64) {
    driver_with_limits(&fixtures::forge_limits())
}

fn driver_with_limits(limits: &engine::Limits) -> (Driver, u64) {
    use temper_engine_domain_world::{forge_stand::ForgeStand, store::Store};
    let mut driver = Driver::configured(Store::new(), [config(), config()], limits);
    driver.forge = Some(ForgeStand::new());
    driver.settle();
    driver.sign_in();
    driver.settle();
    let sign_in = driver.session();
    driver.send(engine::Event::Party(engine::PartyInput::Application {
        scope: jig_domain_people::ApplicationScope::Project(1),
        reply_to: ReplyTo::new(Token::new(900)),
        sign_in,
        key: [71; 16],
        digest: [71; 32],
        carried: None,
        request: temper_engine_domain::Application::Forge(forge::Ask::Adopt(fixtures::adoption(1))),
    }));
    driver.settle();
    (driver, sign_in)
}

fn watch(driver: &mut Driver, sign_in: u64, project: u32, task: u64, watcher: u64) -> &engine::Delivery {
    driver.send(engine::Event::Party(engine::PartyInput::Watch {
        watcher: Token::new(watcher),
        sign_in,
        subject: jig_domain_core::documents::Watched::Application {
            scope: jig_domain_people::ApplicationScope::Project(project),
            connector: 1,
            subject: Token::new(task),
        },
        window: jig_domain_core::documents::Window { rows: 0, after: None },
    }));
    driver.settle();
    driver.delivered.iter().rev().find(|delivery| matches!(delivery,
        engine::Delivery::Panel { watcher: token, .. } | engine::Delivery::WatchEnded { watcher: token, .. } if *token == Token::new(watcher)
    )).unwrap_or_else(|| panic!("panel watch must be answered: {:?}", driver.delivered))
}

// domain/forge.md, 8.8: a task absent from the connector's change records is unknown.
#[test]
fn a_task_without_a_change_record_has_no_panel() {
    let (mut driver, sign_in) = driver();
    let answer = watch(&mut driver, sign_in, 1, 51, 901);
    assert!(
        matches!(answer, engine::Delivery::WatchEnded { .. }),
        "design domain/forge.md 8.8: a task that is no change is unknown: {answer:?}"
    );
}

// domain/forge.md, 8.8: a producing change has a whole panel before its branch is pushed.
#[test]
fn producing_change_panel_has_no_head_or_pull() {
    let (mut driver, sign_in) = driver();
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: change_row() })));
    driver.settle();
    let engine::Delivery::Panel { panel, .. } = watch(&mut driver, sign_in, 1, 51, 902) else { unreachable!() };
    assert!(panel.head.is_none());
    assert!(panel.pull.is_none());
    assert!(matches!(panel.step, forge::PanelStep::Producing));
}

// domain/forge.md, 8.8: the same change watched in a different project's scope is unknown.
#[test]
fn another_projects_change_is_unknown() {
    let (mut driver, sign_in) = driver();
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: change_row() })));
    driver.settle();
    let foreign = driver
        .store
        .rows
        .values()
        .find_map(|record| {
            if let engine::Record::Forge { row } = record
                && let forge::Stored::Repository(repository) = row.as_ref()
            {
                let mut repository = repository.clone();
                repository.project = 2;
                Some(forge::Stored::Repository(repository))
            } else {
                None
            }
        })
        .expect("adopted repository");
    driver.send(engine::Event::Forge(Box::new(forge::Event::Restore { record: foreign })));
    driver.settle();
    let answer = watch(&mut driver, sign_in, 1, 51, 903);
    assert!(
        matches!(answer, engine::Delivery::WatchEnded { .. }),
        "design domain/forge.md 8.8: an admitted watch in the wrong project must answer unknown: {answer:?}"
    );
}

fn queued(task: u64, priority: i32, ready: u64) -> forge::ChangeRow {
    let mut row = change_row();
    let head = [4; 32];
    row.task = task;
    row.change.task = task;
    row.priority = priority;
    row.change.last_head = Some(head);
    row.change.ready_since = Some(skein_lib::Wall::from_nanos(ready));
    row.change.state =
        temper_engine_domain_forge_change::State::Queued { head, ready_since: skein_lib::Wall::from_nanos(ready) };
    row
}

// domain/forge.md, 8.8 and 9: equal-priority queue order is readiness order, ahead nearest first.
#[test]
fn queued_panel_lists_the_nearest_tasks_first() {
    let (mut driver, sign_in) = driver();
    for task in 51..=55 {
        driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(task, 1, task) })));
    }
    driver.settle();
    let engine::Delivery::Panel { panel, .. } = watch(&mut driver, sign_in, 1, 55, 904) else { unreachable!() };
    let forge::PanelStep::Queued { place, ahead } = &panel.step else {
        panic!("design domain/forge.md 8.8: queued change must have its place: {panel:?}");
    };
    assert_eq!(*place, 5, "design domain/forge.md 9: earlier ready changes go first");
    assert_eq!(ahead.as_ref(), &[54, 53, 52, 51], "design domain/forge.md 8.8: ahead is nearest first");
}

// domain/forge.md, 8.8: changing another change's priority marks the watched changes behind it.
#[test]
#[ignore = "needs: committed priority-change hook; Change creates records and ignores existing tasks"]
fn priority_moving_ahead_marks_the_waiting_panel() {
    let (mut driver, sign_in) = driver();
    for task in 51..=53 {
        driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(task, 1, task) })));
    }
    driver.settle();
    watch(&mut driver, sign_in, 1, 52, 905);
    driver.send(engine::Event::Party(engine::PartyInput::Delivered { watcher: Token::new(905) }));
    driver.delivered.clear();
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(53, 0, 53) })));
    driver.settle();
    let saved_priority = driver.store.rows.values().find_map(|record| {
        if let engine::Record::Forge { row } = record
            && let forge::Stored::Change(row) = row.as_ref()
            && row.task == 53
        {
            Some(row.priority)
        } else {
            None
        }
    });
    assert_eq!(
        saved_priority,
        Some(0),
        "setup: the existing Change event must save the new priority before testing its marks"
    );
    let persisted = driver
        .store
        .rows
        .values()
        .filter(|record| {
            matches!(record,
                engine::Record::Forge { row } if matches!(row.as_ref(), forge::Stored::Change(_))
            )
        })
        .count();
    assert!(
        driver.delivered.iter().any(|delivery| matches!(delivery,
            engine::Delivery::Panel { watcher, .. } if *watcher == Token::new(905)
        )),
        "design domain/forge.md 8.8: a change moving ahead must mark the waiting panel; durable change rows={persisted}: {:?}",
        driver.delivered
    );
}

fn reviewed_row(freshness: temper_engine_domain_forge_change::Freshness) -> forge::ChangeRow {
    use temper_engine_domain_forge_change as change;
    let old = [2; 32];
    let head = [3; 32];
    let mut row = change_row();
    row.change.state = change::State::Gating { head, asked: Box::new([]) };
    row.change.last_head = Some(head);
    row.change.clean = Box::new([old, head]);
    row.change.gates =
        Box::new([change::Gate { number: 9, kind: change::GateKind::Agent, blocking: true, freshness, eager: false }]);
    row.verdicts = Box::new([change::GateReport { number: 9, head: old, status: change::Status::Passed }]);
    row
}

// domain/forge.md, 8.8 and 8.3: a clean gate counts at a head reached by temper's clean update.
#[test]
fn clean_update_carries_the_gate_verdict_to_the_current_head() {
    use temper_engine_domain_forge_change as change;
    let (mut driver, sign_in) = driver();
    let head = [3; 32];
    let row = reviewed_row(change::Freshness::Clean);
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row })));
    driver.settle();
    let engine::Delivery::Panel { panel, .. } = watch(&mut driver, sign_in, 1, 51, 906) else { unreachable!() };
    assert_eq!(panel.head, Some(head));
    assert_eq!(
        panel.gates[0].verdict,
        change::Status::Passed,
        "design domain/forge.md 8.8: the gate at the current head must include its clean carry: {panel:?}"
    );
}

// domain/forge.md, 8.8: restart forgets CI until the change's next step reads it.
#[test]
fn restart_forgets_ci_until_the_next_step_reads_it() {
    use temper_engine_domain_forge_change as change;
    use temper_engine_domain_world::{forge_stand::ForgeStand, store::Store};
    let mut driver = Driver::configured(Store::new(), [config(), config()], &fixtures::forge_limits());
    driver.forge = Some(ForgeStand::with_checks(&[(b"org", b"repo", 2), (b"org", b"private", 3)], &[b"build"]));
    driver.settle();
    driver.sign_in();
    driver.settle();
    let sign_in = driver.session();
    driver.send(engine::Event::Party(engine::PartyInput::Application {
        scope: jig_domain_people::ApplicationScope::Project(1),
        reply_to: ReplyTo::new(Token::new(900)),
        sign_in,
        key: [71; 16],
        digest: [71; 32],
        carried: None,
        request: temper_engine_domain::Application::Forge(forge::Ask::Adopt(fixtures::adoption(1))),
    }));
    driver.settle();
    let stand = driver.forge.as_mut().expect("fake forge");
    stand.push_branch(2, b"temper/51");
    let skein_forge::api::Answer::Commit(head) = stand.read(
        temper_engine_forge_world::REPO,
        skein_forge::api::Read::Branch { branch: b"temper/51".as_slice().into() },
    ) else {
        panic!("pushed head")
    };
    let mut row = change_row();
    row.change.state = change::State::Checking { head };
    row.change.last_head = Some(head);
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row })));
    driver.send(engine::Event::Forge(Box::new(forge::Event::StepChange {
        task: 51,
        entry: 990,
        heard: change::Heard {
            delegate: change::Status::Unknown,
            effect: change::EffectResult::None,
            released: false,
            cancelled: false,
        },
        gates: Box::new([]),
        queue_repair_active: false,
        kept: false,
    })));
    driver.settle();
    let engine::Delivery::Panel { panel, .. } = watch(&mut driver, sign_in, 1, 51, 907) else { unreachable!() };
    assert_eq!(panel.ci, change::Status::Passed, "setup: the change's step must first read passing CI");
    let limits = driver.env.limits;
    let stand = driver.forge.take();
    let store = std::mem::replace(&mut driver.store, Store::new());
    let mut restored = Driver::configured(store, [config(), config()], &limits);
    restored.forge = stand;
    restored.settle();
    restored.sign_in();
    restored.settle();
    let session = restored.session();
    let engine::Delivery::Panel { panel, .. } = watch(&mut restored, session, 1, 51, 908) else { unreachable!() };
    assert_eq!(
        panel.ci,
        change::Status::Unknown,
        "design domain/forge.md 8.8: a restarted panel must forget CI until a step reads it: {panel:?}"
    );
    restored.send(engine::Event::Forge(Box::new(forge::Event::StepChange {
        task: 51,
        entry: 991,
        heard: change::Heard {
            delegate: change::Status::Unknown,
            effect: change::EffectResult::None,
            released: false,
            cancelled: false,
        },
        gates: Box::new([]),
        queue_repair_active: false,
        kept: false,
    })));
    restored.settle();
    let engine::Delivery::Panel { panel, .. } = watch(&mut restored, session, 1, 51, 909) else { unreachable!() };
    assert_eq!(
        panel.ci,
        change::Status::Passed,
        "design domain/forge.md 8.8: the next change step must make its freshly read CI visible again: {panel:?}"
    );
}

// domain/root.md, 10 and domain/forge.md, 8.8: 32 marks bound notification, while each new document computes the current place.
#[test]
fn queue_entry_obeys_the_mark_bound_and_a_reopen_reads_the_new_place() {
    use temper_engine_domain_world::walking;
    let mut limits = fixtures::forge_limits();
    limits.forge.changes = 40;
    limits.journal.writes = walking::writes(&limits);
    let (mut driver, sign_in) = driver_with_limits(&limits);
    for task in 51..=85 {
        driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(task, 1, task) })));
    }
    driver.settle();
    watch(&mut driver, sign_in, 1, 52, 910);
    watch(&mut driver, sign_in, 1, 85, 911);
    watch(&mut driver, sign_in, 1, 81, 916);
    watch(&mut driver, sign_in, 1, 82, 917);
    for watcher in [910, 911, 916, 917] {
        driver.send(engine::Event::Party(engine::PartyInput::Delivered { watcher: Token::new(watcher) }));
    }
    driver.delivered.clear();
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(50, 1, 50) })));
    driver.settle();
    assert!(
        driver.delivered.iter().any(|delivery| matches!(delivery,
            engine::Delivery::Panel { watcher, .. } if *watcher == Token::new(910)
        )),
        "design domain/forge.md 8.8: a nearby queued panel must be marked when another enters ahead: {:?}",
        driver.delivered
    );
    assert!(
        driver.delivered.iter().any(|delivery| matches!(delivery,
            engine::Delivery::Panel { watcher, .. } if *watcher == Token::new(916)
        )),
        "design domain/root.md 10: the 31st subject behind, plus the changed subject, must fit 32 marks"
    );
    assert!(
        !driver.delivered.iter().any(|delivery| matches!(delivery,
            engine::Delivery::Panel { watcher, .. } if *watcher == Token::new(917)
        )),
        "design domain/root.md 10: the changed subject plus 32 behind would exceed 32 marks"
    );
    assert!(
        !driver.delivered.iter().any(|delivery| matches!(delivery,
            engine::Delivery::Panel { watcher, .. } if *watcher == Token::new(911)
        )),
        "design domain/root.md 10: a panel beyond the 32-subject bound must await its own mark"
    );
    let engine::Delivery::Panel { panel, .. } = watch(&mut driver, sign_in, 1, 85, 921) else { unreachable!() };
    let forge::PanelStep::Queued { place, ahead } = &panel.step else { panic!("still queued: {panel:?}") };
    assert_eq!(*place, 36, "design domain/forge.md 8.8: a newly opened panel must compute the queue's new place");
    assert_eq!(ahead.len(), 35);
}

// domain/forge.md, 9: aging moves an older ready change ahead regardless of its priority.
#[test]
#[ignore = "defect outside M1: the landing queue orders the higher priority first (forge-change queue.rs), where domain/forge.md 9 and agent.md 4.3 put the lower first; left to the forge-domain mission (429)"]
fn aging_moves_an_older_ready_change_ahead_of_a_higher_priority() {
    let (mut driver, sign_in) = driver();
    driver.fire_at(skein_lib::Duration::from_secs(10));
    driver.settle();
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(51, 100, 0) })));
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(52, 0, 10_000_000_000) })));
    driver.settle();
    let engine::Delivery::Panel { panel, .. } = watch(&mut driver, sign_in, 1, 51, 912) else { unreachable!() };
    assert!(
        matches!(panel.step, forge::PanelStep::Queued { place: 2, .. }),
        "design domain/forge.md 9 and domain/agent.md 4.3: lower-numbered goal priority must go first before aging: {panel:?}"
    );
    driver.fire_at(skein_lib::Duration::from_secs(31));
    driver.settle();
    let engine::Delivery::Panel { panel, .. } = watch(&mut driver, sign_in, 1, 51, 913) else { unreachable!() };
    assert!(
        matches!(panel.step, forge::PanelStep::Queued { place: 1, .. }),
        "design domain/forge.md 9: a ready change past the aging window goes ahead of later ready changes: {panel:?}"
    );
}

// domain/forge.md, 8.8 and 9: leaving the queue marks the changes behind; reentry keeps readiness order.
#[test]
#[ignore = "needs: committed queue-exit and reentry hook for seeded changes; Change is create-only"]
fn queue_exit_and_reentry_refresh_the_panel_behind() {
    let (mut driver, sign_in) = driver();
    for task in 51..=53 {
        driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(task, 1, task) })));
    }
    driver.settle();
    watch(&mut driver, sign_in, 1, 53, 914);
    driver.send(engine::Event::Party(engine::PartyInput::Delivered { watcher: Token::new(914) }));
    driver.delivered.clear();
    let mut row = queued(52, 1, 52);
    row.change.state = temper_engine_domain_forge_change::State::Checking { head: [4; 32] };
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row })));
    driver.settle();
    let panel = driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            if let engine::Delivery::Panel { watcher, panel } = delivery
                && *watcher == Token::new(914)
            {
                Some(panel)
            } else {
                None
            }
        })
        .expect("design domain/forge.md 8.8: a queue exit must mark the change behind");
    assert!(
        matches!(panel.step, forge::PanelStep::Queued { place: 2, .. }),
        "queue exit must update the place: {panel:?}"
    );
    driver.send(engine::Event::Party(engine::PartyInput::Delivered { watcher: Token::new(914) }));
    driver.delivered.clear();
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(52, 1, 52) })));
    driver.settle();
    let panel = driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            if let engine::Delivery::Panel { watcher, panel } = delivery
                && *watcher == Token::new(914)
            {
                Some(panel)
            } else {
                None
            }
        })
        .expect("design domain/forge.md 8.8: reentry ahead must mark the change behind");
    assert!(
        matches!(panel.step, forge::PanelStep::Queued { place: 3, .. }),
        "design domain/forge.md 9: reentry keeps the first ready time and restores the previous place: {panel:?}"
    );
}

// domain/forge.md, 8.3 and 8.8: exact gates are not carried through even a clean update.
#[test]
fn clean_update_does_not_carry_an_exact_gate_verdict() {
    use temper_engine_domain_forge_change as change;
    let (mut driver, sign_in) = driver();
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: reviewed_row(change::Freshness::Exact) })));
    driver.settle();
    let engine::Delivery::Panel { panel, .. } = watch(&mut driver, sign_in, 1, 51, 918) else { unreachable!() };
    assert_eq!(
        panel.gates[0].verdict,
        change::Status::Unknown,
        "design domain/forge.md 8.3: an exact gate at an older head must not count at the cleanly updated head: {panel:?}"
    );
}

// domain/forge.md, 8.8: saving the change's own record marks its watched panel.
#[test]
#[ignore = "needs: committed mutation hook for an existing change record; Change is create-only"]
fn own_record_change_refreshes_a_watched_panel() {
    let (mut driver, sign_in) = driver();
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(51, 1, 51) })));
    driver.settle();
    watch(&mut driver, sign_in, 1, 51, 919);
    driver.send(engine::Event::Party(engine::PartyInput::Delivered { watcher: Token::new(919) }));
    driver.delivered.clear();
    let mut row = queued(51, 1, 51);
    row.change.repairs = 1;
    driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row })));
    driver.settle();
    let saved_repairs = driver.store.rows.values().find_map(|record| {
        if let engine::Record::Forge { row } = record
            && let forge::Stored::Change(row) = row.as_ref()
            && row.task == 51
        {
            Some(row.change.repairs)
        } else {
            None
        }
    });
    assert_eq!(saved_repairs, Some(1), "setup: the Change event must save the new record before testing its own mark");
    let panel = driver
        .delivered
        .iter()
        .find_map(|delivery| {
            if let engine::Delivery::Panel { watcher, panel } = delivery
                && *watcher == Token::new(919)
            {
                Some(panel)
            } else {
                None
            }
        })
        .expect("design domain/forge.md 8.8: saving the change's own record must mark its watched panel");
    assert_eq!(panel.repairs.used, 1, "the delivery must be the whole new panel");
}

// client/forge.md, 9.2: the watch's window is unused, and never slices a whole panel.
#[test]
fn a_watch_window_does_not_slice_the_queue_panel() {
    let (mut driver, sign_in) = driver();
    for task in 51..=55 {
        driver.send(engine::Event::Forge(Box::new(forge::Event::Change { row: queued(task, 1, task) })));
    }
    driver.settle();
    driver.send(engine::Event::Party(engine::PartyInput::Watch {
        watcher: Token::new(920),
        sign_in,
        subject: jig_domain_core::documents::Watched::Application {
            scope: jig_domain_people::ApplicationScope::Project(1),
            connector: 1,
            subject: Token::new(55),
        },
        window: jig_domain_core::documents::Window {
            rows: 1,
            after: Some(jig_domain_core::documents::Cursor::Number { number: u64::MAX }),
        },
    }));
    driver.settle();
    let panel = driver
        .delivered
        .iter()
        .find_map(|delivery| {
            if let engine::Delivery::Panel { watcher, panel } = delivery
                && *watcher == Token::new(920)
            {
                Some(panel)
            } else {
                None
            }
        })
        .expect("a whole panel with an unused window");
    let forge::PanelStep::Queued { place, ahead } = &panel.step else { panic!("queued panel: {panel:?}") };
    assert_eq!(*place, 5);
    assert_eq!(
        ahead.as_ref(),
        &[54, 53, 52, 51],
        "design client/forge.md 9.2: a window of one row after a cursor must still deliver the whole panel"
    );
}

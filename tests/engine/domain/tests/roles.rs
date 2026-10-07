//! Authenticated Owner/Policy role administration, atomic live reroute, keyed
//! races and specific independent-evidence negatives
//! (domain/people.md, section 5.1).

use jig_core_people as people;
use temper_engine_domain::{Key, Record, Write, engine};
use jig_core_tasks as tasks;
use temper_engine_domain_world::roles::{Base, Settings, World, limits, reroute_replayed};

fn successful() -> people::Reply {
    people::Reply::Outcome(people::Outcome::RolesSet { project: 1 })
}

fn refused(refusal: people::Refusal) -> people::Reply {
    people::Reply::Outcome(people::Outcome::Refused(refusal))
}

fn roster(world: &World, owner: usize) -> people::Ask {
    people::Ask::SetRoles {
        project: 1,
        holdings: Box::new([people::Holding { person: world.people[owner], role: people::Role::Owner }]),
    }
}

fn economic_rows(world: &World) -> Vec<(Key, Record)> {
    world
        .store
        .rows
        .iter()
        .filter(|(key, _)| {
            matches!(
                key,
                Key::Tasks(tasks::Key::Ledger(_)) | Key::RunProof { .. } | Key::Terminal { .. } | Key::Turn { .. }
            )
        })
        .map(|(key, row)| (*key, row.clone()))
        .collect()
}

#[test]
fn owner_policy_roster_and_requester_loss_commit_together_then_old_holder_has_no_standing() {
    let mut world = World::new(Settings::calm(9300, Base::Requester));
    let before = world.task_record().clone();
    let economic = economic_rows(&world);
    let ask = roster(&world, 1);
    world.ask(1, 70, ask.clone(), successful(), true);
    world.ask(1, 70, ask.clone(), successful(), true);
    world.run();
    assert_eq!(world.referee.replacements, 1, "two same-key calls share one actual route");
    assert!(matches!(
        world.task_record().escalation,
        tasks::Escalation::Waiting { revision: 2, holder: tasks::EscalationHolder::Role { project: 1, role: 0 }, .. }
    ));
    let mut expected = before;
    expected.escalation = world.task_record().escalation.clone();
    assert_eq!(*world.task_record(), expected, "authority, original funder, expense and tries stay exact");
    assert_eq!(economic_rows(&world), economic, "no accepted work or funding is rewritten");
    world.ask(1, 70, ask, successful(), false);
    world.run();
    assert_eq!(world.referee.replacements, 1, "durable key replay does not reroute again");
    let conflict = people::Ask::SetRoles { project: 1, holdings: Box::new([]) };
    world.ask(1, 70, conflict, people::Reply::Refused(people::Refusal::KeyConflict), false);
    world.ask(
        0,
        71,
        people::Ask::DecideEscalation {
            project: 1,
            task: world.task,
            revision: 2,
            decision: people::EscalationDecision::Release,
        },
        refused(people::Refusal::Standing),
        false,
    );
    world.run();
    assert_eq!(world.referee.replacements, 1);
    assert_eq!(economic_rows(&world), economic);
    assert!(
        !world.store.rows.values().any(|row| matches!(row, Record::EscalationDecision(_))),
        "rerouting is no accepted decision of the obsolete revision"
    );
    assert!(world.referee.done());
}

#[test]
fn unchanged_requester_final_role_and_rejected_state_emit_no_task_write() {
    for base in [Base::Requester, Base::FinalRole, Base::Rejected] {
        let mut world = World::new(Settings::calm(9301, base));
        let before = world.task_record().clone();
        let economic = economic_rows(&world);
        let owner = usize::from(base != Base::Requester);
        world.ask(owner, 72, roster(&world, owner), successful(), false);
        world.run();
        assert_eq!(*world.task_record(), before, "{base:?}: exact row remains inert");
        assert_eq!(economic_rows(&world), economic);
        assert_eq!(world.referee.replacements, 1);
    }
}

#[test]
fn both_current_owner_race_orders_authorize_only_the_first_committed_roster() {
    for winner in [0, 1] {
        let mut world = World::new(Settings { commit_delay: 1, ..Settings::calm(9302, Base::Requester) });
        let loser = 1 - winner;
        world.ask(winner, 73, roster(&world, winner), successful(), winner == 1);
        world.ask(loser, 74, roster(&world, loser), refused(people::Refusal::Role), false);
        world.run();
        assert_eq!(world.referee.replacements, 1);
        let Some(Record::People(people::Stored::Roles { holdings, .. })) =
            world.store.rows.get(&Key::People(people::Key::Roles(1)))
        else {
            panic!("actual durable role replacement");
        };
        assert_eq!(holdings.as_ref(), &[people::Holding { person: world.people[winner], role: people::Role::Owner }]);
    }
}

#[test]
fn role_admission_checks_current_authority_payload_bounds_and_revision_room() {
    let mut world = World::new(Settings::calm(9303, Base::Requester));
    let before = world.task_record().clone();
    let original = world.store.rows.get(&Key::People(people::Key::Roles(1))).cloned();
    let known = people::Holding { person: world.people[0], role: people::Role::Owner };
    for (key, holdings, reply) in [
        (
            75,
            Box::new([people::Holding { person: 0, role: people::Role::Owner }]) as Box<[_]>,
            refused(people::Refusal::Limit),
        ),
        (76, Box::new([known, known]), refused(people::Refusal::Limit)),
        (
            77,
            Box::new([people::Holding { person: u64::MAX, role: people::Role::Owner }]),
            refused(people::Refusal::Unknown),
        ),
        (78, Box::new([known, known, known]), people::Reply::Refused(people::Refusal::Limit)),
    ] {
        world.ask(1, key, people::Ask::SetRoles { project: 1, holdings }, reply, false);
        world.run();
    }
    world.ask(
        1,
        79,
        people::Ask::SetRoles { project: 0, holdings: Box::new([known]) },
        refused(people::Refusal::Unknown),
        false,
    );
    world.run();
    assert_eq!(*world.task_record(), before);
    assert_eq!(world.store.rows.get(&Key::People(people::Key::Roles(1))), original.as_ref());
    assert_eq!(world.referee.replacements, 0);
    current_owner_policy_and_session_expiry_are_separate_real_checks();
    exhausted_waiting_revision_and_insufficient_startup_room_refuse_before_mutation();
}

fn current_owner_policy_and_session_expiry_are_separate_real_checks() {
    let mut no_policy = World::new(Settings { policy: false, ..Settings::calm(9304, Base::Requester) });
    no_policy.ask(1, 80, roster(&no_policy, 1), refused(people::Refusal::Authority), false);
    no_policy.run();
    assert_eq!(no_policy.referee.replacements, 0);
    let mut expired = World::new(Settings { expired: true, ..Settings::calm(9304, Base::Requester) });
    expired.ask(1, 81, roster(&expired, 1), people::Reply::Refused(people::Refusal::SignIn), false);
    expired.run();
    assert_eq!(expired.referee.replacements, 0);
    let mut member = World::new(Settings::calm(9304, Base::Requester));
    let holdings = Box::new([
        people::Holding { person: member.people[0], role: people::Role::Owner },
        people::Holding { person: member.people[1], role: people::Role::Member },
    ]);
    member.ask(0, 82, people::Ask::SetRoles { project: 1, holdings }, successful(), false);
    member.run();
    member.ask(1, 83, roster(&member, 1), refused(people::Refusal::Role), false);
    member.run();
    assert_eq!(member.referee.replacements, 1, "ordinary membership does not grant administration");
}

fn exhausted_waiting_revision_and_insufficient_startup_room_refuse_before_mutation() {
    let mut exhausted = World::new(Settings { exhausted: true, ..Settings::calm(9305, Base::Requester) });
    let before = exhausted.task_record().clone();
    let roles = exhausted.store.rows.get(&Key::People(people::Key::Roles(1))).cloned();
    exhausted.ask(1, 84, roster(&exhausted, 1), refused(people::Refusal::Limit), false);
    exhausted.run();
    assert_eq!(*exhausted.task_record(), before);
    assert_eq!(exhausted.store.rows.get(&Key::People(people::Key::Roles(1))), roles.as_ref());
    assert_eq!(exhausted.referee.replacements, 0);
    let mut too_small = limits();
    too_small.journal.writes = 3;
    assert!(engine::worst_case(&too_small).is_none(), "counted root proof rejects missing atomic role-route room");
}

#[test]
fn role_commit_cut_recovers_exact_keyed_answer_and_same_holder_without_second_write() {
    let world = reroute_replayed(
        Settings { cut: true, commit_delay: 1, page_delay: 1, ..Settings::calm(9306, Base::Requester) },
        1,
    );
    assert_eq!(world.restarts, 1);
    assert!(world.pages > 8, "captured one-row startup pages run again after the durable cut");
    assert_eq!(world.referee.replacements, 1);
    assert!(world.referee.done());
}

#[test]
fn each_role_cohort_corruption_fails_its_specific_contract_after_positive_control() {
    let world = reroute_replayed(Settings::calm(9307, Base::Requester), 1);
    let (before, writes) = world
        .cohorts
        .iter()
        .find(|(_, writes)| {
            writes.iter().any(|write| matches!(write, Write::Save(Record::People(people::Stored::Roles { .. }))))
        })
        .expect("actual role replacement cohort and pre-transaction outside observer");
    assert_eq!(before.clone().commit(writes), Ok(()), "original exact cohort passes its unchanged observer");
    for (corruption, expected) in [
        (0, "roles and keyed answer are not atomic"),
        (1, "keyed role success lacks its roles row"),
        (2, "roles and changed waiting rows are not atomic"),
        (3, "reroute changed more than expected holder and revision"),
        (4, "role administration changed funding or accepted work"),
        (5, "roles differ from outside roster"),
        (6, "same key twice in role transaction"),
    ] {
        let mut altered = writes.clone();
        match corruption {
            0 => altered.retain(|write| !matches!(write, Write::Save(Record::People(people::Stored::Answer { .. })))),
            1 => altered.retain(|write| !matches!(write, Write::Save(Record::People(people::Stored::Roles { .. })))),
            2 => altered.retain(|write| !matches!(write, Write::Save(Record::Tasks(tasks::Stored::Live(_))))),
            3 => {
                for write in &mut altered {
                    if let Write::Save(Record::Tasks(tasks::Stored::Live(record))) = write {
                        record.numbers.spent += 1;
                    }
                }
            }
            4 => {
                let ledger = world
                    .initial_rows
                    .values()
                    .find(|row| matches!(row, Record::Tasks(tasks::Stored::Ledger(_))))
                    .expect("actual original funding source")
                    .clone();
                altered.push(Write::Save(ledger));
            }
            5 => {
                for write in &mut altered {
                    if let Write::Save(Record::People(people::Stored::Roles { holdings, .. })) = write {
                        holdings[0].person = world.people[0];
                    }
                }
            }
            6 => altered.push(altered[0].clone()),
            _ => unreachable!("seven specific outside-evidence corruptions"),
        }
        assert_eq!(before.clone().commit(&altered), Err(expected), "corruption {corruption}");
    }
}

#[test]
fn saturated_facts_change_no_role_request_trace_or_durable_rows() {
    let observed = reroute_replayed(Settings::calm(9308, Base::Requester), 1);
    let saturated = reroute_replayed(Settings { facts: false, ..Settings::calm(9308, Base::Requester) }, 1);
    assert_eq!(observed.trace, saturated.trace);
    assert_eq!(observed.store.rows, saturated.store.rows);
}

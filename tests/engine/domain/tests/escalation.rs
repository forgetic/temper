//! Held-chat requester/final-role stories and independent evidence negatives
//! (domain/engine.md, section 7.7; domain/tasks.md, section 15).

use skein_lib::Token;
use temper_engine_domain::{EscalationDecisionRecord, Key, Record, Write};
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;
use temper_engine_domain_world::escalation::{Cut, Settings, World, run_replayed};
use temper_engine_domain_world::escalation_referee::{REASON, Referee, Story};

fn settled(story: Story) -> World {
    let mut world = World::new(Settings::calm(9100, story));
    world.run();
    world
}

fn archives(world: &World) -> Vec<&EscalationDecisionRecord> {
    world
        .store
        .rows
        .values()
        .filter_map(|row| if let Record::EscalationDecision(archive) = row { Some(archive) } else { None })
        .collect()
}

#[test]
fn priced_failure_is_read_then_requester_release_starts_one_fresh_attempt() {
    let world = settled(Story::Release);
    assert!(world.referee.done());
    assert!(world.pages > 2, "real child startup and named historical read use the ordered store");
    let decisions = archives(&world);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].revision, 1);
    assert_eq!(decisions[0].by, decisions[0].requester);
    assert_eq!(decisions[0].decision, people::EscalationDecision::Release);
    world.referee.final_state(&world.store.rows).expect("lifetime cost 3 + new-attempt cost 2");
}

#[test]
fn requester_passes_to_final_role_second_owner_releases_and_final_pass_refuses() {
    let world = settled(Story::PassRelease);
    let decisions = archives(&world);
    assert_eq!(decisions.len(), 2, "NoFurther archives no semantic revision");
    assert_eq!(decisions[0].decision, people::EscalationDecision::Pass);
    assert_eq!(decisions[0].revision, 1);
    assert_eq!(decisions[1].decision, people::EscalationDecision::Release);
    assert_eq!(decisions[1].revision, 2);
    assert_ne!(decisions[1].by, decisions[1].requester, "second authenticated owner releases");
    assert!(world.store.rows.values().any(|row| matches!(
        row,
        Record::People(people::Stored::Answer { outcome: people::Outcome::Refused(people::Refusal::NoFurther), .. })
    )));
    assert!(world.referee.done());
}

#[test]
fn rejection_retains_a_bounded_reason_and_never_assigns_a_retry() {
    let world = settled(Story::Reject);
    assert_eq!(archives(&world).len(), 1);
    let rejected = world
        .store
        .rows
        .values()
        .find_map(|row| if let Record::Tasks(tasks::Stored::Live(record)) = row { Some(record.as_ref()) } else { None })
        .expect("rejected task stays live and held");
    assert_eq!(rejected.numbers.spent, 3);
    assert!(
        matches!(&rejected.escalation, tasks::Escalation::Rejected { revision: 1, reason, .. } if reason.as_ref() == REASON)
    );
    assert!(!world.store.rows.values().any(|row| matches!(row, Record::Tasks(tasks::Stored::Ended(_)))));
    assert!(world.referee.done(), "same-key replay, key conflict and stale-key outcome all settled");
}

#[test]
fn both_owner_races_return_the_same_first_committed_winner() {
    for story in [Story::RaceRelease, Story::RaceReject] {
        let world = settled(story);
        let decisions = archives(&world);
        assert_eq!(decisions.len(), 2);
        let final_decision = decisions[1];
        let terminals: Vec<_> = world
            .store
            .rows
            .values()
            .filter_map(|row| {
                if let Record::People(people::Stored::Answer {
                    outcome: people::Outcome::EscalationDecided { task, revision: 2, by, choice },
                    ..
                }) = row
                {
                    Some((*task, *by, *choice))
                } else {
                    None
                }
            })
            .collect();
        assert!(terminals.len() >= 3, "winner, different-choice loser and fresh stale-key call all replied");
        assert!(terminals.iter().all(|(task, by, _)| *task == final_decision.task && *by == final_decision.by));
        assert!(world.referee.done());
    }
}

#[test]
fn held_and_decision_commit_cuts_restore_paged_real_children_and_exact_scripts() {
    for cut in [Cut::Held, Cut::Decision] {
        let world =
            run_replayed(Settings { cut, commit_delay: 1, page_delay: 1, ..Settings::calm(9101, Story::PassRelease) });
        assert_eq!(world.restarts, 1);
        assert!(world.pages > 8, "one-row startup and named archive pages are actual requests");
        assert!(world.referee.done());
        world.referee.final_state(&world.store.rows).expect("no repeated charge or failed hold reopen");
    }
}

#[test]
fn saturated_facts_change_no_decision_or_durable_result() {
    for story in [Story::Release, Story::Reject] {
        let mut observed = World::new(Settings::calm(9102, story));
        observed.run();
        let mut saturated = World::new(Settings { facts: false, ..Settings::calm(9102, story) });
        saturated.run();
        assert_eq!(observed.trace, saturated.trace);
        assert_eq!(observed.store.rows, saturated.store.rows);
    }
}

fn before_decision(world: &World, archive: &EscalationDecisionRecord) -> Referee {
    let mut referee = Referee::new(Story::Release);
    for (index, user) in [7, 8].into_iter().enumerate() {
        let person = world
            .store
            .rows
            .values()
            .find_map(|row| {
                if let Record::People(people::Stored::Person { number, identity }) = row
                    && identity.key == (people::IdentityKey { forge: 1, user })
                {
                    Some(*number)
                } else {
                    None
                }
            })
            .expect("actual authenticated identity");
        let sign_in = world
            .store
            .rows
            .iter()
            .find_map(|(key, row)| {
                if let (
                    Key::People(people::Key::SignIn(sign_in)),
                    Record::People(people::Stored::SignIn { person: owner, .. }),
                ) = (key, row)
                    && *owner == person
                {
                    Some(*sign_in)
                } else {
                    None
                }
            })
            .expect("actual session");
        referee.signed_in(&world.store.rows, index, person, sign_in).expect("independent durable identity/session");
    }
    referee.started(&world.store.rows, archive.task).expect("original keyed chat evidence");
    referee.offer(archive.revision, archive.by, archive.decision.clone());
    referee
}

#[test]
fn referee_rejects_split_or_missing_decision_cohorts_and_duplicate_revision_effects() {
    let world = settled(Story::Release);
    let archive = archives(&world)[0];
    let writes = world
        .transactions
        .iter()
        .find(|writes| writes.iter().any(|write| matches!(write, Write::Save(Record::EscalationDecision(_)))))
        .expect("actual atomic decision cohort");
    for omission in 0..3 {
        let split: Vec<_> = writes
            .iter()
            .filter(|write| match omission {
                0 => !matches!(write, Write::Save(Record::EscalationDecision(_))),
                1 => !matches!(write, Write::Save(Record::Tasks(tasks::Stored::Live(_)))),
                2 => !matches!(write, Write::Save(Record::People(people::Stored::Answer { .. }))),
                _ => unreachable!("three cohort families"),
            })
            .cloned()
            .collect();
        assert!(before_decision(&world, archive).commit(&split).is_err(), "missing family {omission} must be rejected");
    }
    let mut duplicate = writes.clone();
    duplicate.push(writes[0].clone());
    assert_eq!(before_decision(&world, archive).commit(&duplicate), Err("same key twice in escalation transaction"));
    let mut referee = before_decision(&world, archive);
    referee.commit(writes).expect("one atomic decision");
    assert_eq!(referee.commit(writes), Err("semantic revision archived twice"));
    let mut altered = writes.clone();
    for write in &mut altered {
        if let Write::Save(Record::EscalationDecision(archive)) = write {
            archive.by += 1;
        }
    }
    assert_eq!(before_decision(&world, archive).commit(&altered), Err("archive differs from scripted winner"));
}

#[test]
fn referee_rejects_missing_terminal_evidence_duplicate_outputs_and_double_funding() {
    let world = settled(Story::Release);
    let archive = archives(&world)[0];
    let terminal = world
        .store
        .rows
        .values()
        .find_map(|row| if let Record::Terminal(terminal) = row { Some(terminal) } else { None })
        .expect("real first priced terminal");
    let mut rows = world.store.rows.clone();
    rows.remove(&Key::Terminal { task: terminal.task, attempt: terminal.attempt });
    assert_eq!(
        world.referee.clone().acknowledged(&rows, terminal.task, terminal.attempt),
        Err("worker ACK before exact durable terminal")
    );
    assert_eq!(
        world.referee.clone().acknowledged(&world.store.rows, terminal.task, terminal.attempt),
        Err("worker terminal ACK twice")
    );
    assert_eq!(
        world.referee.clone().result(&world.store.rows, archive.requester, archive.task, b"released report"),
        Err("unscripted or duplicate final report")
    );
    for row in rows.values_mut() {
        if let Record::Tasks(tasks::Stored::Ledger(pool)) = row
            && pool.numbers.spent_below == 5
        {
            pool.numbers.spent_below *= 2;
        }
    }
    assert_eq!(world.referee.final_state(&rows), Err("expense lost doubled or posted to wrong funding source"));
}

#[test]
fn key_conflict_requires_immediate_refusal_and_preserves_the_saved_winner() {
    let world = settled(Story::Release);
    let archive = archives(&world)[0];
    let to = Token::new(999);
    let key = people::RequestKey { person: archive.by, key: [10; 16] };
    let conflict = people::Ask::DecideEscalation {
        project: 1,
        task: archive.task,
        revision: archive.revision,
        decision: people::EscalationDecision::Reject { reason: REASON.into() },
    };
    let mut referee = before_decision(&world, archive);
    referee.key_conflict(&world.store.rows, to, key.person, key.key, conflict.clone());
    assert_eq!(
        referee.clone().replied(
            &world.store.rows,
            to,
            people::Reply::Outcome(people::Outcome::Refused(people::Refusal::KeyConflict)),
        ),
        Err("key conflict requires the immediate refusal wrapper")
    );
    for changed_ask in [false, true] {
        let mut rows = world.store.rows.clone();
        let Some(Record::People(people::Stored::Answer { ask, outcome, .. })) =
            rows.get_mut(&Key::People(people::Key::Answer(key)))
        else {
            panic!("original saved winner");
        };
        if changed_ask {
            *ask = conflict.clone();
        } else {
            *outcome = people::Outcome::Refused(people::Refusal::KeyConflict);
        }
        assert_eq!(
            referee.clone().replied(&rows, to, people::Reply::Refused(people::Refusal::KeyConflict)),
            Err("key conflict changed the original durable winner")
        );
    }
    referee
        .replied(&world.store.rows, to, people::Reply::Refused(people::Refusal::KeyConflict))
        .expect("direct refusal with the original durable winner unchanged");
}

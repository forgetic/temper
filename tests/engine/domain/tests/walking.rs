use temper_engine_domain::{Key, Record, Write};
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;
use temper_engine_domain_world::walking::{Settings, World, run_replayed};
use temper_engine_domain_world::walking_referee::{FINAL_SPEND, REPORT, WalkingReferee};

fn settled() -> World {
    let mut world = World::new(Settings::calm(65));
    world.run();
    world
}

fn ended(world: &World) -> &tasks::TaskRecord {
    world
        .store
        .rows
        .values()
        .find_map(|row| match row {
            Record::Tasks(tasks::Stored::Ended(record)) => Some(record.as_ref()),
            Record::Deployment(_)
            | Record::Turn(_)
            | Record::People(_)
            | Record::RunProof(_)
            | Record::EscalationDecision(_)
            | Record::Terminal(_)
            | Record::Tasks(tasks::Stored::Live(_) | tasks::Stored::Ledger(_)) => None,
        })
        .expect("story ended its one task")
}

#[test]
fn chat_claim_two_charged_turns_and_result_survive_a_lost_commit_completion() {
    let mut world = World::new(Settings::calm(61));
    world.run();
    assert!(world.referee.done());
    assert_eq!(world.restarts, 1);
    assert!(world.pages > 8, "tiny one-row pages restore the real child records");
    assert!(world.store.pending.is_empty());
}

#[test]
fn walking_story_replays_exactly_with_store_and_page_latency() {
    let settings = Settings { commit_delay: 2, page_delay: 1, ..Settings::calm(62) };
    let first = run_replayed(settings);
    let mut replay = World::new(settings);
    replay.run();
    assert_eq!(first.trace, replay.trace);
    assert_eq!(first.store.rows, replay.store.rows);
}

#[test]
fn saturated_observation_queues_change_no_walking_decision() {
    let mut observed = World::new(Settings::calm(63));
    observed.run();
    let mut saturated = World::new(Settings { facts: false, ..Settings::calm(63) });
    saturated.run();
    assert_eq!(observed.trace, saturated.trace);
    assert_eq!(observed.store.rows, saturated.store.rows);
}

#[test]
fn walking_story_also_settles_without_a_restart() {
    let mut world = World::new(Settings { restart: false, ..Settings::calm(64) });
    world.run();
    assert_eq!(world.restarts, 0);
    assert!(world.referee.done());
}

#[test]
fn walking_referee_rejects_duplicate_transaction_keys_and_transcripts() {
    let world = settled();
    let task = ended(&world);
    let row = world
        .store
        .rows
        .get(&Key::Turn { task: task.number, attempt: task.attempt, turn: 1 })
        .expect("first turn")
        .clone();
    let mut charged = task.clone();
    charged.turn = 1;
    charged.run_spent = 3;
    charged.numbers.spent = 3;
    let charged = Record::Tasks(tasks::Stored::Live(Box::new(charged)));
    let writes = [Write::Save(charged.clone()), Write::Save(row.clone()), Write::Save(row.clone())];
    assert_eq!(WalkingReferee::default().commit(&writes), Err("same key written twice in one decision"));
    let proof = Record::RunProof(temper_engine_domain::RunProof {
        task: task.number,
        attempt: task.attempt,
        turn: Some(temper_engine_domain::TurnProof { turn: 1, cumulative: 3, read: None }),
        terminal: None,
    });
    let writes = [Write::Save(charged), Write::Save(row), Write::Save(proof)];
    let mut referee = WalkingReferee::default();
    referee.commit(&writes).expect("first transcript");
    assert_eq!(referee.commit(&writes), Err("transcript committed twice"));
}

#[test]
fn walking_referee_rejects_split_turn_and_terminal_transactions() {
    let world = settled();
    let task = ended(&world);
    let turn = world
        .store
        .rows
        .get(&Key::Turn { task: task.number, attempt: task.attempt, turn: 1 })
        .expect("first transcript")
        .clone();
    assert_eq!(
        WalkingReferee::default().commit(&[Write::Save(turn)]),
        Err("transcript and accepted charge are not one transaction")
    );
    let terminal = world.store.rows.get(&Key::Tasks(tasks::Key::Ended(task.number))).expect("terminal").clone();
    assert_eq!(
        WalkingReferee::default().commit(&[Write::Save(terminal)]),
        Err("terminal and funding posting are not one transaction")
    );
}

#[test]
fn walking_referee_rejects_web_replies_without_their_atomic_records() {
    let world = settled();
    let task = ended(&world);
    let tasks::Party::Person(person) = task.requester else { panic!("person requester") };
    let sign_in = world.store.header().sign_ins;
    let mut rows = world.store.rows.clone();
    rows.remove(&Key::People(people::Key::Person(person)));
    assert_eq!(
        WalkingReferee::default().signed_in(&rows, person, sign_in),
        Err("sign-in reply before durable identity")
    );
    let mut referee = WalkingReferee::default();
    referee.signed_in(&world.store.rows, person, sign_in).expect("authenticated person");
    rows = world.store.rows.clone();
    rows.remove(&Key::People(people::Key::Answer(people::RequestKey { person, key: [5; 16] })));
    assert_eq!(referee.started(&rows, task.number), Err("chat reply before keyed answer commit"));
}

#[test]
fn walking_referee_rejects_duplicate_sign_in_and_chat_replies() {
    let world = settled();
    let task = ended(&world);
    let tasks::Party::Person(person) = task.requester else { panic!("person requester") };
    assert_eq!(
        world.referee.clone().signed_in(&world.store.rows, person, world.store.header().sign_ins),
        Err("person received sign-in reply twice")
    );
    assert_eq!(
        world.referee.clone().started(&world.store.rows, task.number),
        Err("person received chat-start reply twice")
    );
}

#[test]
fn walking_referee_rejects_wrong_task_or_uncommitted_or_duplicate_assignment() {
    use skein_lib::Duration;
    use temper_engine_domain::engine::Assignment;
    use temper_engine_domain_accounts::Grant;
    use temper_engine_domain_brief::{Body, Kind, Section};
    let world = settled();
    let task = ended(&world);
    let mut assignment = Assignment {
        task: task.number,
        attempt: task.attempt,
        charter: 1,
        sections: Box::new([Section {
            kind: Kind::Task,
            body: Body::Text(
                format!(
                    "say hello\n[Report: at most 128 bytes]\n[Requested by person {}]\n",
                    match task.requester {
                        tasks::Party::Person(person) => person,
                        tasks::Party::Task(_) | tasks::Party::Deployment { .. } => panic!("person requester"),
                    }
                )
                .into_bytes()
                .into_boxed_slice(),
            ),
        }]),
        grant: Grant { account: 1, generation: 1, valid: Duration::from_secs(60) },
    };
    assignment.task = task.number + 1;
    assert_eq!(
        world.referee.clone().assigned(&world.store.rows, &assignment),
        Err("assignment names another person's task")
    );
    assignment.task = task.number;
    assert_eq!(
        world.referee.clone().assigned(&world.store.rows, &assignment),
        Err("assignment without committed claim phase")
    );
    let mut rows = world.store.rows.clone();
    let Some(Record::Tasks(tasks::Stored::Ended(record))) = rows.get_mut(&Key::Tasks(tasks::Key::Ended(task.number)))
    else {
        panic!("task row")
    };
    record.phase = tasks::Phase::Active(tasks::Active::Claimed { attempt: task.attempt });
    assert_eq!(world.referee.clone().assigned(&rows, &assignment), Err("one claim assigned twice across restart"));
}

#[test]
fn walking_referee_rejects_turn_ack_before_transcript_and_charge() {
    let world = settled();
    let task = ended(&world);
    let mut rows = world.store.rows.clone();
    rows.remove(&Key::Turn { task: task.number, attempt: task.attempt, turn: 1 });
    assert_eq!(
        world.referee.clone().turn_ack(&rows, task.number, task.attempt, 1),
        Err("turn ACK before durable transcript")
    );
    rows = world.store.rows.clone();
    let Some(Record::Tasks(tasks::Stored::Ended(record))) = rows.get_mut(&Key::Tasks(tasks::Key::Ended(task.number)))
    else {
        panic!("ended task")
    };
    record.numbers.spent = 0;
    assert_eq!(
        world.referee.clone().turn_ack(&rows, task.number, task.attempt, 1),
        Err("turn ACK before atomic charge")
    );
}

#[test]
fn walking_referee_rejects_answer_ack_before_final_charge() {
    let world = settled();
    let task = ended(&world);
    let mut rows = world.store.rows.clone();
    let Some(Record::Tasks(tasks::Stored::Ended(record))) = rows.get_mut(&Key::Tasks(tasks::Key::Ended(task.number)))
    else {
        panic!("ended task")
    };
    record.numbers.spent = FINAL_SPEND - 1;
    assert_eq!(
        world.referee.clone().answer_ack(&rows, task.number, task.attempt),
        Err("answer ACK before exact terminal charge")
    );
}

#[test]
fn walking_referee_rejects_a_lost_or_duplicate_funding_posting_and_result() {
    let world = settled();
    let task = ended(&world);
    let tasks::Party::Person(person) = task.requester else { panic!("person requester") };
    for wrong in [0, FINAL_SPEND * 2] {
        let mut rows = world.store.rows.clone();
        let pool = tasks::Funder::Pool { project: 1, person, period: 1 };
        let Some(Record::Tasks(tasks::Stored::Ledger(record))) = rows.get_mut(&Key::Tasks(tasks::Key::Ledger(pool)))
        else {
            panic!("person pool")
        };
        record.numbers.spent_below = wrong;
        assert_eq!(
            world.referee.clone().result(&rows, person, task.number, REPORT),
            Err("final charge was lost, duplicated or posted to another source")
        );
    }
    assert_eq!(
        world.referee.clone().result(&world.store.rows, person, task.number, REPORT),
        Err("person received result twice")
    );
}

#[test]
fn walking_referee_rejects_missing_or_split_current_claim_turn_and_terminal_proofs() {
    let world = settled();
    let task = ended(&world);
    let mut live = task.clone();
    live.phase = tasks::Phase::Active(tasks::Active::Claimed { attempt: task.attempt });
    live.turn = 0;
    live.run_spent = 0;
    let initial = Record::RunProof(temper_engine_domain::RunProof {
        task: task.number,
        attempt: task.attempt,
        turn: None,
        terminal: None,
    });
    let claim = Write::Save(Record::Tasks(tasks::Stored::Live(Box::new(live.clone()))));
    assert_eq!(
        WalkingReferee::default().commit(std::slice::from_ref(&claim)),
        Err("claim and reserved root proof are not one transaction")
    );
    WalkingReferee::default().commit(&[claim, Write::Save(initial)]).expect("claim proof shares transaction");
    live.phase = tasks::Phase::Active(tasks::Active::Running { attempt: task.attempt });
    live.turn = 1;
    live.run_spent = 3;
    live.numbers.spent = 3;
    let turn =
        world.store.rows.get(&Key::Turn { task: task.number, attempt: task.attempt, turn: 1 }).expect("turn").clone();
    let charged = Write::Save(Record::Tasks(tasks::Stored::Live(Box::new(live))));
    assert_eq!(
        WalkingReferee::default().commit(&[charged, Write::Save(turn)]),
        Err("turn and root proof are not one transaction")
    );
    let ended = world.store.rows.get(&Key::Tasks(tasks::Key::Ended(task.number))).expect("ended").clone();
    let posted = world.store.rows.get(&Key::Tasks(tasks::Key::Ledger(task.funder))).expect("pool").clone();
    let terminal = world
        .store
        .rows
        .get(&Key::Terminal { task: task.number, attempt: task.attempt })
        .expect("typed terminal")
        .clone();
    assert_eq!(
        WalkingReferee::default().commit(&[
            Write::Save(ended.clone()),
            Write::Save(posted.clone()),
            Write::Save(terminal.clone())
        ]),
        Err("ended task and root terminal proof retirement are not one transaction")
    );
    assert_eq!(
        WalkingReferee::default().commit(&[
            Write::Save(ended.clone()),
            Write::Save(posted.clone()),
            Write::Erase(Key::RunProof { task: task.number })
        ]),
        Err("ended task and root terminal proof retirement are not one transaction")
    );
    WalkingReferee::default()
        .commit(&[
            Write::Save(ended),
            Write::Save(posted),
            Write::Save(terminal),
            Write::Erase(Key::RunProof { task: task.number }),
        ])
        .expect("terminal evidence and retirement share final transaction");
}

#[test]
fn final_transaction_survives_lost_completion_and_named_read_commits_its_position() {
    let mut recovered = World::new(Settings::terminal(81));
    recovered.run();
    assert!(recovered.referee.done() && recovered.referee.terminal_replay_done());
    assert_eq!(recovered.restarts, 1);
    assert!(recovered.pages > 8, "real child rows restore through one-row pages");
    assert!(recovered.store.pending.is_empty());
    let mut uninterrupted = World::new(Settings { restart: false, ..Settings::calm(81) });
    uninterrupted.run();
    let mut recovered_rows = recovered.store.rows.clone();
    let mut uninterrupted_rows = uninterrupted.store.rows.clone();
    recovered_rows.remove(&Key::Deployment);
    uninterrupted_rows.remove(&Key::Deployment);
    recovered_rows.remove(&Key::People(people::Key::ReadPosition(1)));
    assert_eq!(recovered_rows, uninterrupted_rows, "same task, transcript, terminal and financial history");
    assert!(matches!(
        recovered.store.rows.get(&Key::People(people::Key::ReadPosition(1))),
        Some(Record::People(people::Stored::ReadPosition { position: 1, .. }))
    ));
    assert_eq!(recovered.store.applied, uninterrupted.store.applied + 1, "the only extra commit marks the read");
}

#[test]
fn terminal_restart_replays_complete_state_with_delayed_store_and_page_terminals() {
    let settings = Settings { commit_delay: 2, page_delay: 2, ..Settings::terminal(82) };
    let first = run_replayed(settings);
    let mut replay = World::new(settings);
    replay.run();
    assert!(first.referee.terminal_replay_done());
    assert_eq!(first.trace, replay.trace);
    assert_eq!(first.store.rows, replay.store.rows);
}

#[test]
fn terminal_restart_is_unchanged_when_observation_queues_saturate() {
    let mut observed = World::new(Settings::terminal(83));
    observed.run();
    let mut saturated = World::new(Settings { facts: false, ..Settings::terminal(83) });
    saturated.run();
    assert_eq!(observed.trace, saturated.trace);
    assert_eq!(observed.store.rows, saturated.store.rows);
    assert!(saturated.referee.done() && saturated.referee.terminal_replay_done());
}

fn terminal_cut() -> World {
    let mut world = World::new(Settings::terminal(84));
    for _ in 0..600 {
        if world.restarts == 1 {
            world.referee.terminal_cut(&world.store.rows).expect("the selected cut precedes every outward terminal");
            return world;
        }
        world.iterate();
    }
    panic!("script never reached its durable terminal cut");
}

#[test]
fn independent_terminal_cut_referee_rejects_missing_or_altered_evidence() {
    let world = terminal_cut();
    let task = ended(&world);
    let key = Key::Terminal { task: task.number, attempt: task.attempt };
    let mut rows = world.store.rows.clone();
    rows.remove(&key);
    assert_eq!(world.referee.terminal_cut(&rows), Err("durable terminal evidence missing"));
    rows = world.store.rows.clone();
    let Some(Record::Terminal(terminal)) = rows.get_mut(&key) else { panic!("terminal evidence") };
    terminal.cumulative += 1;
    assert_eq!(world.referee.terminal_cut(&rows), Err("durable terminal evidence differs from worker offer"));
    rows = world.store.rows.clone();
    rows.insert(
        Key::RunProof { task: task.number },
        Record::RunProof(temper_engine_domain::RunProof {
            task: task.number,
            attempt: task.attempt,
            turn: None,
            terminal: None,
        }),
    );
    assert_eq!(world.referee.terminal_cut(&rows), Err("ended task retained a live task or proof"));
    rows = world.store.rows.clone();
    let Some(Record::Tasks(tasks::Stored::Ledger(pool))) = rows.get_mut(&Key::Tasks(tasks::Key::Ledger(task.funder)))
    else {
        panic!("person pool")
    };
    pool.numbers.spent_below *= 2;
    assert_eq!(world.referee.terminal_cut(&rows), Err("final charge was lost, duplicated or posted to another source"));
}

#[test]
fn independent_referee_rejects_recommitted_terminal_and_unscripted_ack() {
    let world = terminal_cut();
    let task = ended(&world);
    let writes = [
        Write::Save(world.store.rows[&Key::Tasks(tasks::Key::Ended(task.number))].clone()),
        Write::Save(world.store.rows[&Key::Tasks(tasks::Key::Ledger(task.funder))].clone()),
        Write::Save(world.store.rows[&Key::Terminal { task: task.number, attempt: task.attempt }].clone()),
        Write::Erase(Key::RunProof { task: task.number }),
    ];
    let mut referee = WalkingReferee::default();
    referee.commit(&writes).expect("one actual final transaction");
    assert_eq!(referee.commit(&writes), Err("terminal committed twice"));
    let mut referee = world.referee.clone();
    assert_eq!(
        referee.replayed_answer_ack(&world.store.rows, task.number, task.attempt),
        Err("replay ACK is not the one explicit post-ACK resend")
    );
    referee.answer_ack(&world.store.rows, task.number, task.attempt).expect("one recovered answer ACK");
    assert_eq!(
        referee.terminal_cut(&world.store.rows),
        Err("cut is not after one terminal commit before ACK/result release")
    );
    referee
        .replayed_answer_ack(&world.store.rows, task.number, task.attempt)
        .expect("the explicit worker resend is ACKed");
    assert_eq!(
        referee.replayed_answer_ack(&world.store.rows, task.number, task.attempt),
        Err("replay ACK is not the one explicit post-ACK resend")
    );
}

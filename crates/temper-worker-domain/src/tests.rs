//! Worker-root stories at Smith's typed process boundary (jig's domain/hosts.md, 4 and 6.4).
use crate::{Domain, Event, Limits, Request, agent, checkout, host, max_out, step, wire, worst_case};
use alloc::boxed::Box;
use skein_lib::{Duration, Env, List, Queue, Time, Token, Wall};
const LIMITS: Limits = Limits {
    host: host::Limits {
        accounts: 4,
        slots: 3,
        charter_bytes: 8_192,
        snapshot_bytes: 0,
        transcript_bytes: 0,
        delivery_evidence_bytes: 0,
        turn_bytes: 64,
        outcome_bytes: 4_096,
        detail_bytes: 32,
        held: 2,
        event_bytes: 96,
        run_calls: 2,
        facts: 256,
        told: 16,
        fact_bytes: 32,
        turns: 1,
        turn_queue_bytes: 64,
    },
    checkout: checkout::Limits {
        workspaces: 4,
        repositories: 2,
        name_bytes: 32,
        message_bytes: 512,
        conflicts: 0,
        path_bytes: 0,
        remote_timeout: Duration::from_secs(60),
        local_timeout: Duration::from_secs(10),
        facts: 256,
    },
    agent: agent::Limits {
        accounts: 4,
        directories: 8,
        name_bytes: 256,
        agents: 3,
        charter_bytes: 8_192,
        transcript_bytes: 0,
        answered_bytes: 4096,
        turn_bytes: 64,
        turns: 1,
        unacknowledged_bytes: 64,
        conflicts: 0,
        path_bytes: 0,
        message_bytes: 96,
        messages: 2,
        calls: 2,
        call_bytes: 512,
        answer_bytes: 1_048_576,
        fact_bytes: 32,
        outcome_bytes: 4_096,
        detail_bytes: 32,
        spawn_timeout: Duration::from_secs(1),
        no_progress: Duration::from_secs(10),
        long_span: Duration::from_secs(120),
        wall_time: Duration::from_secs(900),
        grace: Duration::from_secs(5),
        kill_after: Duration::from_secs(2),
        facts: 256,
    },
    grace: Duration::from_secs(60),
    redial: Duration::from_secs(1),
    redial_max: Duration::from_secs(8),
    stalled: 8,
    turn_backoff: Duration::from_secs(1),
};

const RUN: Token = Token::new(31);
const ATTEMPT: Token = Token::new(932);

struct Harness {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
}
impl Harness {
    fn new() -> Self {
        assert!(worst_case(&LIMITS).is_some());
        let mut h = Self {
            domain: Domain::new(&LIMITS, 7),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: LIMITS },
            out: Queue::with_capacity(max_out(&LIMITS)),
        };
        crate::fire(&mut h.domain, &h.env, &mut h.out);
        while h.out.pop().is_some() {}
        h.step(Event::ConnectedV2);
        h
    }
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.domain, &self.env, event, &mut self.out);
        let mut rows = List::with_capacity(max_out(&LIMITS));
        while let Some(row) = self.out.pop() {
            rows.push(row).expect("reserved outputs");
        }
        self.domain.reclaim();
        rows.into_boxed()
    }
    fn start(&mut self) -> Token {
        let rows = self.step(Event::AssignTyped {
            assignment: wire::AssignmentTyped {
                assignment: wire::Assignment {
                    run: RUN,
                    attempt: ATTEMPT,
                    workspace: wire::Workspace { key: Box::new([]), repositories: Box::new([]) },
                    save: None,
                    charter: Box::from(&b"charter"[..]),
                    snapshot: None,
                    grants: Box::new([]),
                },
                turns: Box::new([]),
                answered: Box::new([]),
            },
        });
        let mut spawned = None;
        for row in &rows {
            if let Request::Spawn { owner, .. } = row {
                spawned = Some(*owner);
            }
        }
        let owner = spawned.expect("Smith spawn");
        self.step(Event::Spawned { owner, process: Token::new(777) });
        self.step(Event::Sent { owner });
        self.step(Event::Received { owner, message: agent::Up::Admitted });
        owner
    }
    fn turn(&mut self, owner: Token, number: u32) -> Box<[Request]> {
        self.step(Event::Received {
            owner,
            message: agent::Up::Turn {
                turn: agent::Turn { number, spent: u64::from(number), read: None, body: Box::from([7; 64]) },
            },
        })
    }
}

#[test]
fn the_agent_is_gone_while_the_engine_cannot_acknowledge_turns() {
    let mut h = Harness::new();
    let owner = h.start();
    h.step(Event::Lost);
    let rows = h.turn(owner, 1);
    assert!(acknowledges(&rows, 1));
    h.step(Event::Sent { owner });
    assert_eq!(h.domain.retained_turns(), 1);
    // The hub is full. Smith sends one more turn under its independent window.
    let rows = h.turn(owner, 2);
    assert!(!acknowledges(&rows, 2));
    h.step(Event::Shutdown);
    crate::resume(&mut h.domain, &h.env, &mut h.out);
    while h.out.pop().is_some() {}
    // Cancel precedes the staged turn's acknowledgement on Smith's pipe.
    h.step(Event::Sent { owner });
    h.step(Event::Sent { owner });
    h.step(Event::Exited { owner });
    h.step(Event::Hangup { owner });
    h.step(Event::Reaped { owner, detail: Box::new([]) });
    assert_eq!(h.domain.agent().agents(), 0, "Gone does not need an engine ACK");
    assert_eq!(h.domain.host().unanswered(), 0);
    assert_eq!(h.domain.retained_turns(), 2, "the hub owns both turns until committed");
    h.env.now = h.domain.next_deadline().expect("redial");
    crate::fire(&mut h.domain, &h.env, &mut h.out);
    while h.out.pop().is_some() {}
    let rows = h.step(Event::ConnectedV2);
    let mut turns = 0;
    let mut answered = false;
    for row in &rows {
        if let Request::Turn { turn, .. } = row {
            assert!(!answered, "turns precede the answer");
            turns += 1;
            assert_eq!(turn.turn, turns);
        }
        if let Request::AnswerV2 { .. } = row {
            answered = true;
        }
    }
    assert_eq!(turns, 2);
    assert!(answered, "answer waits for the link");
    h.step(Event::Acknowledged { run: RUN, attempt: ATTEMPT });
    assert_eq!(h.domain.held(), 1);
    h.step(Event::AcknowledgeTurn { run: RUN, attempt: ATTEMPT, turn: 1 });
    h.step(Event::AcknowledgeTurn { run: RUN, attempt: ATTEMPT, turn: 2 });
    assert_eq!(h.domain.held(), 0);
}

#[test]
fn engine_credit_admits_a_staged_turn_and_acknowledges_smith_once() {
    let mut h = Harness::new();
    let owner = h.start();
    h.turn(owner, 1);
    h.step(Event::Sent { owner });
    assert!(h.turn(owner, 2).is_empty());
    let rows = h.step(Event::AcknowledgeTurn { run: RUN, attempt: ATTEMPT, turn: 1 });
    let mut transmitted = false;
    for row in &rows {
        if let Request::Turn { turn, .. } = row {
            transmitted = turn.turn == 2;
        }
    }
    assert!(transmitted);
    assert!(acknowledges(&rows, 2));
    assert!(h.step(Event::AcknowledgeTurn { run: RUN, attempt: ATTEMPT, turn: 1 }).is_empty());
}

#[test]
fn smith_call_names_preserve_activation_completion_and_position() {
    let name = agent::CallName { activation: 0x1020_3040_5060_7080, completion: 0x90a0_b0c0, position: 0xd0e0_f000 };
    let bytes = crate::translate::call_name(name);
    assert_eq!(bytes, [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0, 0xf0, 0]);
    assert_eq!(crate::translate::named(&bytes), Some(name));
    assert!(crate::translate::named(&bytes[..15]).is_none());
}

#[test]
fn host_and_smith_turn_windows_must_fit_the_declared_reserve() {
    assert!(worst_case(&Limits { agent: agent::Limits { turns: 2, ..LIMITS.agent }, ..LIMITS }).is_none());
    assert!(
        worst_case(&Limits { agent: agent::Limits { unacknowledged_bytes: 65, ..LIMITS.agent }, ..LIMITS }).is_none()
    );
}

#[test]
fn malformed_resumed_call_names_are_refused_before_starting_smith() {
    let mut h = Harness::new();
    let rows = h.step(Event::AssignTyped {
        assignment: wire::AssignmentTyped {
            assignment: wire::Assignment {
                run: RUN,
                attempt: ATTEMPT,
                workspace: wire::Workspace { key: Box::new([]), repositories: Box::new([]) },
                save: None,
                charter: Box::from(&b"charter"[..]),
                snapshot: None,
                grants: Box::new([]),
            },
            turns: Box::new([]),
            answered: Box::from([host::AnsweredCall {
                name: Box::from(&b"short"[..]),
                tool: Box::from(&b"read"[..]),
                answer: host::SettledAnswer::Host { error: false, body: Box::new([]) },
            }]),
        },
    });
    let [
        Request::AnswerV2 {
            answer:
                wire::AnswerV2 {
                    ending: wire::EndingV2::Refused(wire::Refusal::Invalid(wire::Invalid::Transcript)), ..
                },
            ..
        },
    ] = &*rows
    else {
        panic!("malformed call name must be refused: {rows:?}");
    };
}

fn acknowledges(rows: &[Request], expected: u32) -> bool {
    for row in rows {
        if let Request::Send { message: agent::Down::Acknowledge { turn }, .. } = row
            && *turn == expected
        {
            return true;
        }
    }
    false
}

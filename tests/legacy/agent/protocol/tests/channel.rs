use skein_lib::{Duration, ReplyTo, Time, Token, stream};
use temper_channel::{payload::v1 as document, wire};
use temper_legacy_agent_domain::{self as agent, llm, run};
use temper_legacy_agent_protocol::{
    Error,
    channel::{self, State, Up},
    payload, worker,
};
use temper_legacy_agent_protocol_world::pipe::{self, World};

fn admitted(world: &mut World) {
    world.down(agent::Request::Admitted { worker: Token::new(5), run: Token::new(9) });
}
fn running() -> World {
    let mut world = World::new();
    world.peer(pipe::start());
    world.settle();
    world.roots();
    admitted(&mut world);
    world.settle();
    world
}
fn answer(spent: run::Spend) -> agent::Request {
    agent::Request::Answer {
        to: ReplyTo::new(Token::new(5)),
        answer: run::Answer::Accepted {
            outcome: run::outcome::Declared::Verdict(run::outcome::Verdict {
                name: payload::REPORT.into(),
                body: b"done".as_slice().into(),
                children: Box::new([]),
            }),
            spent,
        },
    }
}
#[test]
fn start_defers_roots_and_ages_grants_while_rearming_grant_and_cancel_reads() {
    let mut world = World::new();
    assert_eq!(world.channel.state(), State::Starting);
    world.peer(pipe::start());
    world.settle();
    assert_eq!(world.channel.state(), State::Rooting);
    world.now = Time::from_nanos(2_000_000_000);
    world.peer(wire::Message::AgentGrant { grant: pipe::grant(0, 2) });
    world.peer(wire::Message::AgentCancel);
    world.settle();
    world.now = Time::from_nanos(5_000_000_000);
    world.roots();
    admitted(&mut world);
    let starts: Vec<_> = world
        .events
        .iter()
        .filter_map(|up| {
            if let Up::Domain(agent::Event::Start { grants, charter, .. }) = up {
                Some((grants, charter))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].0[0].name.generation, 2);
    assert_eq!(starts[0].0[0].valid, Duration::from_secs(7));
    assert_eq!(starts[0].1.checkout.repositories[0].root, Token::new(7));
    assert_eq!(
        world
            .events
            .iter()
            .filter(|up| matches!(up, Up::Domain(agent::Event::Cancel { run }) if run.raw() == 9))
            .count(),
        1
    );
}
#[test]
fn matching_host_terminal_is_delivered_once_and_stale_answers_are_fenced() {
    let mut world = running();
    world.down(agent::Request::Push {
        worker: Token::new(5),
        owner: Token::new(30),
        change: run::outcome::Change { title: b"fix".as_slice().into(), body: Box::new([]) },
    });
    world.settle();
    world.peer(wire::Message::AgentAnswer { call: 31, reply: wire::Reply::Pushed { push: wire::Push::Done } });
    world.peer(wire::Message::AgentAnswer { call: 30, reply: wire::Reply::Pushed { push: wire::Push::Done } });
    world.peer(wire::Message::AgentAnswer { call: 30, reply: wire::Reply::Pushed { push: wire::Push::Moved } });
    world.settle();
    assert_eq!(world.events.iter().filter(|event| matches!(event, Up::Domain(agent::Event::Pushed { owner, push: run::Push::Done }) if owner.raw() == 30)).count(), 1);
    assert!(
        !world
            .events
            .iter()
            .any(|event| matches!(event, Up::Domain(agent::Event::Pushed { push: run::Push::Moved, .. })))
    );
}
#[test]
fn a_tiny_fact_flood_preserves_terminal_room_and_closed_binding_is_explicit() {
    let mut world = running();
    world.down(agent::Request::Checking { worker: Token::new(5), deadline: Time::from_nanos(10_000_000_000) });
    world.settle();
    world.blocked = true;
    let text = agent::Content::Text { owner: Token::new(22), text: b"x".as_slice().into() };
    for _ in 0..world.configured.sizes.facts {
        assert!(world.channel.can_fact(&world.configured));
        assert!(world.content(&text));
        world.settle();
    }
    assert!(!world.channel.can_fact(&world.configured));
    assert!(!world.content(&text));
    let finished = agent::Fact::Run {
        fact: run::facts::Fact::CheckFinished { run: Token::new(9), exit: run::Exit::Code { code: 0 } },
    };
    assert!(world.channel.can_take_fact(&finished, &world.configured));
    let env = world.env();
    assert!(channel::fact(&mut world.channel, &env, finished).expect("owed LongDone"));
    assert!(channel::begin_check(&mut world.channel, Token::new(700)).expect("actual check binding"));
    assert!(channel::end_check(&mut world.channel, Token::new(700)));
    world.settle();
    assert!(world.messages.iter().any(|message| matches!(message, wire::Message::LongDone)));
    let spent = run::Spend { turns: 1, input: 2, output: 3, cache_read: 4, cache_write: 5 };
    world.down(answer(spent));
    world.settle();
    assert!(world.messages.iter().any(|m| matches!(m, wire::Message::Finish { finish: wire::Finish::Ended { .. } })));
    assert!(world.output_ended);
    assert_eq!(world.channel.spent(), Some(spent));
    assert_eq!(world.channel.state(), State::Finishing);
    assert!(!world.content(&text));
    world.closed();
    assert_eq!(world.channel.state(), State::Closed);
    assert_eq!(world.events.iter().filter(|event| matches!(event, Up::Closed)).count(), 1);
    world.closed();
    assert_eq!(world.events.iter().filter(|event| matches!(event, Up::Closed)).count(), 1);
}
#[test]
fn fault_crossings_drain_admission_host_terminal_spend_and_below_cleanup() {
    let mut world = World::new();
    world.peer(pipe::start());
    world.settle();
    world.roots();
    world.event(stream::Up::Failed(stream::Fault::Reset));
    assert_eq!(world.channel.state(), State::Failed);
    let before = world.messages.len();
    admitted(&mut world);
    world.down(agent::Request::Push {
        worker: Token::new(5),
        owner: Token::new(40),
        change: run::outcome::Change { title: b"late".as_slice().into(), body: Box::new([]) },
    });
    world.down(agent::Request::CancelHost { owner: Token::new(40) });
    world.down(agent::Request::Checking { worker: Token::new(5), deadline: Time::ZERO });
    world.down(agent::Request::Rejected { grant: agent::GrantName { account: 0, generation: 1 } });
    world.down(agent::Request::Exhausted { account: 0, retry_after: Duration::ZERO });
    world.down(agent::Request::Cancel { owner: Token::new(41) });
    world.down(agent::Request::CancelIo { owner: Token::new(42) });
    world.down(agent::Request::Abort { owner: Token::new(43) });
    world.down(answer(run::Spend::ZERO));
    assert_eq!(world.messages.len(), before);
    assert_eq!(
        world.events.iter().filter(|e| matches!(e, Up::Domain(agent::Event::Cancel { run }) if run.raw() == 9)).count(),
        1
    );
    assert_eq!(
        world
            .events
            .iter()
            .filter(|e| matches!(e, Up::Domain(agent::Event::Pushed { owner, .. }) if owner.raw() == 40))
            .count(),
        1
    );
    assert_eq!(
        world
            .events
            .iter()
            .filter(|e| matches!(
                e,
                Up::Below(
                    agent::Request::Cancel { .. } | agent::Request::CancelIo { .. } | agent::Request::Abort { .. }
                )
            ))
            .count(),
        3
    );
    world.closed();
    world.down(agent::Request::Cancel { owner: Token::new(44) });
    assert!(world.events.iter().any(|e| matches!(e, Up::Below(agent::Request::Cancel { owner }) if owner.raw() == 44)));
}
#[test]
fn stdin_eof_cancels_once_but_the_terminal_can_still_be_written() {
    let mut world = running();
    world.eof();
    assert_eq!(world.channel.state(), State::Running);
    world.down(answer(run::Spend::ZERO));
    world.settle();
    assert!(world.output_ended);
    assert_eq!(world.events.iter().filter(|e| matches!(e, Up::Domain(agent::Event::Cancel { .. }))).count(), 1);
}
#[test]
fn running_agent_has_no_ping_silence_or_stall_policy_and_expiry_retires_values() {
    let mut waiting = World::new();
    waiting.now = Time::from_nanos(100_000_000_000);
    waiting.fire();
    assert_eq!(waiting.channel.state(), State::Starting);
    assert_eq!(waiting.channel.next_deadline(&waiting.configured), None);
    let mut world = running();
    world.now = Time::from_nanos(100_000_000_000);
    world.fire();
    assert_eq!(world.channel.state(), State::Running);
    assert_eq!(world.channel.next_deadline(&world.configured), None);
    assert!(
        world
            .channel
            .credentials()
            .get(agent::GrantName { account: 0, generation: 1 }, world.now, Duration::ZERO)
            .is_none()
    );
    assert!(!world.messages.iter().any(|m| matches!(m, wire::Message::Ping)));
    world.peer(wire::Message::AgentGrant { grant: pipe::grant(0, 1) });
    world.settle();
    assert!(
        world
            .channel
            .credentials()
            .get(agent::GrantName { account: 0, generation: 1 }, world.now, Duration::ZERO)
            .is_none()
    );
    let configured = pipe::limits();
    assert!(channel::Channel::new(Token::new(5), Time::ZERO, &configured, Duration::from_secs(1)).is_none());
}
#[test]
fn malformed_entrances_fail_typed_policy_without_start_or_secret_leak() {
    for mode in 0..7 {
        let mut world = World::new();
        let mut start = pipe::start();
        let wire::Message::AgentStart { charter, snapshot, endpoints, grants, .. } = &mut start else {
            panic!("start fixture");
        };
        let expected = match mode {
            0 => {
                *snapshot = Some(b"\xffunknown snapshot".as_slice().into());
                Error::Unsupported
            }
            1 => {
                *charter = b"broken".as_slice().into();
                Error::Malformed
            }
            2 => {
                *endpoints = Box::new([endpoints[0].clone(), endpoints[0].clone()]);
                Error::Endpoint
            }
            3 => {
                endpoints[0].host = b"bad\r\n".as_slice().into();
                Error::Endpoint
            }
            4 => {
                endpoints[0].path = b"/bad\0".as_slice().into();
                Error::Endpoint
            }
            5 => {
                endpoints[0].endpoint = 8;
                Error::Endpoint
            }
            6 => {
                grants[0].account = 1;
                Error::Grant
            }
            _ => unreachable!(),
        };
        world.peer(start);
        world.settle();
        assert!(world.events.iter().any(|up| matches!(up, Up::Refused(error) if *error == expected)), "mode {mode}");
        assert!(!world.events.iter().any(|up| matches!(up, Up::Domain(agent::Event::Start { .. }))));
        assert!(world.messages.iter().any(|m| matches!(
            m,
            wire::Message::Finish { finish: wire::Finish::Failed { failure: wire::RunFailure::Policy } }
        )));
    }
}
#[test]
fn refusal_is_unrepresentable_and_content_keeps_each_conversation_owner() {
    assert!(matches!(
        worker::finish(run::Answer::Refused(run::Refusal::Busy), &pipe::limits().sizes),
        Err(Error::Unsupported)
    ));
    for (value, kind, prefix) in [
        (
            agent::Content::Text { owner: Token::new(1), text: b"a\xff".as_slice().into() },
            document::FactKind::Text,
            b"conversation 1\na\\xFF".as_slice(),
        ),
        (
            agent::Content::Usage { owner: Token::new(2), usage: llm::Usage::ZERO },
            document::FactKind::Usage,
            b"conversation 2\ninput".as_slice(),
        ),
    ] {
        let wire::Message::Fact { fact } = worker::content(&value, &pipe::limits().sizes).expect("projection") else {
            panic!("fact");
        };
        let fact = document::decode_fact(&fact, &pipe::limits().sizes).expect("payload");
        assert_eq!(fact.kind, kind);
        assert!(fact.content.starts_with(prefix));
        assert!(std::str::from_utf8(&fact.content).is_ok());
    }
}
#[test]
fn spent_survives_normal_and_closed_outcome_encoding_failure() {
    for close in [false, true] {
        let mut world = running();
        if close {
            world.event(stream::Up::Failed(stream::Fault::Reset));
        }
        let spent = run::Spend { turns: 1, input: 5, output: 8, cache_read: 0, cache_write: 0 };
        let request = agent::Request::Answer {
            to: ReplyTo::new(Token::new(5)),
            answer: run::Answer::Accepted {
                outcome: run::outcome::Declared::Change(run::outcome::Change {
                    title: vec![b'x'; world.configured.sizes.outcome as usize + 1].into(),
                    body: Box::new([]),
                }),
                spent,
            },
        };
        let env = world.env();
        let mut out = skein_lib::Queue::with_capacity(channel::MAX_UP);
        assert_eq!(
            channel::down(&mut world.channel, &env, request, &mut out),
            if close { Ok(()) } else { Err(Error::TooLarge) }
        );
        assert_eq!(world.channel.spent(), Some(spent));
    }
}
#[test]
fn actual_check_settlement_precedes_finish_even_when_check_finished_fact_drops() {
    let mut world = running();
    // An actual owner can settle before its delayed Checking reaches the pipe.
    assert!(channel::begin_check(&mut world.channel, Token::new(701)).expect("actual check"));
    assert!(channel::end_check(&mut world.channel, Token::new(701)));
    let long = agent::Request::Checking { worker: Token::new(5), deadline: Time::from_nanos(20_000_000_000) };
    assert!(world.channel.can_take(&long));
    world.down(long);
    world.settle();
    // Duplicate observation of that same fenced terminal adds no second notice.
    assert!(!channel::end_check(&mut world.channel, Token::new(701)));
    world.settle();
    assert_eq!(world.messages.iter().filter(|m| matches!(m, wire::Message::LongDone)).count(), 1);
    let long_at = world.messages.iter().position(|m| matches!(m, wire::Message::Long { .. })).expect("Long");
    let done_at = world.messages.iter().position(|m| matches!(m, wire::Message::LongDone)).expect("LongDone");
    assert!(long_at < done_at);
    world.down(answer(run::Spend::ZERO));
    world.settle();
    assert!(world.output_ended);
}
#[test]
fn consecutive_checks_reuse_domain_owner_but_keep_each_notice_cycle_and_ignore_stale_facts() {
    let mut world = running();
    let check = || agent::Request::Check {
        owner: Token::new(70),
        program: run::Place { root: Token::new(7), path: b".temper/pre-pr".as_slice().into() },
        deadline: Time::from_nanos(30_000_000_000),
        tail: 128,
    };
    let long = || agent::Request::Checking { worker: Token::new(5), deadline: Time::from_nanos(30_000_000_000) };
    world.down(long());
    world.down(check());
    assert!(channel::begin_check(&mut world.channel, Token::new(702)).expect("first actual invocation"));
    assert!(channel::end_check(&mut world.channel, Token::new(702)));
    world.settle();
    // Second lower invocation is fresh despite the same domain Call owner.
    world.down(check());
    assert!(channel::begin_check(&mut world.channel, Token::new(703)).expect("second actual invocation"));
    assert!(!channel::end_check(&mut world.channel, Token::new(702)));
    let stale = agent::Fact::Run {
        fact: run::facts::Fact::CheckFinished { run: Token::new(9), exit: run::Exit::Code { code: 0 } },
    };
    let env = world.env();
    assert!(channel::fact(&mut world.channel, &env, stale).expect("old shape consumed"));
    world.down(long());
    world.settle();
    assert_eq!(world.messages.iter().filter(|m| matches!(m, wire::Message::LongDone)).count(), 1);
    assert!(channel::end_check(&mut world.channel, Token::new(703)));
    world.settle();
    // Third invocation settles before its delayed Long, without relying on a fact.
    world.down(check());
    assert!(channel::begin_check(&mut world.channel, Token::new(704)).expect("third actual invocation"));
    assert!(channel::end_check(&mut world.channel, Token::new(704)));
    world.down(long());
    world.settle();
    let notices: Vec<_> = world
        .messages
        .iter()
        .filter_map(|m| match m.kind() {
            0x204 => Some("long"),
            0x205 => Some("done"),
            _ => None,
        })
        .collect();
    assert_eq!(notices, ["long", "done", "long", "done", "long", "done"]);
}

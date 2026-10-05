use skein_lib::Token;
use temper_legacy_agent_domain_session::{self as session, llm, record};
use temper_legacy_agent_session_world::recorded::{self, World, opening, scenario, transcript};

#[test]
fn concrete_turns_resume_verbatim_without_local_tickets() {
    let old = scenario(1, 256);
    let mut resumed = World::new(1, 256);
    resumed.open(opening(Some(transcript(&old)), 100));
    assert!(resumed.end.is_none());
    let prompt = &resumed.prompts[0];
    assert_eq!(prompt.endpoint, llm::Endpoint(7));
    assert_eq!(prompt.messages[1].content[0], llm::Block::Opaque { bytes: recorded::OPAQUE.into() });
    assert!(matches!(prompt.messages[1].content[1], llm::Block::ToolCall { call: llm::Decoded::Historical, .. }));
    assert_eq!(prompt.messages[2].content[0], old.turns[0].messages[2].content[0]);
    assert_eq!(
        prompt.messages.last().expect("the scenario supplied a value").content[0],
        llm::Block::Text { text: b"wake".as_slice().into() }
    );
    resumed.complete(
        Box::new([llm::Block::Text { text: b"resumed".as_slice().into() }]),
        llm::Stop::EndTurn,
        llm::Usage::ZERO,
    );
    assert_eq!(resumed.turns[0].sequence, 3);
    assert_eq!(resumed.turns[0].spent, 0, "old activation spend is not charged again");
    resumed.close();
}

#[test]
fn transcript_refusals_precede_tools_and_completions() {
    type Change = Box<dyn Fn(&mut record::Transcript)>;
    let old = scenario(2, 256);
    let changes: Vec<(record::Refusal, Change)> = vec![
        (record::Refusal::Version, Box::new(|t| t.version = 99)),
        (record::Refusal::Endpoint, Box::new(|t| t.endpoint = llm::Endpoint(8))),
        (record::Refusal::Dialect, Box::new(|t| t.dialect = 24)),
        (record::Refusal::Version, Box::new(|t| t.turns[0].version = 99)),
        (record::Refusal::Endpoint, Box::new(|t| t.turns[0].endpoint = llm::Endpoint(8))),
        (record::Refusal::Dialect, Box::new(|t| t.turns[0].dialect = 24)),
        (record::Refusal::Malformed, Box::new(|t| t.turns[1].sequence = 9)),
        (
            record::Refusal::Malformed,
            Box::new(|t| {
                t.turns[0].messages[2].content[0] =
                    llm::Block::ToolResult { id: b"unmatched".as_slice().into(), result: llm::Returned::NotRun }
            }),
        ),
        (
            record::Refusal::Unresolved,
            Box::new(|t| {
                t.turns[0].messages[2].content[0] = llm::Block::ToolResult {
                    id: b"provider-call".as_slice().into(),
                    result: llm::Returned::Delegated {
                        answer: llm::Answer { ticket: Token::new(1), bytes: 1, error: false },
                    },
                }
            }),
        ),
        (record::Refusal::Unresolved, Box::new(|t| t.turns[0].messages[1].content = recorded::called())),
    ];
    for (reason, change) in changes {
        let mut history = transcript(&old);
        change(&mut history);
        let mut world = World::new(2, 256);
        world.open(opening(Some(history), 100));
        assert_eq!(world.end, Some(session::End::TranscriptRefused { reason }));
        assert!(world.prompts.is_empty());
        world.domain.reclaim();
        assert_eq!(world.domain.kits(), 0);
    }
}

#[test]
fn oversized_history_and_fresh_specs_are_refused_at_the_entrance() {
    let old = scenario(3, 256);
    let mut world = World::new(3, 256);
    world.env.limits.messages = 5;
    world.open(opening(Some(transcript(&old)), 100));
    assert_eq!(world.end, Some(session::End::TranscriptRefused { reason: record::Refusal::TooLarge }));
    let mut world = World::new(3, 256);
    world.env.limits.session_bytes = 512;
    world.open(opening(Some(transcript(&old)), 100));
    assert_eq!(world.end, Some(session::End::TranscriptRefused { reason: record::Refusal::TooLarge }));
    let mut world = World::new(3, 256);
    world.env.limits.spend = 99;
    world.open(opening(None, 100));
    assert_eq!(world.end, Some(session::End::Invalid));
    let mut world = World::new(3, 256);
    let mut spec = opening(None, 100);
    spec.prices.unit = 0;
    world.open(spec);
    assert_eq!(world.end, Some(session::End::Invalid));
}

#[test]
fn unit_budget_stops_after_the_crossing_turn_settles_and_child_counts_once() {
    let mut world = World::new(4, 256);
    world.open(opening(None, 20));
    world.complete(recorded::called(), llm::Stop::ToolUse, recorded::USAGE);
    let owner = world.delegated[0];
    world.step(session::Event::AnsweredV2 { owner, text: b"answer".as_slice().into(), error: false, spent: 9 });
    assert_eq!(world.end, Some(session::End::Budget { spent: session::Dimension::Unit }));
    assert_eq!(world.prompts.len(), 1);
    assert_eq!(world.turns[0].spent, 25);
    let trace = world.trace.clone();
    // Redelivery before reclaim and after reclaim is harmless, with no charge.
    for reclaim in [false, true] {
        if reclaim {
            world.domain.reclaim();
        }
        session::step(
            &mut world.domain,
            &world.env,
            session::Event::AnsweredV2 { owner, text: b"duplicate".as_slice().into(), error: false, spent: 9 },
            &mut world.out,
        );
        assert!(world.out.is_empty());
    }
    assert_eq!(trace, world.trace);
    world.close();
}

#[test]
fn pricing_rounds_the_combined_completion_and_rejects_overflow() {
    assert_eq!(recorded::PRICES.price(recorded::USAGE), Some(16));
    assert_eq!(
        record::Prices { input: 1, cached: 1, output: 1, unit: 3 }.price(llm::Usage {
            input_tokens: 1,
            output_tokens: 1,
            ..llm::Usage::ZERO
        }),
        Some(1)
    );
    assert_eq!(
        record::Prices { input: u64::MAX, cached: 0, output: 0, unit: 1 }
            .price(llm::Usage { input_tokens: 2, ..llm::Usage::ZERO }),
        None
    );
    let mut world = World::new(5, 256);
    let mut spec = opening(None, u64::MAX);
    spec.prices = record::Prices { input: u64::MAX, cached: 0, output: 0, unit: 1 };
    world.open(spec);
    world.complete(
        Box::new([llm::Block::Text { text: b"done".as_slice().into() }]),
        llm::Stop::EndTurn,
        llm::Usage { input_tokens: 2, ..llm::Usage::ZERO },
    );
    assert_eq!(world.spend, [(0, true)]);
    assert_eq!(world.end, Some(session::End::PriceOverflow));
    world.close();
}

#[test]
fn closing_preserves_withdrawn_and_late_answers_and_provider_completions() {
    for wins in [false, true] {
        let mut world = World::new(6, 256);
        world.open(opening(None, 100));
        world.complete(recorded::called(), llm::Stop::ToolUse, recorded::USAGE);
        let owner = world.delegated[0];
        world.step(session::Event::Close { session: world.session.expect("the scenario supplied a value") });
        if wins {
            world.step(session::Event::AnsweredV2 {
                owner,
                text: b"late child".as_slice().into(),
                error: false,
                spent: 9,
            });
            assert_eq!(world.turns[0].spent, 25);
        } else {
            world.step(session::Event::AnswerCancelledV2 { owner, spent: 9 });
            assert_eq!(world.turns[0].spent, 25, "withdrawn child spend is included");
            assert_eq!(
                world.turns[0].messages[2].content[0],
                llm::Block::ToolResult { id: b"provider-call".as_slice().into(), result: llm::Returned::Withdrawn }
            );
        }
        assert_eq!(world.end, Some(session::End::Closed));
        let mut resumed = World::new(6, 0);
        resumed.open(opening(Some(transcript(&world)), 100));
        assert!(resumed.end.is_none());
        resumed.close();
        world.close();
    }
    let mut world = World::new(6, 256);
    world.open(opening(None, 100));
    world.step(session::Event::Close { session: world.session.expect("the scenario supplied a value") });
    world.complete(recorded::called(), llm::Stop::EndTurn, recorded::USAGE);
    assert_eq!(world.turns[0].spent, 16);
    assert_eq!(world.turns[0].messages[1].content[0], llm::Block::Opaque { bytes: recorded::OPAQUE.into() });
    world.close();
}

#[test]
fn committed_call_results_after_a_yield_are_restored_without_tickets() {
    let mut world = World::new(7, 256);
    world.open(opening(None, 100));
    world.complete(recorded::called(), llm::Stop::EndTurn, recorded::USAGE);
    world.close();
    let mut history = transcript(&world);
    history.after = Box::new([llm::Message {
        role: llm::Role::User,
        content: Box::new([llm::Block::ToolResult {
            id: b"provider-call".as_slice().into(),
            result: llm::Returned::Text { text: b"committed answer".as_slice().into(), error: false },
        }]),
    }]);
    let mut resumed = World::new(7, 256);
    resumed.open(opening(Some(history), 100));
    assert_eq!(resumed.prompts[0].messages.len(), 4);
    assert_eq!(
        resumed.prompts[0].messages[2].content[0],
        llm::Block::ToolResult {
            id: b"provider-call".as_slice().into(),
            result: llm::Returned::Text { text: b"committed answer".as_slice().into(), error: false }
        }
    );
    resumed.close();
}

#[test]
fn replay_and_facts_capacity_change_no_decision() {
    let first = scenario(19, 256);
    let replayed = scenario(19, 256);
    assert_eq!(first.trace, replayed.trace);
    assert_eq!(first.snapshots, replayed.snapshots, "the complete frozen domain states replay too");
    assert_eq!(first.trace, scenario(19, 0).trace);
}

#[test]
fn yielded_historical_calls_resume_with_concrete_not_run_results() {
    let mut world = World::new(29, 256);
    world.open(opening(None, 100));
    world.complete(recorded::called(), llm::Stop::EndTurn, recorded::USAGE);
    world.close();
    let mut resumed = World::new(29, 256);
    resumed.open(opening(Some(transcript(&world)), 100));
    assert_eq!(
        resumed.prompts[0].messages[2].content[0],
        llm::Block::ToolResult { id: b"provider-call".as_slice().into(), result: llm::Returned::NotRun }
    );
    resumed.close();
}

#[test]
fn closing_a_resting_batch_keeps_its_real_result_and_marks_only_unstarted_calls() {
    use temper_legacy_agent_domain_tools::{Call, Part, Path};
    let mut world = World::new(30, 256);
    world.open(opening(None, 100));
    let mut blocks = recorded::called().into_vec();
    blocks.insert(
        0,
        llm::Block::ToolCall {
            id: b"denied-read".as_slice().into(),
            name: b"read_file".as_slice().into(),
            input: br#"{"path":"."}"#.as_slice().into(),
            call: llm::Decoded::Owned {
                call: Call::Read {
                    path: Path { absolute: false, parts: Box::new([Part::Current]) },
                    skip: 0,
                    lines: None,
                },
            },
        },
    );
    world.complete(blocks.into(), llm::Stop::ToolUse, recorded::USAGE);
    world.domain.reclaim();
    assert!(world.domain.is_ready(), "the tools' entrance refusal defers the next batch");
    assert!(world.delegated.is_empty());
    world.close();
    let results = &world.turns[0].messages[2].content;
    assert!(matches!(
        results[0],
        llm::Block::ToolResult {
            result: llm::Returned::Owned { outcome: temper_legacy_agent_domain_tools::Outcome::NotGranted },
            ..
        }
    ));
    assert_eq!(
        results[1],
        llm::Block::ToolResult { id: b"provider-call".as_slice().into(), result: llm::Returned::NotRun }
    );
}

#[test]
fn cumulative_child_spend_overflow_is_a_typed_failure() {
    let mut world = World::new(31, 256);
    world.open(opening(None, u64::MAX));
    world.complete(recorded::called(), llm::Stop::ToolUse, recorded::USAGE);
    world.step(session::Event::AnsweredV2 {
        owner: world.delegated[0],
        text: b"child".as_slice().into(),
        error: false,
        spent: u64::MAX,
    });
    assert_eq!(world.spend, [(16, false), (16, true)]);
    assert_eq!(world.end, Some(session::End::PriceOverflow));
    world.close();
}

#[test]
fn owned_io_cancellation_keeps_actual_terminal_results_in_the_turn() {
    use temper_legacy_agent_domain_tools::{
        Authority, Call, Done, Grants, Name, Op, Outcome, Part, Path, Repo, Version,
    };
    for wins in [false, true] {
        let mut world = World::new(32, 256);
        let mut spec = opening(None, 100);
        let name = || Name::new(b"repo".as_slice().into()).expect("a repository mount");
        spec.spec.authority = Authority {
            cwd: Box::new([name()]),
            repos: Box::new([Repo { mount: Box::new([name()]), root: Token::new(7), writable: false }]),
            grants: Grants { inspect: true, modify: false, shell: false },
            env: Box::default(),
        };
        world.open(spec);
        world.complete(
            Box::new([
                llm::Block::Opaque { bytes: recorded::OPAQUE.into() },
                llm::Block::ToolCall {
                    id: b"owned-read".as_slice().into(),
                    name: b"read_file".as_slice().into(),
                    input: br#"{"path":"data"}"#.as_slice().into(),
                    call: llm::Decoded::Owned {
                        call: Call::Read {
                            path: Path {
                                absolute: false,
                                parts: Box::new([Part::Name {
                                    name: Name::new(b"data".as_slice().into()).expect("a file name"),
                                }]),
                            },
                            skip: 0,
                            lines: None,
                        },
                    },
                },
            ]),
            llm::Stop::ToolUse,
            recorded::USAGE,
        );
        let (owner, op) = &world.operations[0];
        let owner = *owner;
        let Op::Load { at, .. } = op else {
            panic!("a read asks io to load the file");
        };
        assert_eq!((at.root, at.path.as_ref()), (Token::new(7), b"data".as_slice()));
        world.step(session::Event::Close { session: world.session.expect("admitted session") });
        assert_eq!(world.cancelled_operations, [owner]);
        assert!(world.turns.is_empty(), "the turn waits for the owned io terminal");
        let done = if wins {
            Done::Loaded { content: b"file bytes\n".as_slice().into(), version: Version::new([1, 0, 0, 0]) }
        } else {
            Done::Cancelled
        };
        world.step(session::Event::Done { owner, done });
        assert_eq!(world.end, Some(session::End::Closed));
        assert_eq!(world.turns[0].spent, 16);
        assert_eq!(world.turns[0].messages[1].content[0], llm::Block::Opaque { bytes: recorded::OPAQUE.into() });
        let llm::Block::ToolResult { result: llm::Returned::Owned { outcome }, .. } =
            &world.turns[0].messages[2].content[0]
        else {
            panic!("the actual owned result is recorded");
        };
        if wins {
            assert_eq!(
                *outcome,
                Outcome::Read {
                    content: b"file bytes\n".as_slice().into(),
                    skipped: 0,
                    lines: 1,
                    total: 1,
                    cut: false
                }
            );
        } else {
            assert_eq!(*outcome, Outcome::Cancelled);
        }
        let mut resumed = World::new(32, 0);
        resumed.open(opening(Some(transcript(&world)), 100));
        assert!(resumed.end.is_none(), "the settled owned terminal is resumable");
        resumed.close();
        world.close();
    }
}

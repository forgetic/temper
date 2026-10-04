//! The channel between the worker and an agent process, as the agent's
//! protocol layer translates it: every message each way, and the charters the
//! engine assigns, as its codec carries them, decoded into ones the agent's
//! run admits.

use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use temper_agent_domain::run::charter::{self, Checkout, Endpoint, Llm, Outlet, Repository};
use temper_agent_domain::run::facts::{self as run_facts, Answered, Asked, Return};
use temper_agent_domain::run::outcome::{Change, ChangeSpec, Child, Children, Declared, Field, OutcomeSpec, Verdict};
use temper_agent_domain::run::{self, Answer, Exhausted, Failure, Fault, Invalid, Place, Policy, Refusal, Spend};
use temper_agent_domain::session::llm::{self as session_llm, Stop, Usage};
use temper_agent_domain::{Event, Fact, Request, llm, session, tools};
use temper_agent_domain_world::LIMITS;
use temper_agent_domain_world::channel::{self, Link, MAX_TOKENS, Toward};
use temper_engine_domain::brief::{Body, Kind, Section, Unread};
use temper_engine_domain::plan::{self, Finish, Grants, Why};
use temper_engine_domain::views::{Capture, Policy as Capturing};
use temper_engine_domain::{Charter, Outcome};
use temper_engine_domain_world::codec;
use temper_worker_domain_agent::channel::{Ask, Down, Finish as Finished, Push, Reply, RunFailure, Up};

const WORKER: Token = Token::new(7);
const RUN: Token = Token::new(11);
const CALL: Token = Token::new(13);

fn bytes(text: &[u8]) -> Box<[u8]> {
    text.into()
}

/// The workspace of two repositories, the first writable.
fn checkout() -> Checkout {
    let repository = |name: &[u8], root, writable| Repository { name: bytes(name), root: Token::new(root), writable };
    Checkout { repositories: Box::new([repository(b"one", 1, true), repository(b"two", 2, false)]) }
}

fn link(run: Option<Token>) -> Link {
    Link { worker: WORKER, checkout: checkout(), run }
}

/// An engine's charter with a value of its own in every field, that may
/// finish as `finish`.
fn engine_charter(finish: Finish) -> Charter {
    Charter {
        why: Why::Produce,
        brief: Box::new([
            Section { kind: Kind::Item, body: Body::Text(bytes(b"Fix the parser.")) },
            Section { kind: Kind::Ci, body: Body::Missing(Unread::Failed) },
        ]),
        instructions: bytes(b"@coding Keep it small."),
        grants: Grants { modify: false, shell: true, forge: false, subagents: true, note: true },
        finish,
        budget: plan::Budget { tokens: 11, turns: 7, time: Duration::from_secs(23) },
        models: [b"main".as_slice(), b"small".as_slice(), b"large".as_slice()]
            .into_iter()
            .map(|model| temper_engine_domain::Model { endpoint: 0, model: bytes(model), max_tokens: MAX_TOKENS })
            .collect(),
        policy: Capturing {
            text: Capture::Content,
            progress: Capture::Shape,
            calls: Capture::Shape,
            tools: Capture::Nothing,
            usage: Capture::Shape,
        },
    }
}

/// The run's charter for [`engine_charter`] of a change, in [`checkout`].
fn run_charter() -> run::Charter {
    let llm = |model: &[u8]| Llm { account: 0, endpoint: Endpoint(0), model: bytes(model), max_tokens: MAX_TOKENS };
    let brief = b"@coding Keep it small.\n\nWhy: Produce\n\n## Item\nFix the parser.\n\n## Ci\n[unread: Failed]\n";
    run::Charter {
        brief: bytes(brief),
        checkout: checkout(),
        grants: charter::Grants {
            tools: charter::Tools { inspect: true, modify: false, shell: true },
            forge: false,
            agents: true,
            outlets: Box::new([Outlet { name: bytes(b"note") }]),
        },
        outcome: OutcomeSpec { change: Some(ChangeSpec { checks: true }), verdicts: Box::new([]) },
        budget: run::Budget {
            turns: 7,
            input: 5,
            output: 2,
            cache_read: 1,
            cache_write: 3,
            time: Duration::from_secs(23),
        },
        llm: llm(b"main"),
        models: Box::new([llm(b"small"), llm(b"large")]),
    }
}

#[test]
fn a_charter_decodes_field_by_field() {
    let encoded = codec::charter(&engine_charter(Finish::Change { checks: true }));
    assert_eq!(channel::charter(&encoded, checkout()), run_charter());
}

#[test]
fn what_a_run_may_finish_with_is_the_engines_outcome_spec() {
    let decode = |finish| channel::charter(&codec::charter(&engine_charter(finish)), checkout()).outcome;
    let report = decode(Finish::Report { grows: false });
    assert!(report.change.is_none());
    let names: Vec<&[u8]> = report.verdicts.iter().map(|rule| &*rule.name).collect();
    assert_eq!(names, [&b"report"[..]]);
    let review = decode(Finish::Verdict);
    assert!(review.change.is_none());
    let rules: Vec<(&[u8], Children)> = review.verdicts.iter().map(|rule| (&*rule.name, rule.children)).collect();
    assert_eq!(
        rules,
        [(&b"approve"[..], Children { min: 0, max: 0 }), (b"request-changes", Children { min: 1, max: 8 })]
    );
    assert_eq!(
        decode(Finish::Change { checks: false }),
        OutcomeSpec { change: Some(ChangeSpec { checks: false }), verdicts: Box::new([]) }
    );
}

#[test]
#[should_panic(expected = "no session reaches an agent")]
fn a_sessions_turn_is_asserted_against() {
    let encoded = codec::charter(&engine_charter(Finish::Turn { supervising: false }));
    let _ = channel::charter(&encoded, checkout());
}

#[test]
#[should_panic(expected = "a charter decodes as the engine's side encoded it")]
fn a_charter_with_bytes_left_over_is_asserted_against() {
    let mut encoded = codec::charter(&engine_charter(Finish::Verdict));
    encoded.push(0);
    let _ = channel::charter(&encoded, checkout());
}

#[test]
#[should_panic(expected = "a charter decodes as the engine's side encoded it")]
fn a_charter_cut_short_is_asserted_against() {
    let encoded = codec::charter(&engine_charter(Finish::Verdict));
    let _ = channel::charter(&encoded[..encoded.len() - 1], checkout());
}

#[test]
fn every_message_down_reaches_the_agent_as_it_should() {
    let encoded = codec::charter(&engine_charter(Finish::Change { checks: true }));
    let start = Down::Start { charter: encoded.into(), snapshot: None, repositories: repositories(), grants: grants() };
    let started = Event::Start {
        grants: Box::new([temper_agent_domain::Grant {
            name: temper_agent_domain::GrantName { account: 0, generation: 0 },
            valid: Duration::from_secs(100_000),
        }]),
        reply_to: ReplyTo::new(WORKER),
        worker: WORKER,
        charter: run_charter(),
    };
    assert_eq!(channel::down(start, &link(None)), Some(started));

    assert_eq!(
        channel::down(Down::Event { name: Token::new(1), event: bytes(b"a human said hello") }, &link(Some(RUN))),
        None
    );

    let answer = Down::Answer { call: CALL, reply: Reply::Pushed(Push::Done) };
    let pushed = Event::Pushed { owner: CALL, push: run::Push::Done };
    assert_eq!(channel::down(answer, &link(Some(RUN))), Some(pushed));

    assert_eq!(channel::down(Down::Cancel, &link(Some(RUN))), Some(Event::Cancel { run: RUN }));
    assert_eq!(channel::down(Down::Cancel, &link(None)), None, "a cancel that crossed a refusal finds no run");
}

#[test]
#[should_panic(expected = "the agent never parks")]
fn a_start_from_a_snapshot_is_asserted_against() {
    let encoded = codec::charter(&engine_charter(Finish::Verdict));
    let _ = channel::down(
        Down::Start {
            charter: encoded.into(),
            snapshot: Some(bytes(b"parked")),
            repositories: repositories(),
            grants: grants(),
        },
        &link(None),
    );
}

#[test]
fn every_reply_ends_the_push_it_answers() {
    let pushed = |push| Event::Pushed { owner: CALL, push };
    let replies = [
        (Reply::Pushed(Push::Done), pushed(run::Push::Done)),
        (Reply::Pushed(Push::Moved), pushed(run::Push::Moved)),
        (
            Reply::Pushed(Push::Failed {
                failure: temper_worker_domain_agent::PushFailure::new(temper_worker_domain_agent::PushReason::Refused),
            }),
            pushed(run::Push::Failed { failure: run::PushFailure::new(run::PushReason::Refused) }),
        ),
        (Reply::Pushed(Push::Nothing), pushed(run::Push::Nothing)),
        (
            Reply::Unavailable,
            pushed(run::Push::Failed { failure: run::PushFailure::new(run::PushReason::Unavailable) }),
        ),
        (Reply::Busy, pushed(run::Push::Failed { failure: run::PushFailure::new(run::PushReason::Busy) })),
        (Reply::TooLarge, pushed(run::Push::Failed { failure: run::PushFailure::new(run::PushReason::TooLarge) })),
        (Reply::Withdrawn, Event::HostCancelled { owner: CALL }),
    ];
    for (reply, event) in replies {
        assert_eq!(channel::answer(CALL, &reply), event, "{reply:?}");
    }
}

#[test]
#[should_panic(expected = "the run relays no calls")]
fn a_relayed_answer_is_asserted_against() {
    let _ = channel::answer(CALL, &Reply::Relayed { answer: bytes(b"the issue") });
}

fn verdict() -> Declared {
    let field = |name: &[u8], value: &[u8]| Field { name: bytes(name), value: bytes(value) };
    let children = Box::new([
        Child { kind: bytes(b"nit"), fields: Box::new([field(b"path", b"src/lib.rs"), field(b"body", b"A typo.")]) },
        Child { kind: bytes(b"blocking"), fields: Box::new([]) },
    ]);
    Declared::Verdict(Verdict { name: bytes(b"request-changes"), body: bytes(b"Two things."), children })
}

fn change() -> Declared {
    Declared::Change(Change { title: bytes(b"Fix the parser"), body: bytes(b"It dropped the last token.") })
}

/// The outcome the run finished with, as the engine's side reads back what
/// went up.
fn ended(finish: Finished) -> Outcome {
    let Finished::Ended { outcome } = finish else { panic!("expected an ending, got {finish:?}") };
    codec::outcome_of(&outcome).expect("an outcome decodes")
}

#[test]
fn every_answer_finishes_the_run_as_it_should() {
    let spent = Spend { turns: 3, input: 100, output: 50, cache_read: 0, cache_write: 0 };
    let message = bytes(b"Fix the parser\n\nIt dropped the last token.");
    assert_eq!(ended(channel::finish(Answer::Accepted { outcome: change(), spent })), Outcome::Change { message });
    let text = bytes(b"Two things.\n- nit path: src/lib.rs body: A typo.\n- blocking");
    let changes = Outcome::Verdict { verdict: plan::Verdict::Changes, text };
    assert_eq!(ended(channel::finish(Answer::Accepted { outcome: verdict(), spent })), changes);
    let named =
        |name: &[u8]| Declared::Verdict(Verdict { name: bytes(name), body: bytes(b"Fine."), children: Box::new([]) });
    let approve = Outcome::Verdict { verdict: plan::Verdict::Approve, text: bytes(b"Fine.") };
    assert_eq!(ended(channel::finish(Answer::Accepted { outcome: named(b"approve"), spent })), approve);
    let report = Outcome::Report { text: bytes(b"Fine.") };
    assert_eq!(ended(channel::finish(Answer::Accepted { outcome: named(b"report"), spent })), report);

    let failures = [
        (Answer::Failed { failure: Failure::Model(Fault::Provider), spent }, RunFailure::Model),
        (Answer::Failed { failure: Failure::Budget(Exhausted::Turns), spent }, RunFailure::Budget),
        (
            Answer::Failed { failure: Failure::Policy(Policy::Unfinished { nudges: 1, rejected: 2 }), spent },
            RunFailure::Policy,
        ),
        (Answer::Failed { failure: Failure::Cancelled, spent }, RunFailure::Cancelled),
        (Answer::Failed { failure: Failure::Stale, spent }, RunFailure::Stale),
        (Answer::Refused(Refusal::Busy), RunFailure::Policy),
        (Answer::Refused(Refusal::Invalid(Invalid::Budget)), RunFailure::Policy),
        (Answer::Refused(Refusal::Invalid(Invalid::Conversation)), RunFailure::Policy),
    ];
    for (answer, failure) in failures {
        assert_eq!(channel::finish(answer), Finished::Failed { failure });
    }
}

#[test]
#[should_panic(expected = "a run declares only the verdicts its charter allows")]
fn a_verdict_no_charter_allows_is_asserted_against() {
    let declared = Declared::Verdict(Verdict { name: bytes(b"maybe"), body: bytes(b""), children: Box::new([]) });
    let _ = channel::outcome(&declared);
}

#[test]
fn every_request_goes_where_it_should() {
    let now = Time::ZERO.saturating_add(Duration::from_secs(100));
    let worker = Toward::Worker;

    let admitted = Request::Admitted { worker: WORKER, run: RUN };
    assert_eq!(channel::up(admitted, now), Toward::Admitted { run: RUN });

    let answer = Request::Answer { to: ReplyTo::new(WORKER), answer: Answer::Refused(Refusal::Busy) };
    let finish = Finished::Failed { failure: RunFailure::Policy };
    assert_eq!(channel::up(answer, now), worker(Up::Finish { finish }));

    let deadline = now.saturating_add(Duration::from_secs(60));
    let checking = Request::Checking { worker: WORKER, deadline };
    assert_eq!(channel::up(checking, now), worker(Up::Long { span: Duration::from_secs(60) }));

    let change = Change { title: bytes(b"Fix the parser"), body: bytes(b"It dropped the last token.") };
    let push = Request::Push { worker: WORKER, owner: CALL, change };
    let message = bytes(b"Fix the parser\n\nIt dropped the last token.");
    assert_eq!(channel::up(push, now), worker(Up::Call { call: CALL, ask: Ask::Push { message } }));
    let change = Change { title: bytes(b"Fix the parser"), body: bytes(b"") };
    let push = Request::Push { worker: WORKER, owner: CALL, change };
    let message = bytes(b"Fix the parser");
    assert_eq!(channel::up(push, now), worker(Up::Call { call: CALL, ask: Ask::Push { message } }));

    assert_eq!(channel::up(Request::CancelHost { owner: CALL }, now), worker(Up::Withdraw { call: CALL }));

    for (request, same) in below(deadline).into_iter().zip(below(deadline)) {
        assert_eq!(channel::up(request, now), Toward::Below(same));
    }
}

/// A request of each kind that is not the worker's.
fn below(deadline: Time) -> [Request; 8] {
    let prompt = llm::Prompt {
        endpoint: session_llm::Endpoint(0),
        model: bytes(b"main"),
        system: bytes(b"You fix parsers."),
        tools: tools::Grants { inspect: true, modify: false, shell: false },
        served: Box::new([llm::Served::Finish]),
        messages: Box::new([]),
        max_tokens: 1024,
    };
    let place = || Place { root: Token::new(1), path: bytes(b".temper/pre-pr") };
    let tool_place = tools::Place { root: Token::new(1), path: bytes(b"src/lib.rs") };
    [
        Request::Complete {
            owner: CALL,
            grant: temper_agent_domain::GrantName { account: 0, generation: 0 },
            prompt,
            timeout: Duration::from_secs(60),
        },
        Request::Cancel { owner: CALL },
        Request::Io { owner: CALL, op: tools::Op::Load { at: tool_place, max: 4096 }, deadline },
        Request::CancelIo { owner: CALL },
        Request::Read { owner: CALL, at: place(), max: 4096, deadline },
        Request::Probe { owner: CALL, at: place(), deadline },
        Request::Check { owner: CALL, program: place(), deadline, tail: 512 },
        Request::Abort { owner: CALL },
    ]
}

#[test]
fn every_fact_goes_up_as_its_kind() {
    let run = RUN;
    let opener = Token::new(17);
    let session = Token::new(19);
    let usage = Usage::ZERO;
    let tool = tools::Tool::Read;
    let run_facts = [
        run_facts::Fact::Admitted { run },
        run_facts::Fact::Prepared { run, guides: 1, checks: 1 },
        run_facts::Fact::Opened { run, conversation: opener, depth: 0 },
        run_facts::Fact::Ended { run, conversation: opener, end: run::End::Closed },
        run_facts::Fact::Called { run, conversation: opener, call: CALL, ask: Asked::Finish },
        run_facts::Fact::Returned { run, call: CALL, result: Return::Accepted },
        run_facts::Fact::CheckStarted { run, deadline: Time::ZERO },
        run_facts::Fact::Pushed { run, push: run::Push::Done },
        run_facts::Fact::Answered { run, answer: Answered::Accepted },
    ];
    let session_facts = [
        session::Fact::Opened { opener },
        session::Fact::CompletionStarted { opener, attempt: 0, messages: 1, max_tokens: 1024 },
        session::Fact::CompletionAnswered { opener, stop: Stop::EndTurn, blocks: 1, calls: 0, invalid: 0 },
        session::Fact::CompletionFailed { opener, failure: session_llm::Failure::Overloaded },
        session::Fact::CompletionCancelled { opener },
        session::Fact::CompletionRetried { opener, attempt: 1, delay: Duration::from_secs(1) },
        session::Fact::DelegateStarted { opener, block: 0 },
        session::Fact::DelegateAnswered { opener, bytes: 10, error: false },
        session::Fact::DelegateCancelled { opener },
        session::Fact::Yielded { opener, stop: session::Yield::Done },
        session::Fact::Used { opener, usage },
        session::Fact::Ended { opener, end: session::End::Busy, turns: 1, usage },
    ];
    let tools_facts = [
        tools::Fact::Opened { session },
        tools::Fact::Refused { session, refusal: tools::Refusal::Busy },
        tools::Fact::Started { session, tool },
        tools::Fact::Answered { session, tool, verdict: tools::Verdict::Read, bytes: 10 },
        tools::Fact::Closing { session, running: 0 },
        tools::Fact::Closed { session },
    ];
    let facts = run_facts
        .into_iter()
        .map(|fact| Fact::Run { fact })
        .chain(session_facts.into_iter().map(|fact| Fact::Session { fact }))
        .chain(tools_facts.into_iter().map(|fact| Fact::Session { fact: session::Fact::Tools { opener, fact } }));
    let mut kinds = Vec::new();
    for fact in facts {
        let Up::Fact { fact: kind } = channel::fact(fact) else { panic!("{fact:?} goes up as a fact") };
        assert!(!kinds.contains(&kind), "{fact:?} has a kind of its own");
        kinds.push(kind);
    }
    assert_eq!(kinds.len(), 9 + 12 + 6);

    let finished = Fact::Run { fact: run_facts::Fact::CheckFinished { run, exit: run::Exit::Code { code: 0 } } };
    assert_eq!(channel::fact(finished), Up::LongDone);
}

#[test]
fn charters_the_engine_assigns_decode_into_ones_the_run_admits() {
    let finishes = [Finish::Report { grows: false }, Finish::Change { checks: true }, Finish::Verdict];
    for (seed, finish) in (0..).zip(finishes) {
        let charter = Charter {
            budget: plan::Budget { tokens: 1 << 20, turns: 64, time: Duration::from_secs(600) },
            ..engine_charter(finish)
        };
        let encoded = codec::charter(&charter);
        let mut agent = temper_agent_domain::Domain::new(&LIMITS, seed);
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: LIMITS };
        let mut out = Queue::with_capacity(temper_agent_domain::max_out(&LIMITS));
        let start =
            Down::Start { charter: encoded.into(), snapshot: None, repositories: repositories(), grants: grants() };
        let event = channel::down(start, &link(None)).expect("a start is heard");
        temper_agent_domain::step(&mut agent, &env, event, &mut out);
        let admitted = out.iter().filter(|request| matches!(request, Request::Admitted { .. })).count();
        assert_eq!(admitted, 1, "the run admits the charter of {finish:?}");
        assert_eq!(channel::fact(agent.pop_fact().expect("a fact is told")), Up::Fact { fact: bytes(b"run.admitted") });
    }
}

#[test]
fn a_budgets_tokens_are_split_across_the_kinds_spending_no_more_than_given() {
    for tokens in [0, 1, 7, 11, 2_000, 1 << 20] {
        let budget = channel::split(plan::Budget { tokens, turns: 3, time: Duration::from_secs(1) });
        assert_eq!(budget.input + budget.output + budget.cache_read + budget.cache_write, tokens, "{budget:?}");
        assert!(budget.input >= budget.output && budget.output >= budget.cache_read, "{budget:?}");
        assert_eq!((budget.turns, budget.time), (3, Duration::from_secs(1)));
    }
}

#[test]
fn a_push_failure_reaches_the_llm_as_its_reason_and_actual_diagnostic_output() {
    use temper_agent_domain_world::translate;
    use temper_fake_llm_domain::api::Part;
    use temper_worker_domain_agent::{PushDiagnostic, PushFailure, PushReason};

    let failure = PushFailure {
        repository: Some(2),
        reason: PushReason::Refused,
        diagnostic: PushDiagnostic::new(b"remote: hook declined: missing changelog", 37),
    };
    let Event::Pushed { push: run::Push::Failed { failure }, .. } =
        channel::answer(CALL, &Reply::Pushed(Push::Failed { failure }))
    else {
        panic!("a failed push retains its feedback")
    };
    let prompt = llm::Prompt {
        endpoint: llm::Endpoint(0),
        model: bytes(b"test"),
        system: Box::new([]),
        tools: tools::Grants { inspect: false, modify: false, shell: false },
        served: Box::new([]),
        messages: Box::new([llm::Message {
            role: llm::Role::User,
            content: Box::new([llm::Block::ToolResult {
                id: bytes(b"finish"),
                result: llm::Returned::Served { returned: run::Returned::Unpushed { failure }, error: true },
            }]),
        }]),
        max_tokens: 100,
    };
    let query = translate::query(prompt);
    let Part::ToolOutput { output, is_error, .. } = &query.messages[0].parts[0] else {
        panic!("the provider receives a tool result")
    };
    assert!(*is_error);
    assert_eq!(
        output.as_ref(),
        b"push failed: Refused (repository 2); 37 diagnostic bytes omitted\nremote: hook declined: missing changelog"
    );
}

fn repositories() -> Box<[temper_worker_domain_agent::channel::Repository]> {
    checkout()
        .repositories
        .iter()
        .map(|placed| temper_worker_domain_agent::channel::Repository {
            name: placed.name.clone(),
            writable: placed.writable,
        })
        .collect()
}
fn grants() -> Box<[temper_worker_domain_agent::channel::Grant]> {
    Box::new([temper_worker_domain_agent::channel::Grant {
        account: 0,
        generation: 0,
        valid: Duration::from_secs(100_000),
    }])
}

//! The channel between the worker and an agent process, as the agent's
//! protocol layer translates it: every message each way, and the charters the
//! fake engine draws, decoded into ones the agent's run admits.

use temper_agent_model::run::charter::{self, Checkout, Endpoint, Llm, Outlet, Repository};
use temper_agent_model::run::facts::{self as run_facts, Answered, Asked, Return};
use temper_agent_model::run::outcome::VerdictRule;
use temper_agent_model::run::outcome::{Change, ChangeSpec, Child, Children, Declared, Field, OutcomeSpec, Verdict};
use temper_agent_model::run::{self, Answer, Exhausted, Failure, Fault, Invalid, Place, Policy, Refusal, Spend};
use temper_agent_model::session::llm::{self as session_llm, Stop, Usage};
use temper_agent_model::{Event, Fact, Request, llm, session, tools};
use temper_agent_model_tests::LIMITS;
use temper_agent_model_tests::channel::{self, Link, Toward};
use temper_fake_engine_model::api::{self as engine, Access, Assignment, Hello, Workspace};
use temper_fake_engine_model::{Config, MAX_OUT, Origin};
use temper_lib::{Duration, Env, Queue, ReplyTo, Time, Token};
use temper_worker_model_agent::channel::{Ask, Down, Finish, Push, Reply, RunFailure, Up};

const WORKER: Token = Token::new(7);
const RUN: Token = Token::new(11);
const CALL: Token = Token::new(13);

fn bytes(text: &[u8]) -> Box<[u8]> {
    text.into()
}

/// The workspace of two repositories, the first writable.
fn checkout() -> Checkout {
    let repository = |name: &[u8], root, writable| Repository { name: bytes(name), root: Token::new(root), writable };
    Checkout { repositories: Box::new([repository(b"temper", 1, true), repository(b"docs", 2, false)]) }
}

fn link(run: Option<Token>) -> Link {
    Link { worker: WORKER, checkout: checkout(), run }
}

/// An engine's charter with a value of its own in every field.
fn engine_charter() -> engine::Charter {
    engine::Charter {
        brief: bytes(b"Fix the parser."),
        tools: engine::Tools { read: true, write: false, shell: true },
        forge: false,
        agents: true,
        outlets: Box::new([bytes(b"comment"), bytes(b"label")]),
        outcome: engine::Outcome {
            change: true,
            checks: true,
            verdicts: Box::new([engine::Verdict {
                name: bytes(b"request-changes"),
                min_children: 1,
                max_children: 3,
                kinds: Box::new([bytes(b"nit")]),
                fields: Box::new([bytes(b"path"), bytes(b"body")]),
            }]),
        },
        budget: engine::Budget {
            turns: 7,
            input_tokens: 11,
            output_tokens: 13,
            cache_read_tokens: 17,
            cache_write_tokens: 19,
            wall_time: Duration::from_secs(23),
        },
        endpoint: 29,
        model: bytes(b"main"),
        max_tokens: 31,
        models: Box::new([bytes(b"small"), bytes(b"large")]),
    }
}

/// The run's charter for [`engine_charter`], in [`checkout`].
fn run_charter() -> run::Charter {
    let llm = |model: &[u8]| Llm { endpoint: Endpoint(29), model: bytes(model), max_tokens: 31 };
    run::Charter {
        brief: bytes(b"Fix the parser."),
        checkout: checkout(),
        grants: charter::Grants {
            tools: charter::Tools { inspect: true, modify: false, shell: true },
            forge: false,
            agents: true,
            outlets: Box::new([Outlet { name: bytes(b"comment") }, Outlet { name: bytes(b"label") }]),
        },
        outcome: OutcomeSpec {
            change: Some(ChangeSpec { checks: true }),
            verdicts: Box::new([VerdictRule {
                name: bytes(b"request-changes"),
                children: Children { min: 1, max: 3 },
                kinds: Box::new([bytes(b"nit")]),
                fields: Box::new([bytes(b"path"), bytes(b"body")]),
            }]),
        },
        budget: run::Budget {
            turns: 7,
            input: 11,
            output: 13,
            cache_read: 17,
            cache_write: 19,
            time: Duration::from_secs(23),
        },
        llm: llm(b"main"),
        models: Box::new([llm(b"small"), llm(b"large")]),
    }
}

#[test]
fn a_charter_decodes_field_by_field() {
    let encoded = temper_fake_engine_model::charter::encode(&engine_charter());
    assert_eq!(channel::charter(&encoded, checkout()), run_charter());
}

#[test]
#[should_panic(expected = "an encoding is all its bytes")]
fn a_charter_with_bytes_left_over_is_asserted_against() {
    let mut encoded = temper_fake_engine_model::charter::encode(&engine_charter()).into_vec();
    encoded.push(0);
    let _ = channel::charter(&encoded, checkout());
}

#[test]
#[should_panic(expected = "an encoding holds what it counts")]
fn a_charter_cut_short_is_asserted_against() {
    let encoded = temper_fake_engine_model::charter::encode(&engine_charter());
    let _ = channel::charter(&encoded[..encoded.len() - 1], checkout());
}

#[test]
fn every_message_down_reaches_the_agent_as_it_should() {
    let encoded = temper_fake_engine_model::charter::encode(&engine_charter());
    let start = Down::Start { charter: encoded, snapshot: None };
    let started = Event::Start { reply_to: ReplyTo::new(WORKER), worker: WORKER, charter: run_charter() };
    assert_eq!(channel::down(start, &link(None)), Some(started));

    assert_eq!(channel::down(Down::Event { event: bytes(b"a human said hello") }, &link(Some(RUN))), None);

    let answer = Down::Answer { call: CALL, reply: Reply::Pushed(Push::Done) };
    let pushed = Event::Pushed { owner: CALL, push: run::Push::Done };
    assert_eq!(channel::down(answer, &link(Some(RUN))), Some(pushed));

    assert_eq!(channel::down(Down::Cancel, &link(Some(RUN))), Some(Event::Cancel { run: RUN }));
    assert_eq!(channel::down(Down::Cancel, &link(None)), None, "a cancel that crossed a refusal finds no run");
}

#[test]
#[should_panic(expected = "the agent never parks")]
fn a_start_from_a_snapshot_is_asserted_against() {
    let encoded = temper_fake_engine_model::charter::encode(&engine_charter());
    let _ = channel::down(Down::Start { charter: encoded, snapshot: Some(bytes(b"parked")) }, &link(None));
}

#[test]
fn every_reply_ends_the_push_it_answers() {
    let pushed = |push| Event::Pushed { owner: CALL, push };
    let replies = [
        (Reply::Pushed(Push::Done), pushed(run::Push::Done)),
        (Reply::Pushed(Push::Moved), pushed(run::Push::Moved)),
        (Reply::Pushed(Push::Failed), pushed(run::Push::Failed)),
        (Reply::Pushed(Push::Nothing), pushed(run::Push::Failed)),
        (Reply::Unavailable, pushed(run::Push::Failed)),
        (Reply::Busy, pushed(run::Push::Failed)),
        (Reply::TooLarge, pushed(run::Push::Failed)),
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

/// The outcome the run finished with, read back from what went up.
fn ended(finish: Finish) -> Declared {
    let Finish::Ended { outcome } = finish else { panic!("expected an ending, got {finish:?}") };
    channel::declared(&outcome)
}

#[test]
fn every_answer_finishes_the_run_as_it_should() {
    let spent = Spend { turns: 3, input: 100, output: 50, cache_read: 0, cache_write: 0 };
    assert_eq!(ended(channel::finish(Answer::Accepted { outcome: change(), spent })), change());
    assert_eq!(ended(channel::finish(Answer::Accepted { outcome: verdict(), spent })), verdict());
    let empty = Declared::Verdict(Verdict { name: bytes(b"approve"), body: bytes(b""), children: Box::new([]) });
    let again = Declared::Verdict(Verdict { name: bytes(b"approve"), body: bytes(b""), children: Box::new([]) });
    assert_eq!(ended(channel::finish(Answer::Accepted { outcome: empty, spent })), again);

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
        assert_eq!(channel::finish(answer), Finish::Failed { failure });
    }
}

#[test]
#[should_panic(expected = "an outcome is a change (0) or a verdict (1)")]
fn an_outcome_of_neither_kind_is_asserted_against() {
    let _ = channel::declared(&[2]);
}

#[test]
fn every_request_goes_where_it_should() {
    let now = Time::ZERO.saturating_add(Duration::from_secs(100));
    let worker = Toward::Worker;

    let admitted = Request::Admitted { worker: WORKER, run: RUN };
    assert_eq!(channel::up(admitted, now), Toward::Admitted { run: RUN });

    let answer = Request::Answer { to: ReplyTo::new(WORKER), answer: Answer::Refused(Refusal::Busy) };
    let finish = Finish::Failed { failure: RunFailure::Policy };
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
        Request::Complete { owner: CALL, prompt, timeout: Duration::from_secs(60) },
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

/// The workers' hello and the configuration that the fake engine draws
/// assignments from: charters of every shape the agent's limits take.
fn config(items: u32) -> Config {
    let origin = |name: &[u8], remote: &[u8]| Origin { name: bytes(name), remote: bytes(remote) };
    Config {
        items,
        window: Duration::from_secs(10),
        workers: 1,
        workstreams: Box::new([bytes(b"parser"), bytes(b"docs")]),
        repositories: Box::new([origin(b"temper", b"ai/temper"), origin(b"docs", b"ai/docs")]),
        spread_min: 1,
        spread_max: 2,
        commits: 300,
        branches: 300,
        writable: 500,
        saves: 500,
        invalid: 0,
        brief_min: 1,
        brief_max: 4096,
        turns_min: 1,
        turns_max: LIMITS.run.budget.turns,
        tokens_min: 1,
        tokens_max: LIMITS.run.budget.input,
        time_min: Duration::from_secs(1),
        time_max: LIMITS.run.budget.time,
        max_tokens: LIMITS.run.max_tokens,
        changes: 500,
        checks: 500,
        verdicts: 500,
        agents: 500,
        attempts: 1,
        transient: 0,
        permanent: 0,
        backoff_min: Duration::from_secs(1),
        backoff_max: Duration::from_secs(1),
        wakes: 0,
        wake_min: Duration::from_secs(1),
        wake_max: Duration::from_secs(1),
        resumes: 0,
        overbook: 0,
        inbound: 0,
        inbound_min: Duration::from_secs(1),
        inbound_max: Duration::from_secs(1),
        event_min: 1,
        event_max: 1,
        resends: 0,
        cancels: 0,
        late_cancels: 0,
        stale: 0,
        cancel_min: Duration::from_secs(1),
        cancel_max: Duration::from_secs(1),
        calls: 1,
        relay_min: Duration::from_secs(1),
        relay_max: Duration::from_secs(1),
        relay_errors: 0,
        answer_min: 1,
        answer_max: 1,
        grace: Duration::from_secs(1),
        keeps: 0,
    }
}

/// Every assignment the fake engine makes of `config`'s items to one worker
/// with a slot for each, each answered as it comes, so that the next item of
/// its workstream may follow.
fn assignments(config: Config, seed: u64) -> Vec<Assignment> {
    let items = config.items;
    let mut model = temper_fake_engine_model::Model::new(&config, seed);
    let mut env = Env { now: Time::ZERO, limits: config };
    let mut out = Queue::with_capacity(MAX_OUT);
    let hello = Hello { slots: items, workstreams: Box::new([]), hosting: Box::new([]) };
    let hello = temper_fake_engine_model::Event::Hello { worker: WORKER, hello };
    temper_fake_engine_model::step(&mut model, &env, hello, &mut out);
    let mut assigned = Vec::new();
    while assigned.len() < usize::try_from(items).expect("a u32 fits in a usize") {
        if model.is_ready() {
            temper_fake_engine_model::resume(&mut model, &env, &mut out);
        } else {
            env.now = model.next_deadline().expect("an item falls due");
            temper_fake_engine_model::fire(&mut model, &env, &mut out);
        }
        while let Some(request) = out.pop() {
            let temper_fake_engine_model::Request::Assign { worker: _, assignment } = request else {
                panic!("expected an assignment, got {request:?}");
            };
            let (run, attempt) = (assignment.run, assignment.attempt);
            let work = temper_fake_engine_model::api::Work { landed: Box::new([]), saved: None };
            let answer = temper_fake_engine_model::api::Answer::Ended { outcome: bytes(b"done"), work };
            let answered = temper_fake_engine_model::Event::Answered { worker: WORKER, run, attempt, answer };
            temper_fake_engine_model::step(&mut model, &env, answered, &mut out);
            let acknowledged = out.pop();
            let Some(temper_fake_engine_model::Request::Acknowledge { .. }) = acknowledged else {
                panic!("expected the answer acknowledged, got {acknowledged:?}");
            };
            assigned.push(assignment);
        }
    }
    assigned
}

/// The repositories of `workspace`, as io put them, each at a root of its own.
fn prepared(workspace: &Workspace) -> Checkout {
    let repositories = (1..)
        .zip(&workspace.repositories)
        .map(|(root, repository)| Repository {
            name: repository.name.clone(),
            root: Token::new(root),
            writable: match repository.access {
                Access::ReadOnly => false,
                Access::Writable { .. } => true,
            },
        })
        .collect();
    Checkout { repositories }
}

#[test]
fn charters_the_fake_engine_draws_decode_into_ones_the_run_admits() {
    let config = config(16);
    let (mut changes, mut verdicts) = (0, 0);
    for seed in 0..8 {
        for assignment in assignments(config.clone(), seed) {
            let checkout = prepared(&assignment.workspace);
            let decoded = channel::charter(&assignment.charter, checkout.clone());
            let main = Llm { endpoint: Endpoint(0), model: bytes(b"fake-1"), max_tokens: config.max_tokens };
            assert_eq!(decoded.llm, main);
            let models: Vec<&[u8]> = decoded.models.iter().map(|llm| &*llm.model).collect();
            assert_eq!(models, [&b"fake-2"[..], b"fake-3"]);
            assert!(
                decoded.models.iter().all(|llm| llm.endpoint == main.endpoint && llm.max_tokens == main.max_tokens)
            );
            assert!(decoded.grants.tools.inspect, "reading is always granted");
            let writable = checkout.repositories.iter().any(|repository| repository.writable);
            assert!(decoded.outcome.change.is_none() || writable, "only a writable workspace takes a change");
            changes += u32::from(decoded.outcome.change.is_some());
            verdicts += u32::from(!decoded.outcome.verdicts.is_empty());
            assert_eq!(decoded.checkout, checkout);

            let mut agent = temper_agent_model::Model::new(&LIMITS, seed);
            let env = Env { now: Time::ZERO, limits: LIMITS };
            let mut out = Queue::with_capacity(temper_agent_model::max_out(&LIMITS));
            let start = Down::Start { charter: assignment.charter, snapshot: None };
            let event = channel::down(start, &Link { worker: WORKER, checkout, run: None }).expect("a start is heard");
            temper_agent_model::step(&mut agent, &env, event, &mut out);
            let admitted = out.iter().filter(|request| matches!(request, Request::Admitted { .. })).count();
            assert_eq!(admitted, 1, "the run admits the charter: {decoded:?}");
            assert_eq!(
                channel::fact(agent.pop_fact().expect("a fact is told")),
                Up::Fact { fact: bytes(b"run.admitted") }
            );
        }
    }
    assert!(changes > 0 && verdicts > 0, "charters of both outcomes were drawn");
}

//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;
use core::mem::size_of;

use temper_agent_model_tools::{
    self as tools, Authority, Call, Done, Effect, Grants, Name, Op, Outcome, Part, Path, Place, Repo, Version,
};
use temper_lib::{Duration, Env, List, Queue, Time, Token};

use crate::llm::{
    Answer, Block, Completion, Decoded, Descriptor, Endpoint, Failure, Message, Problem, Prompt, Returned, Role, Stop,
    Usage,
};
use crate::{
    Budget, Dimension, End, Event, Fact, Limits, MAX_PARALLEL, Model, Request, Spec, Yield, fire, max_out, resume,
    step, worst_case,
};

/// The budget every spec asks for, unless a test says otherwise: the most the
/// limits allow.
const BUDGET: Budget = Budget {
    turns: 4,
    input: 1_000_000,
    output: 1_000_000,
    cache_read: 1_000_000,
    cache_write: 1_000_000,
    time: Duration::from_secs(600),
};

/// The tools' limits: a kit for each session, room for a batch and more.
const TOOLS: tools::Limits = tools::Limits {
    kits: 2,
    calls: 8,
    repos: 2,
    path_bytes: 64,
    known_files: 8,
    file_bytes: 65_536,
    read_bytes: 4096,
    list_entries: 16,
    match_lines: 4,
    file_timeout: Duration::from_secs(60),
    env_bytes: 64,
    shell_timeout: Duration::from_secs(60),
    shell_timeout_max: Duration::from_secs(600),
    shell_head: 64,
    shell_tail: 64,
    search_hits: 8,
    search_bytes: 256,
    search_timeout: Duration::from_secs(30),
    facts: 64,
};

const LIMITS: Limits = Limits {
    sessions: 2,
    messages: 8,
    session_bytes: 65_536,
    budget: BUDGET,
    max_tokens: 1024,
    retries: 2,
    backoff_base: Duration::from_millis(100),
    backoff_max: Duration::from_secs(1),
    call_timeout: Duration::from_secs(30),
    tool_timeout: Duration::from_secs(20),
    facts: 64,
    parallel_tools: 2,
    tools: TOOLS,
};

const OUT_OF_TIME: End = End::Budget { spent: Dimension::Time };

/// The model, its environment, room for one step's output, and the `Used`
/// it has reported.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
    turns: u32,
    usage: Usage,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        Harness {
            model: Model::new(&limits, 1),
            env: Env { now: Time::ZERO, limits },
            out: Queue::with_capacity(max_out(&limits)),
            turns: 0,
            usage: Usage::ZERO,
        }
    }

    /// Steps the model with `event`, which emits at most one request besides
    /// `Used`.
    fn step(&mut self, event: Event) -> Option<Request> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.one()
    }

    /// Fires the alarm that is due, which emits at most one request besides
    /// `Used`.
    fn fire(&mut self) -> Option<Request> {
        assert!(self.model.is_due(self.env.now), "an alarm is due");
        fire(&mut self.model, &self.env, &mut self.out);
        self.one()
    }

    /// Starts the session that is ready, which emits at most one request
    /// besides `Used`.
    fn resume(&mut self) -> Option<Request> {
        assert!(self.model.is_ready(), "a session is ready");
        resume(&mut self.model, &self.env, &mut self.out);
        self.one()
    }

    /// The one request out besides `Used`, which is tallied.
    fn one(&mut self) -> Option<Request> {
        let mut one = None;
        while let Some(request) = self.out.pop() {
            match request {
                Request::Used { opener: _, usage } => {
                    assert!(one.is_none(), "a completion's usage comes first");
                    self.turns = self.turns.saturating_add(1);
                    self.usage = self.usage.saturating_add(usage);
                }
                request @ (Request::Opened { .. }
                | Request::Yielded { .. }
                | Request::Ended { .. }
                | Request::Complete { .. }
                | Request::Cancel { .. }
                | Request::Io { .. }
                | Request::CancelIo { .. }
                | Request::Delegate { .. }
                | Request::Withdraw { .. }) => {
                    assert!(one.is_none(), "one request at most besides Used");
                    one = Some(request);
                }
            }
        }
        one
    }

    /// Steps the model with `event`, which starts or cancels a batch of tool
    /// runs, and returns the tokens of the runs it names, in order.
    fn batch(&mut self, event: Event) -> List<Token> {
        step(&mut self.model, &self.env, event, &mut self.out);
        let mut runs = List::with_capacity(max_out(&self.env.limits));
        while let Some(request) = self.out.pop() {
            match request {
                Request::Io { owner, .. }
                | Request::CancelIo { owner }
                | Request::Delegate { owner, .. }
                | Request::Withdraw { owner } => {
                    runs.push(owner).expect("room for a batch");
                }
                Request::Used { .. } => {}
                other @ (Request::Opened { .. }
                | Request::Yielded { .. }
                | Request::Ended { .. }
                | Request::Complete { .. }
                | Request::Cancel { .. }) => panic!("expected a batch, not {other:?}"),
            }
        }
        runs
    }

    /// Opens a session for `opener`, returning the session's name, which also
    /// owns its calls, and its first prompt.
    fn open(&mut self, opener: u64) -> (Token, Prompt) {
        self.open_with(opener, spec())
    }

    fn open_with(&mut self, opener: u64, spec: Spec) -> (Token, Prompt) {
        step(&mut self.model, &self.env, Event::Open { opener: Token::new(opener), spec }, &mut self.out);
        let Some(Request::Opened { opener: to, session }) = self.out.pop() else {
            panic!("expected the session to open");
        };
        assert_eq!(to.raw(), opener);
        let (owner, prompt) = calling(self.one());
        assert_eq!(owner, session, "a session's calls are its own");
        (session, prompt)
    }

    /// Opens a session for opener 1, and has the LLM finish its turn.
    fn yielded(&mut self) -> Token {
        self.yielded_with(spec(), done())
    }

    /// Opens a session for `spec` for opener 1, and has the LLM yield with
    /// `answer`.
    fn yielded_with(&mut self, spec: Spec, answer: Completion) -> Token {
        let (session, _) = self.open_with(1, spec);
        drop(yielded(self.step(Event::Completed { owner: session, completion: answer })));
        session
    }

    /// Drains the facts told so far, checking that they are `expected`.
    fn told(&mut self, expected: &[Fact]) {
        for fact in expected {
            assert_eq!(self.model.pop_fact().as_ref(), Some(fact));
        }
        assert_eq!(self.model.pop_fact(), None, "nothing more was told");
    }

    fn after(&mut self, span: Duration) {
        self.env.now = self.env.now.checked_add(span).expect("the test stays in range");
    }
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn spec() -> Spec {
    Spec {
        endpoint: Endpoint(0),
        model: bytes(b"model"),
        system: bytes(b"be brief"),
        authority: authority(1),
        delegated: Box::new([FINISH]),
        prompt: bytes(b"fix the bug"),
        max_tokens: 1024,
        budget: BUDGET,
    }
}

/// The root of the checkout's one repository, as io names it.
const ROOT: Token = Token::new(70);

/// What a spec's kit may do: inspect and modify, in `repos` repositories
/// mounted side by side under `/work`, the first at `/work` itself.
fn authority(repos: u8) -> Authority {
    let mut mounted = List::with_capacity(u32::from(repos));
    for repo in 0..repos {
        let mount: Box<[Name]> = match repo.checked_sub(1) {
            None => Box::new([named(b"work")]),
            Some(other) => Box::new([named(b"other"), named(&[b'a'.checked_add(other).expect("few")])]),
        };
        let root = Token::new(ROOT.raw().checked_add(u64::from(repo)).expect("few"));
        mounted.push(Repo { mount, root, writable: true }).expect("room for each");
    }
    Authority {
        cwd: Box::new([named(b"work")]),
        repos: mounted.into_boxed(),
        grants: Grants { inspect: true, modify: true, shell: false },
        env: Box::new([]),
    }
}

fn named(text: &[u8]) -> Name {
    Name::new(bytes(text)).expect("a test name is a name")
}

/// Where an operation loads, for a read.
fn loading(op: &Op) -> Place {
    let Op::Load { at, max: _ } = op else {
        panic!("expected a load, not {op:?}");
    };
    at.clone()
}

/// Where the tools find the file `name` of the repository.
fn place(name: &[u8]) -> Place {
    Place { root: ROOT, path: bytes(name) }
}

fn budget(budget: Budget) -> Spec {
    Spec { budget, ..spec() }
}

fn text(text: &[u8]) -> Block {
    Block::Text { text: bytes(text) }
}

/// A relative path of one name.
fn path(name: &[u8]) -> Path {
    Path { absolute: false, parts: Box::new([Part::Name { name: named(name) }]) }
}

/// A path outside the checkout.
fn outside() -> Path {
    Path { absolute: true, parts: Box::new([Part::Name { name: named(b"etc") }]) }
}

/// Lists the working directory.
fn list() -> Call {
    Call::List { path: Path { absolute: false, parts: Box::new([Part::Current]) } }
}

/// Reads `name`.
fn cat(name: &[u8]) -> Call {
    Call::Read { path: path(name), skip: 0, lines: None }
}

/// The LLM's call `id` of `call`, as the protocol layer decoded it.
fn tool_call(id: &[u8], call: Call) -> Block {
    Block::ToolCall { id: bytes(id), name: bytes(b"tool"), input: bytes(b"{}"), call: Decoded::Owned { call } }
}

/// The LLM's call `id`, which the protocol layer could not decode.
fn invalid(id: &[u8], problem: Problem) -> Block {
    Block::ToolCall { id: bytes(id), name: bytes(b"tool"), input: bytes(b"{"), call: Decoded::Invalid { problem } }
}

/// Writes `name`.
fn write(name: &[u8]) -> Call {
    Call::Write { path: path(name), content: bytes(b"x") }
}

/// The tool the opener serves in every spec: a finish, which writes.
const FINISH: Descriptor = Descriptor { ticket: Token::new(7), effect: Effect::Write };

/// The LLM's call `id` to a tool the opener serves, kept under `ticket`.
fn delegated(id: &[u8], ticket: u64, effect: Effect) -> Block {
    let call = Decoded::Delegated { ticket: Token::new(ticket), effect };
    Block::ToolCall { id: bytes(id), name: bytes(b"finish"), input: bytes(b"{}"), call }
}

/// The opener's answer, kept under `ticket`.
fn answer(ticket: u64, bytes: u64, error: bool) -> Answer {
    Answer { ticket: Token::new(ticket), bytes, error }
}

/// A read that found `content`.
fn read(content: &[u8]) -> Outcome {
    Outcome::Read { content: bytes(content), skipped: 0, lines: 1, total: 1, cut: false }
}

/// io loaded `content` for the operation of `owner`.
fn loaded(content: &[u8]) -> Done {
    Done::Loaded { content: bytes(content), version: Version::new([1, 0, 0, 0]) }
}

/// io loaded `content` for the operation of `owner`, the tools' read of a file.
fn ran(owner: Token, content: &[u8]) -> Event {
    Event::Done { owner, done: loaded(content) }
}

/// A fact the tools told, as the session passes it on.
fn by_tools(fact: tools::Fact) -> Fact {
    Fact::Tools { fact }
}

fn result(id: &[u8], outcome: Outcome) -> Block {
    Block::ToolResult { id: bytes(id), result: Returned::Owned { outcome } }
}

/// What every completion uses, unless a test says otherwise.
const USAGE: Usage = Usage { input_tokens: 10, output_tokens: 5, cache_read_tokens: 3, cache_write_tokens: 2 };

fn completion(content: Box<[Block]>, stop: Stop) -> Completion {
    Completion { content, stop, usage: USAGE }
}

fn done() -> Completion {
    completion(Box::new([text(b"done")]), Stop::EndTurn)
}

/// The LLM reads `main.rs`.
fn reading() -> Completion {
    completion(Box::new([tool_call(b"c1", cat(b"main.rs"))]), Stop::ToolUse)
}

fn calling(request: Option<Request>) -> (Token, Prompt) {
    let Some(Request::Complete { owner, prompt, timeout }) = request else {
        panic!("expected a call, not {request:?}");
    };
    assert_eq!(timeout, LIMITS.call_timeout);
    (owner, prompt)
}

/// The operation the tools ask of io for a call, named by its owner.
fn running(request: Option<Request>) -> (Token, Op) {
    let Some(Request::Io { owner, op, deadline: _ }) = request else {
        panic!("expected an operation for the tools, not {request:?}");
    };
    (owner, op)
}

fn yielded(request: Option<Request>) -> (u64, Yield, Box<[u8]>) {
    let Some(Request::Yielded { opener, stop, text }) = request else {
        panic!("expected a yield, not {request:?}");
    };
    (opener.raw(), stop, text)
}

/// The end of opener 1's session, after `turns` completions that each used
/// `USAGE`.
fn ended(end: End, turns: u32) -> Request {
    let mut usage = Usage::ZERO;
    for _ in 0..turns {
        usage = usage.saturating_add(USAGE);
    }
    Request::Ended { opener: Token::new(1), end, turns, usage }
}

#[test]
fn a_session_runs_the_tools_it_is_asked_for_and_yields_once_the_llm_is_done() {
    let mut h = Harness::new(LIMITS);
    let (owner, prompt) = h.open(1);
    assert_eq!(prompt.messages.len(), 1);
    assert_eq!(prompt.tools, spec().authority.grants);

    let content =
        Box::new([text(b"let me look"), tool_call(b"c1", cat(b"main.rs")), tool_call(b"c2", cat(b"gone.rs"))]);
    // Two reads: they run together, each the tools' load of its file.
    step(&mut h.model, &h.env, Event::Completed { owner, completion: completion(content, Stop::ToolUse) }, &mut h.out);
    h.out.pop().expect("the completion's usage");
    let (first, op) = running(h.out.pop());
    assert_eq!(loading(&op), place(b"main.rs"));
    let (second, op) = running(h.one());
    assert_eq!(loading(&op), place(b"gone.rs"));
    assert_ne!(first, second, "each operation has a token of its own");
    // The second ends first: the results still go back in call order.
    assert_eq!(h.step(Event::Done { owner: second, done: Done::Missing }), None);
    let (_, prompt) = calling(h.step(ran(first, b"main.rs")));
    assert_eq!(prompt.messages.len(), 3);
    let results = [result(b"c1", read(b"main.rs")), result(b"c2", Outcome::NotFound)];
    assert_eq!(&*prompt.messages[2].content, &results);

    let done = completion(Box::new([text(b"fixed "), text(b"it")]), Stop::EndTurn);
    assert_eq!(yielded(h.step(Event::Completed { owner, completion: done })), (1, Yield::Done, bytes(b"fixed it")));
    let expiry = Time::ZERO.saturating_add(BUDGET.time);
    assert_eq!(h.model.next_deadline(), Some(expiry), "a yielded session still expires");

    // Its kit closes with it, at once, as it runs nothing.
    assert_eq!(h.step(Event::Close { session: owner }), Some(ended(End::Closed, 2)));
    assert_eq!(h.model.next_deadline(), None);
    assert_eq!((h.model.sessions(), h.model.kits()), (1, 1));
    h.model.reclaim();
    assert_eq!((h.model.sessions(), h.model.kits(), h.model.runs()), (0, 0, 0));
}

#[test]
fn a_yielded_session_goes_on_with_the_openers_message() {
    let mut h = Harness::new(LIMITS);
    let session = h.yielded();
    let (owner, prompt) = calling(h.step(Event::Continue { session, content: bytes(b"you have not finished") }));
    assert_eq!(owner, session);
    let messages = [
        Message { role: Role::User, content: Box::new([text(b"fix the bug")]) },
        Message { role: Role::Assistant, content: Box::new([text(b"done")]) },
        Message { role: Role::User, content: Box::new([text(b"you have not finished")]) },
    ];
    assert_eq!(&*prompt.messages, &messages);

    let again = completion(Box::new([text(b"finished")]), Stop::EndTurn);
    assert_eq!(yielded(h.step(Event::Completed { owner, completion: again })), (1, Yield::Done, bytes(b"finished")));
}

/// Opens a session whose LLM answers `content` with `stop`, and returns how it
/// yielded.
fn yield_on(stop: Stop, content: Box<[Block]>) -> (u64, Yield, Box<[u8]>) {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    yielded(h.step(Event::Completed { owner, completion: completion(content, stop) }))
}

#[test]
fn an_invalid_call_is_answered_with_its_problem_and_nothing_runs_for_it() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let missing = Problem::Missing { field: bytes(b"path") };
    let content = Box::new([
        invalid(b"c1", Problem::NotAnObject),
        tool_call(b"c2", cat(b"main.rs")),
        invalid(b"c3", missing.clone()),
    ]);
    let (run, op) = running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    assert_eq!(loading(&op), place(b"main.rs"));
    let (_, prompt) = calling(h.step(ran(run, b"main.rs")));
    let results = [
        Block::ToolResult { id: bytes(b"c1"), result: Returned::Invalid { problem: Problem::NotAnObject } },
        result(b"c2", read(b"main.rs")),
        Block::ToolResult { id: bytes(b"c3"), result: Returned::Invalid { problem: missing } },
    ];
    assert_eq!(&*prompt.messages[2].content, &results, "every call gets its result, in call order");

    let opener = Token::new(1);
    let started = Fact::CompletionStarted { opener, attempt: 0, messages: 1, max_tokens: 1024 };
    let kit = Fact::Tools { fact: tools::Fact::Opened { session: owner } };
    let opening = [h.model.pop_fact(), h.model.pop_fact(), h.model.pop_fact()];
    assert_eq!(opening, [Some(Fact::Opened { opener }), Some(started), Some(kit)]);
    let answered = Fact::CompletionAnswered { opener, stop: Stop::ToolUse, blocks: 3, calls: 3, invalid: 2 };
    assert_eq!(h.model.pop_fact(), Some(answered));
}

#[test]
fn a_message_of_invalid_calls_goes_straight_back() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([invalid(b"c1", Problem::UnknownTool)]);
    let (_, prompt) = calling(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    let answer = Block::ToolResult { id: bytes(b"c1"), result: Returned::Invalid { problem: Problem::UnknownTool } };
    assert_eq!(&*prompt.messages[2].content, &[answer]);
}

#[test]
fn a_call_too_large_for_the_agent_is_answered_so_like_any_invalid_call() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([invalid(b"c1", Problem::TooLarge)]);
    let (_, prompt) = calling(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    let answer = Block::ToolResult { id: bytes(b"c1"), result: Returned::Invalid { problem: Problem::TooLarge } };
    assert_eq!(&*prompt.messages[2].content, &[answer]);
}

#[test]
fn the_calls_a_yield_leaves_are_answered_when_the_opener_continues() {
    let mut h = Harness::new(LIMITS);
    let cut = completion(Box::new([text(b"let me"), invalid(b"c1", Problem::NotAnObject)]), Stop::MaxTokens);
    let session = h.yielded_with(spec(), cut);
    let (_, prompt) = calling(h.step(Event::Continue { session, content: bytes(b"go on") }));
    let last = prompt.messages.last().expect("the opener's message goes last");
    let content = [Block::ToolResult { id: bytes(b"c1"), result: Returned::NotRun }, text(b"go on")];
    assert_eq!((last.role, &*last.content), (Role::User, &content[..]));

    // A turn that ended with a call it did not wait for: the call does not run.
    let mut h = Harness::new(LIMITS);
    let ended = completion(Box::new([tool_call(b"c1", list())]), Stop::EndTurn);
    let session = h.yielded_with(spec(), ended);
    let (_, prompt) = calling(h.step(Event::Continue { session, content: bytes(b"go on") }));
    let last = prompt.messages.last().expect("the opener's message goes last");
    assert_eq!(last.content.first(), Some(&Block::ToolResult { id: bytes(b"c1"), result: Returned::NotRun }));
}

#[test]
fn a_tool_call_carries_its_deadline_and_one_that_runs_out_of_time_goes_back_to_the_llm() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    h.after(Duration::from_secs(1));
    step(&mut h.model, &h.env, Event::Completed { owner, completion: reading() }, &mut h.out);
    drop(h.out.pop());
    let Some(Request::Io { owner: run, op: _, deadline }) = h.one() else {
        panic!("expected an operation for the tools");
    };
    assert_eq!(deadline, h.env.now.saturating_add(LIMITS.tool_timeout), "the call's deadline bounds its operation");
    // Whoever runs the call runs the race: the session arms nothing for it.
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(BUDGET.time)));
    h.env.now = deadline;
    let (_, prompt) = calling(h.step(Event::Done { owner: run, done: Done::TimedOut }));
    assert_eq!(&*prompt.messages[2].content, &[result(b"c1", Outcome::TimedOut)]);
}

#[test]
fn adjacent_reads_run_together_and_a_write_runs_alone_and_results_go_back_in_order() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([
        tool_call(b"a", cat(b"a")),
        tool_call(b"b", cat(b"b")),
        tool_call(b"c", cat(b"c")),
        tool_call(b"d", write(b"d")),
        tool_call(b"e", cat(b"e")),
    ]);
    // Two reads at a time, as the limits allow.
    let first = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    let &[run_a, run_b] = first.as_slice() else { panic!("two reads, not {first:?}") };
    assert_eq!(h.step(ran(run_b, b"b")), None, "the batch waits for all its runs");
    // The read before the write, alone; the write; the read after it.
    let second = h.batch(ran(run_a, b"a"));
    let &[run_c] = second.as_slice() else { panic!("one read, not {second:?}") };
    let third = h.batch(ran(run_c, b"c"));
    let &[run_d] = third.as_slice() else { panic!("one write, not {third:?}") };
    let fourth = h.batch(Event::Done { owner: run_d, done: Done::Stored { version: Version::new([2, 0, 0, 0]) } });
    let &[run_e] = fourth.as_slice() else { panic!("one read, not {fourth:?}") };
    let (_, prompt) = calling(h.step(ran(run_e, b"e")));
    let results = [
        result(b"a", read(b"a")),
        result(b"b", read(b"b")),
        result(b"c", read(b"c")),
        result(b"d", Outcome::Written { created: true }),
        result(b"e", read(b"e")),
    ];
    assert_eq!(&*prompt.messages[2].content, &results);
}

#[test]
fn a_batch_the_tools_answer_at_once_goes_on_from_the_ready_list_after_the_reclaim_point() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    // Outside the checkout: the tools answer these at their entrance.
    let away = Call::Read { path: outside(), skip: 0, lines: None };
    let over = Call::Write { path: outside(), content: bytes(b"x") };
    let content = Box::new([tool_call(b"c1", away), tool_call(b"c2", over), tool_call(b"c3", cat(b"main.rs"))]);
    // The first batch is answered within the step that started it: the
    // session rests, and is ready once the iteration's reclaim point passed.
    assert_eq!(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }), None);
    assert!(!h.model.is_ready(), "not before the reclaim point");
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(BUDGET.time)), "no alarm but expiry");
    h.model.reclaim();
    assert_eq!(h.model.runs(), 0, "the runs answered at once are reclaimed before the next batch");
    assert_eq!(h.resume(), None, "the write is answered at once too");
    assert!(!h.model.is_ready(), "it rests again until the next reclaim point");
    h.model.reclaim();
    let (load, op) = running(h.resume());
    assert!(!h.model.is_ready());
    assert_eq!(loading(&op), place(b"main.rs"));
    let (_, prompt) = calling(h.step(ran(load, b"main.rs")));
    let results = [result(b"c1", Outcome::Outside), result(b"c2", Outcome::Outside), result(b"c3", read(b"main.rs"))];
    assert_eq!(&*prompt.messages[2].content, &results);
}

#[test]
fn a_session_closed_while_it_rests_leaves_the_ready_list() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let away = Call::Read { path: outside(), skip: 0, lines: None };
    let over = Call::Write { path: outside(), content: bytes(b"x") };
    let content = Box::new([tool_call(b"c1", away), tool_call(b"c2", over)]);
    assert_eq!(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }), None);
    assert_eq!(h.step(Event::Close { session: owner }), Some(ended(End::Closed, 1)));
    h.model.reclaim();
    assert!(!h.model.is_ready(), "a closed session is not resumed");
}

#[test]
fn a_batch_as_wide_as_the_most_a_step_emits_fits() {
    let limits = Limits { parallel_tools: MAX_PARALLEL, ..LIMITS };
    let mut h = Harness::new(limits);
    let (owner, _) = h.open(1);
    let mut content = List::with_capacity(MAX_PARALLEL);
    for _ in 0..MAX_PARALLEL {
        content.push(tool_call(b"c", list())).expect("room for every call");
    }
    let completion = completion(content.into_boxed(), Stop::ToolUse);
    step(&mut h.model, &h.env, Event::Completed { owner, completion }, &mut h.out);
    assert_eq!(h.out.len(), 1 + MAX_PARALLEL, "the usage, and an operation for each call");
    assert!(h.out.len() <= max_out(&limits), "within what a step may emit");
    assert_eq!(worst_case(&Limits { parallel_tools: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { parallel_tools: MAX_PARALLEL + 1, ..LIMITS }), None);
    // A batch the tools could not take whole, and a session without a kit.
    assert_eq!(worst_case(&Limits { tools: tools::Limits { calls: 1, ..TOOLS }, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { tools: tools::Limits { kits: 1, ..TOOLS }, ..LIMITS }), None);
}

#[test]
fn closing_a_session_with_a_batch_in_flight_closes_its_kit_and_waits_for_every_run() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"a", cat(b"a")), tool_call(b"b", cat(b"b"))]);
    let runs = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    // The kit's close cancels the operation of each call it runs.
    let cancels = h.batch(Event::Close { session: owner });
    assert_eq!(cancels.as_slice(), runs.as_slice());
    let &[a, b] = runs.as_slice() else { panic!("two reads, not {runs:?}") };
    assert_eq!(h.step(Event::Done { owner: a, done: Done::Cancelled }), None);
    // The other won its race; with it, the kit has closed.
    assert_eq!(h.step(ran(b, b"late")), Some(ended(End::Closed, 1)));
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn a_result_that_does_not_fit_cancels_the_rest_of_its_batch() {
    let mut h = Harness::new(Limits { session_bytes: 2048, ..LIMITS });
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"a", cat(b"a")), tool_call(b"b", cat(b"b"))]);
    let runs = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    let &[a, b] = runs.as_slice() else { panic!("two reads, not {runs:?}") };
    assert_eq!(h.step(ran(a, &[b'x'; 4096])), Some(Request::CancelIo { owner: b }));
    let end = h.step(Event::Done { owner: b, done: Done::Cancelled });
    assert_eq!(end, Some(ended(End::TranscriptFull, 1)));
}

#[test]
fn a_result_is_charged_its_id_with_its_call_so_a_refusal_at_the_entrance_always_fits() {
    let limits = Limits { session_bytes: 4096, ..LIMITS };
    let away = Call::Read { path: outside(), skip: 0, lines: None };
    // An id the transcript has room for once but not twice: the message that
    // carries it does not fit, and no batch starts to be cancelled.
    let mut h = Harness::new(limits);
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"a", cat(b"main.rs")), tool_call(&[b'i'; 2100], away.clone())]);
    let end = h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    assert_eq!(end, Some(ended(End::TranscriptFull, 1)));
    // Room for it twice: the refusal at the entrance fits, and its batch goes
    // on.
    let mut h = Harness::new(limits);
    let (owner, _) = h.open(1);
    let id = [b'i'; 1500];
    let content = Box::new([tool_call(b"a", cat(b"main.rs")), tool_call(&id, away)]);
    let runs = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    let &[a] = runs.as_slice() else { panic!("the read alone runs, not {runs:?}") };
    let (_, prompt) = calling(h.step(ran(a, b"main.rs")));
    assert_eq!(&*prompt.messages[2].content, &[result(b"a", read(b"main.rs")), result(&id, Outcome::Outside)]);
}

#[test]
fn the_tools_tell_of_each_call_and_the_session_passes_it_on() {
    let mut h = Harness::new(LIMITS);
    let opener = Token::new(1);
    let (owner, _) = h.open(1);
    let content = Box::new([text(b"two"), tool_call(b"a", cat(b"a")), tool_call(b"b", cat(b"b"))]);
    let runs = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    assert_eq!(h.step(ran(runs.as_slice()[1], b"b")), None);
    let read = tools::Tool::Read;
    h.told(&[
        Fact::Opened { opener },
        Fact::CompletionStarted { opener, attempt: 0, messages: 1, max_tokens: 1024 },
        by_tools(tools::Fact::Opened { session: owner }),
        Fact::CompletionAnswered { opener, stop: Stop::ToolUse, blocks: 3, calls: 2, invalid: 0 },
        Fact::Used { opener, usage: USAGE },
        by_tools(tools::Fact::Started { session: owner, tool: read }),
        by_tools(tools::Fact::Started { session: owner, tool: read }),
        by_tools(tools::Fact::Answered { session: owner, tool: read, verdict: tools::Verdict::Read, bytes: 1 }),
    ]);
}

#[test]
fn a_delegated_call_goes_to_the_opener_and_its_answer_goes_back_to_the_llm() {
    let mut h = Harness::new(LIMITS);
    let (owner, prompt) = h.open(1);
    assert_eq!(&*prompt.delegated, &[FINISH], "the prompt offers what the opener serves");
    let content = Box::new([delegated(b"c1", 9, Effect::Write)]);
    step(&mut h.model, &h.env, Event::Completed { owner, completion: completion(content, Stop::ToolUse) }, &mut h.out);
    drop(h.out.pop());
    let Some(Request::Delegate { owner: run, opener, call, deadline }) = h.one() else {
        panic!("expected the call delegated");
    };
    assert_eq!((opener, call), (Token::new(1), Token::new(9)));
    assert_eq!(deadline, h.env.now.saturating_add(BUDGET.time), "the opener races it against the session's time");
    let (_, prompt) = calling(h.step(Event::Answered { owner: run, answer: answer(11, 20, true) }));
    let result = Block::ToolResult { id: bytes(b"c1"), result: Returned::Delegated { answer: answer(11, 20, true) } };
    assert_eq!(&*prompt.messages[2].content, &[result]);
}

#[test]
fn owned_and_delegated_calls_run_in_one_order_of_batches() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([
        tool_call(b"a", cat(b"a")),
        delegated(b"b", 9, Effect::Read),
        delegated(b"c", 10, Effect::Write),
        tool_call(b"d", cat(b"d")),
    ]);
    // A read of the tools and one the opener serves, together.
    step(&mut h.model, &h.env, Event::Completed { owner, completion: completion(content, Stop::ToolUse) }, &mut h.out);
    drop(h.out.pop());
    let Some(Request::Io { owner: read_a, .. }) = h.out.pop() else { panic!("expected the tools' read") };
    let Some(Request::Delegate { owner: read_b, call, .. }) = h.one() else { panic!("expected the opener's read") };
    assert_eq!(call, Token::new(9));
    assert_eq!(h.step(Event::Answered { owner: read_b, answer: answer(20, 3, false) }), None);
    // The write alone, then the last read.
    let Some(Request::Delegate { owner: write_c, call, .. }) = h.step(ran(read_a, b"a")) else {
        panic!("expected the opener's write");
    };
    assert_eq!(call, Token::new(10));
    let (read_d, _) = running(h.step(Event::Answered { owner: write_c, answer: answer(21, 3, false) }));
    let (_, prompt) = calling(h.step(ran(read_d, b"d")));
    let results = [
        result(b"a", read(b"a")),
        Block::ToolResult { id: bytes(b"b"), result: Returned::Delegated { answer: answer(20, 3, false) } },
        Block::ToolResult { id: bytes(b"c"), result: Returned::Delegated { answer: answer(21, 3, false) } },
        result(b"d", read(b"d")),
    ];
    assert_eq!(&*prompt.messages[2].content, &results);
}

#[test]
fn closing_withdraws_the_delegated_calls_in_flight_and_waits_for_their_terminals() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"a", cat(b"a")), delegated(b"b", 9, Effect::Read)]);
    let runs = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    let &[read_a, read_b] = runs.as_slice() else { panic!("two runs, not {runs:?}") };
    // The withdraw, then the kit's close, which cancels the tools' read.
    step(&mut h.model, &h.env, Event::Close { session: owner }, &mut h.out);
    assert_eq!(h.out.pop(), Some(Request::Withdraw { owner: read_b }));
    assert_eq!(h.out.pop(), Some(Request::CancelIo { owner: read_a }));
    // The answer won its race with the withdraw; the session still waits.
    assert_eq!(h.step(Event::Answered { owner: read_b, answer: answer(20, 3, false) }), None);
    let end = h.step(Event::Done { owner: read_a, done: Done::Cancelled });
    assert_eq!(end, Some(ended(End::Closed, 1)));

    // And as the session runs out of time.
    let (owner, _) = h.open(1);
    let content = Box::new([delegated(b"b", 9, Effect::Read)]);
    let runs = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    h.after(BUDGET.time);
    assert_eq!(h.fire(), Some(Request::Withdraw { owner: runs.as_slice()[0] }));
    assert_eq!(h.step(Event::AnswerCancelled { owner: runs.as_slice()[0] }), Some(ended(OUT_OF_TIME, 1)));
}

#[test]
fn an_answer_counts_against_the_byte_limit() {
    let mut h = Harness::new(Limits { session_bytes: 2048, ..LIMITS });
    let (owner, _) = h.open(1);
    let content = Box::new([delegated(b"b", 9, Effect::Read), tool_call(b"a", cat(b"a"))]);
    let runs = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    let &[read_b, read_a] = runs.as_slice() else { panic!("two runs, not {runs:?}") };
    let end = h.step(Event::Answered { owner: read_b, answer: answer(20, 4096, false) });
    assert_eq!(end, Some(Request::CancelIo { owner: read_a }));
    let end = h.step(Event::Done { owner: read_a, done: Done::Cancelled });
    assert_eq!(end, Some(ended(End::TranscriptFull, 1)));
}

#[test]
fn the_turn_that_crosses_a_budget_still_has_its_delegated_calls_answered() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open_with(1, budget(Budget { input: 9, ..BUDGET }));
    let content = Box::new([delegated(b"f", 9, Effect::Write)]);
    let runs = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    let end = h.step(Event::Answered { owner: runs.as_slice()[0], answer: answer(20, 3, false) });
    assert_eq!(end, Some(ended(End::Budget { spent: Dimension::Input }, 1)));
}

#[test]
fn delegated_calls_are_told_as_they_start_and_end() {
    let mut h = Harness::new(LIMITS);
    let opener = Token::new(1);
    let (owner, _) = h.open(1);
    let content = Box::new([delegated(b"a", 9, Effect::Read), delegated(b"b", 10, Effect::Read)]);
    let runs = h.batch(Event::Completed { owner, completion: completion(content, Stop::ToolUse) });
    assert_eq!(h.step(Event::Answered { owner: runs.as_slice()[0], answer: answer(20, 3, true) }), None);
    drop(h.batch(Event::Close { session: owner }));
    drop(h.step(Event::AnswerCancelled { owner: runs.as_slice()[1] }));
    h.told(&[
        Fact::Opened { opener },
        Fact::CompletionStarted { opener, attempt: 0, messages: 1, max_tokens: 1024 },
        Fact::Tools { fact: tools::Fact::Opened { session: owner } },
        Fact::CompletionAnswered { opener, stop: Stop::ToolUse, blocks: 2, calls: 2, invalid: 0 },
        Fact::Used { opener, usage: USAGE },
        Fact::DelegateStarted { opener, block: 0 },
        Fact::DelegateStarted { opener, block: 1 },
        Fact::DelegateAnswered { opener, bytes: 3, error: true },
        // The kit closes as the session does, with nothing of its own to settle.
        Fact::Tools { fact: tools::Fact::Closed { session: owner } },
        Fact::DelegateCancelled { opener },
        Fact::Ended { opener, end: End::Closed, turns: 1, usage: USAGE },
    ]);
}

#[test]
fn the_llm_yields_whenever_it_stops_calling_tools() {
    let truncated = yield_on(Stop::MaxTokens, Box::new([text(b"I was say")]));
    assert_eq!(truncated, (1, Yield::Truncated, bytes(b"I was say")));
    assert_eq!(yield_on(Stop::Refusal, Box::new([text(b"no")])), (1, Yield::Refused, bytes(b"no")));
    let malformed = yield_on(Stop::ToolUse, Box::new([text(b"I will call a tool")]));
    assert_eq!(malformed, (1, Yield::Malformed, bytes(b"I will call a tool")));
    assert_eq!(yield_on(Stop::EndTurn, Box::new([])), (1, Yield::Done, bytes(b"")));
}

#[test]
fn opens_beyond_the_session_slots_are_refused_as_busy() {
    let mut h = Harness::new(Limits { sessions: 1, ..LIMITS });
    drop(h.open(1));
    let refused = h.step(Event::Open { opener: Token::new(2), spec: spec() });
    assert_eq!(refused, Some(Request::Ended { opener: Token::new(2), end: End::Busy, turns: 0, usage: Usage::ZERO }));
}

#[test]
fn specs_beyond_the_limits_are_refused_as_invalid() {
    for limits in [Limits { session_bytes: 16, ..LIMITS }, Limits { max_tokens: 1, ..LIMITS }] {
        let mut h = Harness::new(limits);
        let refused = h.step(Event::Open { opener: Token::new(1), spec: spec() });
        assert_eq!(refused, Some(ended(End::Invalid, 0)));
        assert_eq!(h.model.sessions(), 0);
    }
}

#[test]
fn a_spec_whose_authority_the_tools_refuse_is_refused_as_invalid() {
    // More repositories than the tools' limits let a kit name.
    let mut h = Harness::new(LIMITS);
    let refused = h.step(Event::Open { opener: Token::new(1), spec: Spec { authority: authority(3), ..spec() } });
    assert_eq!(refused, Some(ended(End::Invalid, 0)));
    h.model.reclaim();
    assert_eq!((h.model.sessions(), h.model.kits()), (0, 0));
}

#[test]
fn transient_failures_are_retried_after_a_backoff_until_the_retries_run_out() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    for _ in 0..LIMITS.retries {
        assert_eq!(h.step(Event::Failed { owner, failure: Failure::Overloaded }), None);
        let retry = h.model.next_deadline().expect("a retry is armed");
        assert!(retry <= h.env.now.saturating_add(LIMITS.backoff_max), "the backoff is capped");
        h.env.now = retry;
        drop(calling(h.fire()));
    }
    let end = h.step(Event::Failed { owner, failure: Failure::Overloaded });
    assert_eq!(end, Some(ended(End::Failed { failure: Failure::Overloaded }, 0)));
}

#[test]
fn a_rate_limit_is_waited_out_and_lasting_failures_are_not_retried() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(2);
    let failure = Failure::RateLimited { retry_after: Duration::from_secs(5) };
    assert_eq!(h.step(Event::Failed { owner, failure }), None);
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(5))));

    let (owner, _) = h.open(1);
    let end = h.step(Event::Failed { owner, failure: Failure::Unauthorized });
    assert_eq!(end, Some(ended(End::Failed { failure: Failure::Unauthorized }, 0)));
}

#[test]
fn closing_a_calling_session_cancels_its_call_and_ends_once_the_call_has_ended() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::Cancel { owner }));
    assert_eq!(h.model.next_deadline(), None, "a closing session waits for nothing but its call");
    assert_eq!(h.step(Event::Cancelled { owner }), Some(ended(End::Closed, 0)));
}

#[test]
fn a_completion_that_wins_the_race_with_a_close_still_ends_the_session() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::Cancel { owner }));
    // Its tokens were spent all the same.
    assert_eq!(h.step(Event::Completed { owner, completion: reading() }), Some(ended(End::Closed, 1)));
    assert_eq!((h.turns, h.usage), (1, USAGE));

    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::Cancel { owner }));
    assert_eq!(h.step(Event::Failed { owner, failure: Failure::Overloaded }), Some(ended(End::Closed, 0)));
}

#[test]
fn closing_a_tooling_session_cancels_its_tool() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"c1", cat(b"main.rs"))]);
    let (run, _) = running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::CancelIo { owner: run }));
    assert_eq!(h.step(Event::Done { owner: run, done: Done::Cancelled }), Some(ended(End::Closed, 1)));

    // The tool won the race.
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"c1", cat(b"main.rs"))]);
    let (run, _) = running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::CancelIo { owner: run }));
    assert_eq!(h.step(ran(run, b"main.rs")), Some(ended(End::Closed, 1)));
}

#[test]
fn closing_a_session_with_nothing_in_flight_ends_it_at_once() {
    let mut h = Harness::new(LIMITS);
    let session = h.yielded();
    assert_eq!(h.step(Event::Close { session }), Some(ended(End::Closed, 1)));
    assert_eq!(h.model.next_deadline(), None);

    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Failed { owner, failure: Failure::Unavailable }), None);
    assert_eq!(h.step(Event::Close { session: owner }), Some(ended(End::Closed, 0)));
    assert_eq!(h.model.next_deadline(), None, "the retry is called off");
}

#[test]
fn closing_a_session_that_is_already_closing_changes_nothing() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    h.after(BUDGET.time);
    assert_eq!(h.fire(), Some(Request::Cancel { owner }));
    assert_eq!(h.step(Event::Close { session: owner }), None);
    assert_eq!(h.step(Event::Cancelled { owner }), Some(ended(OUT_OF_TIME, 0)));
}

#[test]
fn a_handle_to_a_session_that_has_ended_is_dropped() {
    let mut h = Harness::new(Limits { sessions: 1, ..LIMITS });
    let session = h.yielded();
    assert_eq!(h.step(Event::Close { session }), Some(ended(End::Closed, 1)));
    // Ended, and not yet reclaimed.
    assert_eq!(h.step(Event::Continue { session, content: bytes(b"go on") }), None);
    assert_eq!(h.step(Event::Close { session }), None);
    // Reclaimed, and its slot taken by another session.
    h.model.reclaim();
    let (other, _) = h.open(2);
    assert_ne!(other, session, "a handle is never reused");
    assert_eq!(h.step(Event::Close { session }), None);
    assert_eq!(h.step(Event::Continue { session, content: bytes(b"go on") }), None);
    assert_eq!(h.model.sessions(), 1, "the other session lives on");
}

#[test]
fn an_expiring_session_cancels_its_call_and_ends_once_the_call_has_ended() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    h.after(BUDGET.time);
    assert_eq!(h.fire(), Some(Request::Cancel { owner }));
    assert_eq!(h.model.next_deadline(), None);
    // The completion won the race with the cancel; the session still expires.
    assert_eq!(h.step(Event::Completed { owner, completion: done() }), Some(ended(OUT_OF_TIME, 1)));
}

#[test]
fn an_expiring_session_cancels_its_tool() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"c1", cat(b"main.rs"))]);
    let (run, _) = running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    h.after(BUDGET.time);
    assert_eq!(h.fire(), Some(Request::CancelIo { owner: run }));
    assert_eq!(h.step(Event::Done { owner: run, done: Done::Cancelled }), Some(ended(OUT_OF_TIME, 1)));
}

#[test]
fn an_expiring_session_in_backoff_or_yielded_ends_at_once() {
    let mut h = Harness::new(Limits {
        backoff_base: Duration::from_secs(3600),
        backoff_max: Duration::from_secs(3600),
        ..LIMITS
    });
    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Failed { owner, failure: Failure::Unavailable }), None);
    h.after(BUDGET.time);
    assert_eq!(h.fire(), Some(ended(OUT_OF_TIME, 0)));
    assert_eq!(h.model.next_deadline(), None);

    let mut h = Harness::new(LIMITS);
    let _session: Token = h.yielded();
    h.after(BUDGET.time);
    assert_eq!(h.fire(), Some(ended(OUT_OF_TIME, 1)));
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn every_completion_is_reported_before_what_follows_it() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open(1);
    step(&mut h.model, &h.env, Event::Completed { owner, completion: reading() }, &mut h.out);
    assert_eq!(h.out.pop(), Some(Request::Used { opener: Token::new(1), usage: USAGE }));
    let (run, _) = running(h.one());
    drop(calling(h.step(ran(run, b"main.rs"))));
    drop(yielded(h.step(Event::Completed { owner, completion: done() })));
    // The end adds up both completions; the harness tallied the second.
    assert_eq!(h.step(Event::Close { session: owner }), Some(ended(End::Closed, 2)));
    assert_eq!((h.turns, h.usage), (1, USAGE));
}

#[test]
fn a_spec_whose_budget_is_beyond_the_limits_is_refused() {
    let over = Duration::from_nanos(BUDGET.time.as_nanos() + 1);
    let budgets = [
        Budget { turns: BUDGET.turns + 1, ..BUDGET },
        Budget { input: BUDGET.input + 1, ..BUDGET },
        Budget { output: BUDGET.output + 1, ..BUDGET },
        Budget { cache_read: BUDGET.cache_read + 1, ..BUDGET },
        Budget { cache_write: BUDGET.cache_write + 1, ..BUDGET },
        Budget { time: over, ..BUDGET },
    ];
    for asked in budgets {
        let mut h = Harness::new(LIMITS);
        let refused = h.step(Event::Open { opener: Token::new(1), spec: budget(asked) });
        assert_eq!(refused, Some(ended(End::Invalid, 0)), "{asked:?}");
    }
}

#[test]
fn a_session_whose_budget_leaves_no_room_for_a_completion_ends_as_it_opens() {
    let cases = [
        (Budget { turns: 0, ..BUDGET }, Dimension::Turns),
        (Budget { input: 0, ..BUDGET }, Dimension::Input),
        (Budget { output: 0, ..BUDGET }, Dimension::Output),
        (Budget { time: Duration::ZERO, ..BUDGET }, Dimension::Time),
    ];
    for (asked, spent) in cases {
        let mut h = Harness::new(LIMITS);
        step(&mut h.model, &h.env, Event::Open { opener: Token::new(1), spec: budget(asked) }, &mut h.out);
        let Some(Request::Opened { .. }) = h.out.pop() else {
            panic!("{asked:?}: the session opens");
        };
        assert_eq!(h.one(), Some(ended(End::Budget { spent }, 0)), "{asked:?}");
    }
}

#[test]
fn the_llm_gets_as_many_turns_as_the_budget_allows() {
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open_with(1, budget(Budget { turns: 1, ..BUDGET }));
    // The tools of the last turn still run; their results do not go back.
    let (run, _) = running(h.step(Event::Completed { owner, completion: reading() }));
    let end = h.step(ran(run, b"main.rs"));
    assert_eq!(end, Some(ended(End::Budget { spent: Dimension::Turns }, 1)));

    h.model.reclaim();
    let session = h.yielded_with(budget(Budget { turns: 1, ..BUDGET }), done());
    let end = h.step(Event::Continue { session, content: bytes(b"go on") });
    assert_eq!(end, Some(ended(End::Budget { spent: Dimension::Turns }, 1)));
}

#[test]
fn a_session_starts_no_completion_once_its_input_or_output_reach_their_budget() {
    let cases =
        [(Budget { input: 10, ..BUDGET }, Dimension::Input), (Budget { output: 5, ..BUDGET }, Dimension::Output)];
    for (asked, spent) in cases {
        let mut h = Harness::new(LIMITS);
        let session = h.yielded_with(budget(asked), done());
        let end = h.step(Event::Continue { session, content: bytes(b"go on") });
        assert_eq!(end, Some(ended(End::Budget { spent }, 1)), "{asked:?}");
    }
    // Cache tokens that reach their budget, and go no further, stop nothing.
    let mut h = Harness::new(LIMITS);
    let session =
        h.yielded_with(budget(Budget { input: 11, output: 6, cache_read: 3, cache_write: 2, ..BUDGET }), done());
    drop(calling(h.step(Event::Continue { session, content: bytes(b"go on") })));
}

#[test]
fn the_turn_that_takes_tokens_past_their_budget_runs_its_tools_and_ends_the_session() {
    let cases = [
        (Budget { input: 9, ..BUDGET }, Dimension::Input),
        (Budget { cache_read: 2, ..BUDGET }, Dimension::CacheRead),
        (Budget { cache_write: 1, ..BUDGET }, Dimension::CacheWrite),
        // A zero cache budget is a budget like any other.
        (Budget { cache_read: 0, ..BUDGET }, Dimension::CacheRead),
    ];
    for (asked, spent) in cases {
        // The tools run, and their results are kept, but do not go back.
        let mut h = Harness::new(LIMITS);
        let (owner, _) = h.open_with(1, budget(asked));
        let (run, _) = running(h.step(Event::Completed { owner, completion: reading() }));
        let end = h.step(ran(run, b"main.rs"));
        assert_eq!(end, Some(ended(End::Budget { spent }, 1)), "{asked:?}");

        // A yield still yields; the next message ends it.
        let mut h = Harness::new(LIMITS);
        let session = h.yielded_with(budget(asked), done());
        let end = h.step(Event::Continue { session, content: bytes(b"go on") });
        assert_eq!(end, Some(ended(End::Budget { spent }, 1)), "{asked:?}");
    }
    // An answer past its output budget, which a provider should not give.
    let mut h = Harness::new(LIMITS);
    let (owner, prompt) = h.open_with(1, budget(Budget { output: 4, ..BUDGET }));
    assert_eq!(prompt.max_tokens, 4);
    let (run, _) = running(h.step(Event::Completed { owner, completion: reading() }));
    let end = h.step(ran(run, b"main.rs"));
    assert_eq!(end, Some(ended(End::Budget { spent: Dimension::Output }, 1)));

    // Without the cache, a zero cache budget stops nothing.
    let mut h = Harness::new(LIMITS);
    let uncached = Completion { usage: Usage { cache_read_tokens: 0, cache_write_tokens: 0, ..USAGE }, ..done() };
    let session = h.yielded_with(budget(Budget { cache_read: 0, cache_write: 0, ..BUDGET }), uncached);
    drop(calling(h.step(Event::Continue { session, content: bytes(b"go on") })));
}

#[test]
fn time_does_not_wait_for_the_turn_that_crossed_a_budget() {
    let time = Duration::from_secs(60);
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open_with(1, budget(Budget { input: 9, time, ..BUDGET }));
    let (run, _) = running(h.step(Event::Completed { owner, completion: reading() }));
    h.after(time);
    assert_eq!(h.fire(), Some(Request::CancelIo { owner: run }));
    assert_eq!(h.step(Event::Done { owner: run, done: Done::Cancelled }), Some(ended(OUT_OF_TIME, 1)));
}

#[test]
fn each_answer_may_take_no_more_than_the_output_budget_left() {
    let mut h = Harness::new(LIMITS);
    let (owner, prompt) = h.open_with(1, budget(Budget { output: 12, ..BUDGET }));
    assert_eq!(prompt.max_tokens, 12);
    let (run, _) = running(h.step(Event::Completed { owner, completion: reading() }));
    let (_, prompt) = calling(h.step(ran(run, b"main.rs")));
    assert_eq!(prompt.max_tokens, 7);
    let usage = Usage { output_tokens: 7, ..USAGE };
    let cut = Completion { usage, ..completion(Box::new([text(b"I was")]), Stop::MaxTokens) };
    let request = h.step(Event::Completed { owner, completion: cut });
    assert_eq!(yielded(request), (1, Yield::Truncated, bytes(b"I was")));
    let end = h.step(Event::Continue { session: owner, content: bytes(b"go on") });
    let spent = End::Budget { spent: Dimension::Output };
    let usage = USAGE.saturating_add(usage);
    assert_eq!(end, Some(Request::Ended { opener: Token::new(1), end: spent, turns: 2, usage }));

    // A spec's own max_tokens stays the cap while more is left.
    let mut h = Harness::new(LIMITS);
    let (_, prompt) = h.open_with(1, Spec { max_tokens: 100, ..budget(Budget { output: 1000, ..BUDGET }) });
    assert_eq!(prompt.max_tokens, 100);
}

#[test]
fn the_time_budget_ends_a_session_whatever_it_is_doing() {
    let time = Duration::from_secs(60);
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open_with(1, budget(Budget { time, ..BUDGET }));
    assert_eq!(h.model.next_deadline(), Some(Time::ZERO.saturating_add(time)));
    h.after(time);
    assert_eq!(h.fire(), Some(Request::Cancel { owner }));
    assert_eq!(h.step(Event::Cancelled { owner }), Some(ended(OUT_OF_TIME, 0)));
}

#[test]
fn a_session_whose_time_is_up_starts_no_completion_even_before_its_alarm_fires() {
    let time = Duration::from_secs(60);
    let mut h = Harness::new(LIMITS);
    let (owner, _) = h.open_with(1, budget(Budget { time, ..BUDGET }));
    let (run, _) = running(h.step(Event::Completed { owner, completion: reading() }));
    // The tool's result and the alarm fall due in the same iteration: the
    // result is handled first, and the session ends there.
    h.after(time);
    let end = h.step(ran(run, b"main.rs"));
    assert_eq!(end, Some(ended(OUT_OF_TIME, 1)));
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn a_conversation_that_outgrows_its_bytes_ends_the_session() {
    let mut h = Harness::new(Limits { session_bytes: 1024, ..LIMITS });
    let (owner, _) = h.open(1);
    let content = Box::new([tool_call(b"c1", cat(b"main.rs"))]);
    let (run, _) = running(h.step(Event::Completed { owner, completion: completion(content, Stop::ToolUse) }));
    let huge = Box::from([b'x'; 2048].as_slice());
    let end = h.step(Event::Done { owner: run, done: loaded(&huge) });
    assert_eq!(end, Some(ended(End::TranscriptFull, 1)));

    let session = h.yielded();
    let huge = Box::from([b'x'; 2048].as_slice());
    assert_eq!(h.step(Event::Continue { session, content: huge }), Some(ended(End::TranscriptFull, 1)));

    h.model.reclaim();
    let (owner, _) = h.open(1);
    let huge = Box::new([Block::Text { text: Box::from([b'x'; 2048].as_slice()) }]);
    let end = h.step(Event::Completed { owner, completion: completion(huge, Stop::EndTurn) });
    assert_eq!(end, Some(ended(End::TranscriptFull, 1)), "a yield the session could not continue from ends it");
}

#[test]
fn a_session_starts_no_completion_whose_answer_it_could_not_keep() {
    // The prompt, the tools' call and their results: no room for an answer.
    let mut h = Harness::new(Limits { messages: 3, ..LIMITS });
    let (owner, _) = h.open(1);
    let (run, _) = running(h.step(Event::Completed { owner, completion: reading() }));
    assert_eq!(h.step(ran(run, b"main.rs")), Some(ended(End::TranscriptFull, 1)));
    // Nor is a transcript too short to hold a prompt and its answer.
    assert_eq!(worst_case(&Limits { messages: 1, ..LIMITS }), None);
}

#[test]
fn a_transcript_with_no_room_for_another_message_ends_the_session() {
    let mut h = Harness::new(Limits { messages: 2, ..LIMITS });
    let session = h.yielded();
    assert_eq!(h.step(Event::Continue { session, content: bytes(b"go on") }), Some(ended(End::TranscriptFull, 1)));
}

#[test]
fn a_session_tells_what_happens_as_facts() {
    let mut h = Harness::new(LIMITS);
    let opener = Token::new(1);
    let (owner, _) = h.open(1);
    h.told(&[
        Fact::Opened { opener },
        Fact::CompletionStarted { opener, attempt: 0, messages: 1, max_tokens: 1024 },
        by_tools(tools::Fact::Opened { session: owner }),
    ]);

    let (run, _) = running(h.step(Event::Completed { owner, completion: reading() }));
    h.told(&[
        Fact::CompletionAnswered { opener, stop: Stop::ToolUse, blocks: 1, calls: 1, invalid: 0 },
        Fact::Used { opener, usage: USAGE },
        by_tools(tools::Fact::Started { session: owner, tool: tools::Tool::Read }),
    ]);
    drop(calling(h.step(ran(run, b"main.rs"))));
    let verdict = tools::Verdict::Read;
    h.told(&[
        Fact::CompletionStarted { opener, attempt: 0, messages: 3, max_tokens: 1024 },
        by_tools(tools::Fact::Answered { session: owner, tool: tools::Tool::Read, verdict, bytes: 7 }),
    ]);
    drop(yielded(h.step(Event::Completed { owner, completion: done() })));
    h.told(&[
        Fact::CompletionAnswered { opener, stop: Stop::EndTurn, blocks: 1, calls: 0, invalid: 0 },
        Fact::Used { opener, usage: USAGE },
        Fact::Yielded { opener, stop: Yield::Done },
    ]);
    let end = h.step(Event::Close { session: owner });
    let Some(Request::Ended { opener: _, end, turns, usage }) = end else {
        panic!("expected the end, not {end:?}");
    };
    h.told(&[Fact::Ended { opener, end, turns, usage }, by_tools(tools::Fact::Closed { session: owner })]);
}

#[test]
fn retries_cancels_and_refusals_are_told_too() {
    let mut h = Harness::new(Limits { sessions: 1, ..LIMITS });
    let opener = Token::new(1);
    let (owner, _) = h.open(1);
    assert_eq!(h.step(Event::Failed { owner, failure: Failure::Overloaded }), None);
    let retry = h.model.next_deadline().expect("a retry is armed");
    let started = Fact::CompletionStarted { opener, attempt: 0, messages: 1, max_tokens: 1024 };
    let delay = retry.saturating_since(Time::ZERO);
    h.told(&[
        Fact::Opened { opener },
        started,
        Fact::Tools { fact: tools::Fact::Opened { session: owner } },
        Fact::CompletionFailed { opener, failure: Failure::Overloaded },
        Fact::CompletionRetried { opener, attempt: 1, delay },
    ]);
    h.env.now = retry;
    drop(calling(h.fire()));
    h.told(&[Fact::CompletionStarted { opener, attempt: 1, messages: 1, max_tokens: 1024 }]);

    let refused = Token::new(2);
    drop(h.step(Event::Open { opener: refused, spec: spec() }));
    h.told(&[Fact::Ended { opener: refused, end: End::Busy, turns: 0, usage: Usage::ZERO }]);

    assert_eq!(h.step(Event::Close { session: owner }), Some(Request::Cancel { owner }));
    h.told(&[Fact::Tools { fact: tools::Fact::Closed { session: owner } }]);
    drop(h.step(Event::Cancelled { owner }));
    let end = Fact::Ended { opener, end: End::Closed, turns: 0, usage: Usage::ZERO };
    h.told(&[Fact::CompletionCancelled { opener }, end]);
}

/// Opens a session, has the LLM call a tool and the tool run, and returns
/// the session's name and what came out.
fn drive(h: &mut Harness) -> (Token, Prompt, Option<Request>, Option<Request>) {
    let (owner, prompt) = h.open(1);
    let tool = h.step(Event::Completed { owner, completion: reading() });
    let Some(Request::Io { owner: run, .. }) = tool else {
        panic!("expected a tool run, not {tool:?}");
    };
    let call = h.step(ran(run, b"main.rs"));
    (owner, prompt, tool, call)
}

#[test]
fn facts_beyond_their_room_are_dropped_and_counted_and_change_nothing() {
    let mut full = Harness::new(Limits { facts: 2, ..LIMITS });
    let mut none = Harness::new(Limits { facts: 0, ..LIMITS });
    let mut roomy = Harness::new(LIMITS);
    let requests = drive(&mut roomy);
    assert_eq!(drive(&mut full), requests);
    assert_eq!(drive(&mut none), requests);
    // Three facts on opening, the tools' among them, three on the completion,
    // two on the tool's result.
    assert_eq!((full.model.facts_lost(), none.model.facts_lost(), roomy.model.facts_lost()), (6, 8, 0));
    let opener = Token::new(1);
    full.told(&[
        Fact::Opened { opener },
        Fact::CompletionStarted { opener, attempt: 0, messages: 1, max_tokens: 1024 },
    ]);
    none.told(&[]);

    // Drained, there is room again; what was dropped stays counted.
    let (owner, ..) = requests;
    drop(yielded(full.step(Event::Completed { owner, completion: done() })));
    let answered = Fact::CompletionAnswered { opener, stop: Stop::EndTurn, blocks: 1, calls: 0, invalid: 0 };
    full.told(&[answered, Fact::Used { opener, usage: USAGE }]);
    assert_eq!(full.model.facts_lost(), 7);
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bytes = worst_case(&LIMITS).expect("the test limits fit");
    assert!(bytes > 2 * LIMITS.session_bytes, "every session may hold its bytes");
    let told = worst_case(&Limits { facts: LIMITS.facts + 1, ..LIMITS }).expect("the test limits fit");
    let fact = u64::try_from(size_of::<Fact>()).expect("a size fits");
    assert_eq!(told - bytes, fact, "the facts' queue is counted, and nothing else of theirs");
    assert_eq!(worst_case(&Limits { sessions: u32::MAX, session_bytes: u64::MAX, ..LIMITS }), None);
}

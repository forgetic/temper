//! Feed the model events, inspect the requests that come out: the paths that
//! cross from the run to the sessions and back, the tickets the top level
//! keeps for them, and the hand-offs it defers to the ready list. What each
//! sub-model does on its own is its own tests' business.

use alloc::boxed::Box;

use temper_agent_model_run::charter::{Checkout, Endpoint, Families, Grants, Llm, Repository, Tools};
use temper_agent_model_run::outcome::{Children, Declared, OutcomeSpec, Verdict, VerdictRule};
use temper_agent_model_run::{self as run, Ask, Charter};
use temper_agent_model_session as session;
use temper_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token};

use crate::llm::{Block, Completion, Decoded, Message, Problem, Prompt, Returned, Role, Said, Served, Stop, Usage};
use crate::tools::{self, Call, Name, Op, Part, Path, Place};
use crate::{Event, Fact, Limits, Model, Request, fire, max_out, resume, step, worst_case};

const BUDGET: run::Budget = run::Budget {
    turns: 10,
    input: 100_000,
    output: 10_000,
    cache_read: 100_000,
    cache_write: 100_000,
    time: Duration::from_secs(600),
};

const CEILING: session::Budget = session::Budget {
    turns: 100,
    input: 1_000_000,
    output: 100_000,
    cache_read: 1_000_000,
    cache_write: 1_000_000,
    time: Duration::from_secs(3600),
};

const LIMITS: Limits = Limits {
    run: run::Limits {
        runs: 2,
        conversations: 4,
        run_bytes: 4096,
        repositories: 2,
        outlets: 2,
        verdicts: 2,
        calls: 4,
        budget: run::Budget {
            turns: CEILING.turns,
            input: CEILING.input,
            output: CEILING.output,
            cache_read: CEILING.cache_read,
            cache_write: CEILING.cache_write,
            time: CEILING.time,
        },
        max_tokens: 1024,
        models: 1,
        depth: 2,
        run_conversations: 3,
        answer_bytes: 64,
        nudges: 1,
        guide_bytes: 64,
        io_timeout: Duration::from_secs(10),
        outcome_bytes: 1024,
        check_timeout: Duration::from_secs(300),
        check_tail: 256,
        facts: 64,
    },
    session: session::Limits {
        sessions: 4,
        messages: 16,
        session_bytes: 65_536,
        budget: CEILING,
        max_tokens: 1024,
        retries: 1,
        backoff_base: Duration::from_millis(100),
        backoff_max: Duration::from_secs(1),
        call_timeout: Duration::from_secs(30),
        tool_timeout: Duration::from_secs(20),
        facts: 64,
        parallel_tools: 2,
        tools: tools::Limits {
            kits: 4,
            calls: 4,
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
        },
    },
};

const USAGE: Usage = Usage { input_tokens: 100, output_tokens: 10, cache_read_tokens: 0, cache_write_tokens: 0 };

/// The model, its environment, and room for one entry point's output.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new() -> Harness {
        Harness {
            model: Model::new(&LIMITS, 1),
            env: Env { now: Time::ZERO, limits: LIMITS },
            out: Queue::with_capacity(max_out(&LIMITS)),
        }
    }

    /// Steps `event`, returning what it emitted, oldest first.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.drain()
    }

    fn fire(&mut self) -> Box<[Request]> {
        assert!(self.model.is_due(self.env.now), "an alarm is due");
        fire(&mut self.model, &self.env, &mut self.out);
        self.drain()
    }

    /// The loop's next iteration, as far as the model goes: the reclaim point
    /// of this one, then whatever is ready, at the start of the model's stage.
    fn next(&mut self) -> Box<[Request]> {
        self.model.reclaim();
        let mut all = List::with_capacity(max_out(&LIMITS));
        for _ in 0..max_out(&LIMITS) {
            if !self.model.is_ready() {
                break;
            }
            resume(&mut self.model, &self.env, &mut self.out);
            for request in self.drain() {
                all.push(request).expect("a test's iteration emits little");
            }
        }
        assert!(!self.model.is_ready(), "what is made ready while the list is drained waits for the next iteration");
        all.into_boxed()
    }

    fn drain(&mut self) -> Box<[Request]> {
        let mut requests = List::with_capacity(max_out(&LIMITS));
        for _ in 0..max_out(&LIMITS) {
            let Some(request) = self.out.pop() else { break };
            requests.push(request).expect("room for max_out");
        }
        assert!(self.out.is_empty(), "an entry point emits at most max_out");
        requests.into_boxed()
    }

    /// Starts a run of `charter` for call `call`, and has it find no guide:
    /// the run's token, and main's first call to its LLM.
    fn admit(&mut self, call: u64, charter: Charter) -> (Token, Token, Prompt) {
        let emitted =
            self.step(Event::Start { reply_to: ReplyTo::new(Token::new(call)), worker: Token::new(call), charter });
        let [Request::Admitted { worker: _, run }, Request::Read { owner, .. }] = &*emitted else {
            panic!("expected an admitted run, got {emitted:?}");
        };
        assert_eq!(owner, run);
        let run = *run;
        let (main, prompt) = completing(self.step(Event::Read { owner: run, read: run::Read::Missing }));
        (run, main, prompt)
    }

    /// The session `owner`'s LLM answers with `content`, stopping for tools.
    fn answer(&mut self, owner: Token, content: Box<[Said]>) -> Box<[Request]> {
        let completion = Completion { content, stop: Stop::ToolUse, usage: USAGE };
        self.step(Event::Completed { owner, completion })
    }

    /// The session `owner`'s LLM ends its turn saying `text`.
    fn says(&mut self, owner: Token, text: &[u8]) -> Box<[Request]> {
        let completion =
            Completion { content: Box::new([Said::Text { text: bytes(text) }]), stop: Stop::EndTurn, usage: USAGE };
        self.step(Event::Completed { owner, completion })
    }
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn charter() -> Charter {
    Charter {
        brief: bytes(b"Review the change."),
        checkout: Checkout {
            repositories: Box::new([Repository { name: bytes(b"temper"), root: Token::new(900), writable: true }]),
        },
        grants: Grants { tools: TOOLS, forge: false, agents: true, outlets: Box::new([]) },
        outcome: OutcomeSpec { change: None, verdicts: Box::new([rule(b"approve")]) },
        budget: BUDGET,
        llm: Llm { endpoint: Endpoint(1), model: bytes(b"model-a"), max_tokens: 512 },
        models: Box::new([]),
    }
}

const TOOLS: Tools = Tools { inspect: true, modify: true, shell: false };

fn rule(name: &[u8]) -> VerdictRule {
    VerdictRule { name: bytes(name), children: Children { min: 0, max: 0 }, kinds: Box::new([]), fields: Box::new([]) }
}

fn verdict(name: &[u8]) -> Ask {
    Ask::Finish {
        outcome: Declared::Verdict(Verdict { name: bytes(name), body: bytes(b"ok"), children: Box::new([]) }),
    }
}

/// A sub-agent that may only read, and itself ask for sub-agents.
fn sub_agent(brief: &[u8]) -> Ask {
    let families = Families { tools: Tools { inspect: true, modify: false, shell: false }, forge: false, agents: true };
    Ask::SubAgent { brief: bytes(brief), families, llm: None, share: None }
}

fn served(id: &[u8], ask: Ask) -> Said {
    Said::ToolCall { id: bytes(id), name: bytes(b"served"), input: bytes(b"{}"), call: Decoded::Served { ask } }
}

fn owned(id: &[u8], call: Call) -> Said {
    Said::ToolCall { id: bytes(id), name: bytes(b"read"), input: bytes(b"{}"), call: Decoded::Owned { call } }
}

fn path(absolute: bool, names: &[&[u8]]) -> Path {
    let mut parts = List::with_capacity(4);
    for name in names {
        parts.push(Part::Name { name: Name::new(bytes(name)).expect("a name") }).expect("short paths");
    }
    Path { absolute, parts: parts.into_boxed() }
}

/// The one call to an LLM `emitted` holds: its owner and its prompt.
fn completing(emitted: Box<[Request]>) -> (Token, Prompt) {
    let Ok(one) = Box::<[Request; 1]>::try_from(emitted) else {
        panic!("expected one request");
    };
    let [Request::Complete { owner, prompt, timeout }] = *one else {
        panic!("expected a call to an LLM");
    };
    assert_eq!(timeout, LIMITS.session.call_timeout);
    (owner, prompt)
}

/// The blocks of `prompt`'s last message.
fn last(prompt: &Prompt) -> &[Block] {
    &prompt.messages.last().expect("a prompt has messages").content
}

#[test]
fn the_limits_fit_and_a_session_must_take_what_the_run_asks() {
    assert!(worst_case(&LIMITS).is_some());
    let fewer = Limits { session: session::Limits { sessions: 3, ..LIMITS.session }, ..LIMITS };
    assert_eq!(worst_case(&fewer), None, "a session for every conversation the run may have");
    let budget = session::Budget { turns: 50, ..CEILING };
    let smaller = Limits { session: session::Limits { budget, ..LIMITS.session }, ..LIMITS };
    assert_eq!(worst_case(&smaller), None, "a session's budget as large as the run's");
}

#[test]
fn a_run_opens_main_as_a_session_through_to_its_first_call_to_the_llm() {
    let mut h = Harness::new();
    let (_, main, prompt) = h.admit(7, charter());
    assert_eq!((prompt.endpoint, &*prompt.model, prompt.max_tokens), (session::llm::Endpoint(1), &b"model-a"[..], 512));
    assert_eq!(prompt.tools, tools::Grants { inspect: true, modify: true, shell: false });
    assert_eq!(&*prompt.served, &[Served::Finish, Served::SubAgent], "main may finish, and ask for sub-agents");
    let [Message { role: Role::User, content }] = &*prompt.messages else {
        panic!("expected the first message only");
    };
    assert!(matches_text(content), "the run's first message");
    assert_eq!((h.model.peers(), h.model.tickets(), h.model.flights()), (1, 0, 0));

    // Relative paths start in the first repository, mounted under its name.
    let relative = Call::Read { path: path(false, &[b"src", b"lib.rs"]), skip: 0, lines: None };
    let absolute = Call::Read { path: path(true, &[b"temper", b"README"]), skip: 0, lines: None };
    let emitted = h.answer(main, Box::new([owned(b"c1", relative), owned(b"c2", absolute)]));
    let [Request::Io { op: Op::Load { at: first, .. }, .. }, Request::Io { op: Op::Load { at: second, .. }, .. }] =
        &*emitted
    else {
        panic!("expected two loads, got {emitted:?}");
    };
    assert_eq!(first, &Place { root: Token::new(900), path: bytes(b"src/lib.rs") });
    assert_eq!(second, &Place { root: Token::new(900), path: bytes(b"README") });
}

fn matches_text(content: &[Block]) -> bool {
    match content {
        [Block::Text { text }] => !text.is_empty(),
        _ => false,
    }
}

#[test]
fn a_checkout_the_tools_cannot_lay_out_refuses_main_as_invalid() {
    let mut h = Harness::new();
    let checkout = Checkout {
        repositories: Box::new([Repository { name: bytes(b"ai/temper"), root: Token::new(900), writable: true }]),
    };
    let emitted = h.step(Event::Start {
        reply_to: ReplyTo::new(Token::new(7)),
        worker: Token::new(7),
        charter: Charter { checkout, ..charter() },
    });
    let [Request::Admitted { run, .. }, Request::Read { .. }] = &*emitted else {
        panic!("expected an admitted run, got {emitted:?}");
    };
    let emitted = h.step(Event::Read { owner: *run, read: run::Read::Missing });
    let [Request::Answer { to: _, answer }] = &*emitted else {
        panic!("expected the run's answer, got {emitted:?}");
    };
    assert_eq!(answer, &run::Answer::Refused(run::Refusal::Invalid(run::Invalid::Conversation)));
    assert_eq!(h.model.peers(), 0);
}

#[test]
fn a_finish_the_run_rejects_at_once_comes_back_from_the_ready_list_as_the_sessions_answer() {
    let mut h = Harness::new();
    let (_, main, _) = h.admit(7, charter());
    // The run judges the finish within the step, and rejects it; the answer
    // waits for the next iteration.
    assert!(h.answer(main, Box::new([served(b"f1", verdict(b"merge"))])).is_empty());
    assert_eq!((h.model.tickets(), h.model.flights()), (1, 1), "the rejection's ticket, and the call it answers");
    assert!(!h.model.is_ready(), "not before the reclaim point");

    let (owner, prompt) = completing(h.next());
    assert_eq!(owner, main);
    let [Block::ToolResult { id, result: Returned::Served { returned, error: true } }] = last(&prompt) else {
        panic!("expected the rejection, got {:?}", last(&prompt));
    };
    assert_eq!(&**id, b"f1");
    assert!(matches_rejected(returned), "got {returned:?}");
    let [_, Message { role: Role::Assistant, content }, _] = &*prompt.messages else {
        panic!("expected the call and its result");
    };
    assert_eq!(&**content, &[Block::ToolCall { id: bytes(b"f1"), name: bytes(b"served"), input: bytes(b"{}") }]);
    assert_eq!((h.model.tickets(), h.model.flights()), (1, 0), "the answer stays in the transcript");
}

fn matches_rejected(returned: &run::Returned) -> bool {
    match returned {
        run::Returned::Rejected { problems } => !problems.listed.is_empty(),
        run::Returned::Accepted
        | run::Returned::ChecksFailed { .. }
        | run::Returned::Moved
        | run::Returned::Unpushed
        | run::Returned::Cancelled
        | run::Returned::Busy
        | run::Returned::Answered { .. }
        | run::Returned::Unanswered { .. }
        | run::Returned::Refused { .. } => false,
    }
}

#[test]
fn an_accepted_finish_closes_main_and_the_run_answers_the_worker() {
    let mut h = Harness::new();
    let (_, main, _) = h.admit(7, charter());
    assert!(h.answer(main, Box::new([served(b"f1", verdict(b"approve"))])).is_empty());
    // The answer and the close come in the next iteration: the close first,
    // which withdraws the call, whose answer has won.
    let emitted = h.next();
    let [Request::Answer { to, answer: run::Answer::Accepted { outcome: _, spent } }] = &*emitted else {
        panic!("expected the run accepted, got {emitted:?}");
    };
    assert_eq!(to, &ReplyTo::new(Token::new(7)));
    assert_eq!((spent.turns, spent.input), (1, USAGE.input_tokens));
    h.model.reclaim();
    assert_eq!((h.model.peers(), h.model.tickets(), h.model.flights()), (0, 0, 0), "main's tickets went with it");
}

#[test]
fn a_sub_agent_opens_a_child_session_whose_last_message_answers_the_call() {
    let mut h = Harness::new();
    let (_, main, _) = h.admit(7, charter());
    // The run opens the child within the step: its session calls its LLM.
    let (child, prompt) = completing(h.answer(main, Box::new([served(b"a1", sub_agent(b"Find the bug."))])));
    assert_ne!(child, main);
    assert_eq!(prompt.tools, tools::Grants { inspect: true, modify: false, shell: false }, "the families asked for");
    assert_eq!(&*prompt.served, &[Served::SubAgent], "a child never finishes");
    assert_eq!((h.model.peers(), h.model.flights()), (2, 1));

    // The child calls finish, which it was not offered: no call, answered at
    // once, and its LLM is called again.
    let (again, prompt) = completing(h.answer(child, Box::new([served(b"f1", verdict(b"approve"))])));
    assert_eq!(again, child);
    let [Block::ToolResult { id: _, result: Returned::Invalid { problem: Problem::UnknownTool } }] = last(&prompt)
    else {
        panic!("expected the call refused, got {:?}", last(&prompt));
    };

    // The child yields: the run closes it (from the ready list), and its end
    // answers main's call (from the ready list again).
    assert!(h.says(child, b"It is in main.rs.").is_empty());
    assert!(h.next().is_empty(), "the child closes, and ends");
    let (owner, prompt) = completing(h.next());
    assert_eq!(owner, main);
    let [Block::ToolResult { id, result: Returned::Served { returned, error: false } }] = last(&prompt) else {
        panic!("expected the child's answer, got {:?}", last(&prompt));
    };
    assert_eq!(&**id, b"a1");
    let run::Returned::Answered { text, cut: 0, stop: run::Stop::EndTurn } = returned else {
        panic!("expected the child's last message, got {returned:?}");
    };
    assert_eq!(&**text, b"It is in main.rs.");
    h.model.reclaim();
    assert_eq!((h.model.peers(), h.model.flights(), h.model.tickets()), (1, 0, 1));
}

#[test]
fn a_cancel_cascades_down_a_two_level_tree_one_level_an_iteration() {
    let mut h = Harness::new();
    let (run, main, _) = h.admit(7, charter());
    let (child, _) = completing(h.answer(main, Box::new([served(b"a1", sub_agent(b"Look around."))])));
    let (grandchild, _) = completing(h.answer(child, Box::new([served(b"a2", sub_agent(b"Look closer."))])));
    assert_eq!((h.model.peers(), h.model.flights()), (3, 2));

    // Each close withdraws the call whose sub-agent the run closes next, an
    // iteration later; the grandchild cancels its call to the LLM.
    assert!(h.step(Event::Cancel { run }).is_empty());
    assert!(h.next().is_empty(), "main closes, withdrawing its call");
    assert!(h.next().is_empty(), "the child closes, withdrawing its call");
    assert_eq!(&*h.next(), &[Request::Cancel { owner: grandchild }]);

    // The ends come back up the same way, each answer from the ready list.
    assert!(h.step(Event::Cancelled { owner: grandchild }).is_empty());
    assert!(h.next().is_empty(), "the child's call is answered cancelled, and it ends");
    let emitted = h.next();
    let [Request::Answer { to: _, answer: run::Answer::Failed { failure: run::Failure::Cancelled, spent } }] =
        &*emitted
    else {
        panic!("expected the run cancelled, got {emitted:?}");
    };
    assert_eq!(spent.turns, 2, "main's turn and the child's; the grandchild's was cancelled");
    h.model.reclaim();
    assert_eq!((h.model.peers(), h.model.flights(), h.model.tickets()), (0, 0, 0), "every ticket freed");
    assert_eq!((h.model.run().runs(), h.model.session().sessions()), (0, 0));
}

#[test]
fn the_asks_of_an_answer_that_yields_are_forgotten() {
    let mut h = Harness::new();
    let (_, main, _) = h.admit(7, charter());
    let completion =
        Completion { content: Box::new([served(b"f1", verdict(b"approve"))]), stop: Stop::MaxTokens, usage: USAGE };
    // The run nudges at once: the session is continued within the step.
    let (owner, prompt) = completing(h.step(Event::Completed { owner: main, completion }));
    assert_eq!(owner, main);
    assert_eq!(h.model.tickets(), 0, "the call that did not run left no ticket");
    let [_, _, Message { role: Role::User, content }] = &*prompt.messages else {
        panic!("expected the nudge after the call that did not run");
    };
    let [Block::ToolResult { id: _, result: Returned::NotRun }, Block::Text { .. }] = &**content else {
        panic!("expected the call not run, then the nudge, got {content:?}");
    };
}

#[test]
fn an_ask_larger_than_the_session_may_hold_is_too_large() {
    let mut h = Harness::new();
    let (_, main, _) = h.admit(7, charter());
    let mut brief = List::with_capacity(70_000);
    for _ in 0..70_000_u32 {
        brief.push(b'x').expect("room for the brief");
    }
    let brief = brief.into_boxed();
    let ask = Ask::SubAgent {
        brief,
        families: Families { tools: TOOLS, forge: false, agents: false },
        llm: None,
        share: None,
    };
    let (_, prompt) = completing(h.answer(main, Box::new([served(b"a1", ask)])));
    let [Block::ToolResult { id: _, result: Returned::Invalid { problem: Problem::TooLarge } }] = last(&prompt) else {
        panic!("expected the call too large, got {:?}", last(&prompt));
    };
    assert_eq!((h.model.tickets(), h.model.peers()), (0, 1));
}

#[test]
fn the_runs_deadline_fires_before_the_sessions_expiry_at_the_same_instant() {
    let mut h = Harness::new();
    let (_, main, _) = h.admit(7, charter());
    h.env.now = Time::ZERO.saturating_add(BUDGET.time);
    // The run winds down: its close of main waits for the next iteration, and
    // main's own expiry fires meanwhile.
    assert!(h.fire().is_empty());
    assert_eq!(&*h.fire(), &[Request::Cancel { owner: main }]);
    assert!(!h.model.is_due(h.env.now));
    assert!(h.next().is_empty(), "the close finds main closing already");
    let emitted = h.step(Event::Cancelled { owner: main });
    let [Request::Answer { to: _, answer: run::Answer::Failed { failure, spent: _ } }] = &*emitted else {
        panic!("expected the run failed, got {emitted:?}");
    };
    assert_eq!(failure, &run::Failure::Budget(run::Exhausted::Time));
}

#[test]
fn the_facts_of_both_sub_models_are_gathered() {
    let mut h = Harness::new();
    let (_run, _main, _prompt) = h.admit(7, charter());
    let mut run = 0_u32;
    let mut sessions = 0_u32;
    for _ in 0..64_u32 {
        match h.model.pop_fact() {
            Some(Fact::Run { .. }) => run += 1,
            Some(Fact::Session { .. }) => sessions += 1,
            None => break,
        }
    }
    assert!(run > 0 && sessions > 0, "{run} of the run's, {sessions} of the sessions'");
    assert_eq!(h.model.facts_lost(), 0);
}

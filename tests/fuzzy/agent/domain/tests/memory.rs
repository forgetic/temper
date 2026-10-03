//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the top level driven at random through runs on
//! charters as large as they may be, their conversations' completions (calls
//! to the tools, finishes and sub-agents among them, at sizes up to what a
//! session holds), what io and the worker answer, and cancels; its peak
//! measured in every entry point, as the loop calls them.

use skein_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token, Wall};
use temper_agent_domain::llm::{Completion, Decoded, Failure, Problem, Prompt, Said, Served, Stop, Usage};
use temper_agent_domain::run::charter::{Checkout, Endpoint, Families, Grants, Llm, Repository, Tools};
use temper_agent_domain::run::outcome::{Change, ChangeSpec, Child, Children, Declared, Field, OutcomeSpec};
use temper_agent_domain::run::outcome::{Verdict, VerdictRule};
use temper_agent_domain::run::{self, Ask, Charter};
use temper_agent_domain::tools::{Call, Done, Entry, Exit, Fault, Hit, Kind, Name, Op, Part, Path, Version};
use temper_agent_domain::{Domain, Event, Limits, Request, fire, max_out, resume, step, worst_case};
use temper_agent_domain_tests::TIGHT;
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

fn bytes(len: u64) -> Box<[u8]> {
    vec![b'x'; usize::try_from(len).expect("a test length fits")].into_boxed_slice()
}

fn name(text: &[u8]) -> Name {
    Name::new(text.into()).expect("a name")
}

fn path(names: &[&[u8]]) -> Path {
    Path { absolute: false, parts: names.iter().map(|text| Part::Name { name: name(text) }).collect() }
}

fn index(rng: &mut Rng, len: usize) -> usize {
    usize::try_from(rng.below(u64::try_from(len).expect("small"))).expect("an index")
}

/// Small limits, so that every bound is met often: few runs and sessions,
/// few bytes each.
const LIMITS: Limits = Limits {
    run: run::Limits {
        runs: 2,
        conversations: 4,
        run_bytes: 2048,
        run_conversations: 3,
        calls: 4,
        answer_bytes: 128,
        guide_bytes: 128,
        outcome_bytes: 512,
        check_tail: 64,
        facts: 32,
        ..TIGHT.run
    },
    session: temper_agent_domain::session::Limits {
        sessions: 4,
        messages: 12,
        session_bytes: 4096,
        parallel_tools: 2,
        facts: 32,
        tools: temper_agent_domain::tools::Limits {
            kits: 4,
            calls: 2,
            file_bytes: 512,
            read_bytes: 256,
            list_entries: 8,
            shell_head: 64,
            shell_tail: 64,
            search_hits: 4,
            search_bytes: 128,
            facts: 32,
            ..TIGHT.session.tools
        },
        ..TIGHT.session
    },
};

/// A charter of `brief` bytes of brief that grants everything, and wants a
/// change that passes its checks or a verdict.
fn charter(brief: u64) -> Charter {
    let repository = Repository { name: (*b"temper").into(), root: Token::new(1), writable: true };
    let all = Tools { inspect: true, modify: true, shell: true };
    let rule = VerdictRule {
        name: (*b"request-changes").into(),
        children: Children { min: 1, max: 4 },
        kinds: Box::new([(*b"nit").into()]),
        fields: Box::new([(*b"path").into()]),
    };
    Charter {
        brief: bytes(brief),
        checkout: Checkout { repositories: Box::new([repository]) },
        grants: Grants { tools: all, forge: false, agents: true, outlets: Box::new([]) },
        outcome: OutcomeSpec { change: Some(ChangeSpec { checks: true }), verdicts: Box::new([rule]) },
        budget: run::Budget { turns: 12, ..TIGHT.run.budget },
        llm: Llm { endpoint: Endpoint(0), model: (*b"m").into(), max_tokens: 256 },
        models: Box::new([]),
    }
}

/// What the domain asked for and the driver has yet to end, as the driver
/// keeps it: tokens and kinds, nothing the domain allocated.
#[derive(Clone, Copy, Debug)]
enum Asked {
    /// A call to an LLM, which may finish and ask for sub-agents if
    /// offered; and whether it was cancelled.
    Complete {
        owner: Token,
        finish: bool,
        agents: bool,
        cancelled: bool,
    },
    Io {
        owner: Token,
        op: OpKind,
        cancelled: bool,
    },
    Read {
        owner: Token,
    },
    Probe {
        owner: Token,
    },
    Check {
        owner: Token,
        aborted: bool,
    },
    Push {
        owner: Token,
        cancelled: bool,
    },
}

/// The families of requests that may be cancelled.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    Llm,
    Io,
    Check,
    Push,
}

#[derive(Clone, Copy, Debug)]
enum OpKind {
    Load,
    Scan,
    Store,
    Spawn,
    Search,
}

/// The driver: what is in flight, and the runs, in containers allocated
/// before the meter's base that never grow past their capacity.
struct Driver {
    rng: Rng,
    asked: Vec<Asked>,
    runs: Vec<Token>,
    workers: u64,
    /// Requests seen, by kind: completions, io, looks, checks, pushes,
    /// answers.
    seen: [u32; 6],
}

impl Driver {
    /// Takes what an entry point asked for, keeping what the driver needs to
    /// end it, and drops the rest.
    fn drain(&mut self, out: &mut Queue<Request>) {
        while let Some(request) = out.pop() {
            match request {
                Request::Admitted { worker: _, run } => self.runs.push(run),
                Request::Answer { .. } => self.seen[5] += 1,
                Request::Checking { .. } => {}
                Request::Push { owner, .. } => {
                    self.seen[4] += 1;
                    self.ask(Asked::Push { owner, cancelled: false });
                }
                Request::CancelHost { owner } => self.cancel(Family::Push, owner),
                Request::Complete { owner, prompt, timeout: _ } => {
                    self.seen[0] += 1;
                    let (finish, agents) = served(&prompt);
                    self.ask(Asked::Complete { owner, finish, agents, cancelled: false });
                }
                Request::Cancel { owner } => self.cancel(Family::Llm, owner),
                Request::Io { owner, op, deadline: _ } => {
                    self.seen[1] += 1;
                    let op = match op {
                        Op::Load { .. } => OpKind::Load,
                        Op::Scan { .. } => OpKind::Scan,
                        Op::Store { .. } => OpKind::Store,
                        Op::Spawn { .. } => OpKind::Spawn,
                        Op::Search { .. } => OpKind::Search,
                    };
                    self.ask(Asked::Io { owner, op, cancelled: false });
                }
                Request::CancelIo { owner } => self.cancel(Family::Io, owner),
                Request::Read { owner, .. } => {
                    self.seen[2] += 1;
                    self.ask(Asked::Read { owner });
                }
                Request::Probe { owner, .. } => {
                    self.seen[2] += 1;
                    self.ask(Asked::Probe { owner });
                }
                Request::Check { owner, .. } => {
                    self.seen[3] += 1;
                    self.ask(Asked::Check { owner, aborted: false });
                }
                Request::Abort { owner } => self.cancel(Family::Check, owner),
            }
        }
    }

    fn ask(&mut self, asked: Asked) {
        assert!(self.asked.len() < self.asked.capacity(), "the driver's room is allocated before the base");
        self.asked.push(asked);
    }

    /// Marks what `owner` asked for of `family` as cancelled, if it is in
    /// flight: tokens of different families may be equal.
    fn cancel(&mut self, family: Family, owner: Token) {
        for asked in &mut self.asked {
            let (of, cancelled) = match asked {
                Asked::Complete { owner, cancelled, .. } => (Family::Llm, (owner, cancelled)),
                Asked::Io { owner, cancelled, .. } => (Family::Io, (owner, cancelled)),
                Asked::Check { owner, aborted } => (Family::Check, (owner, aborted)),
                Asked::Push { owner, cancelled } => (Family::Push, (owner, cancelled)),
                Asked::Read { .. } | Asked::Probe { .. } => continue,
            };
            if of == family && *cancelled.0 == owner {
                *cancelled.1 = true;
            }
        }
    }

    /// The next event: a start, an end of something in flight, or a cancel.
    fn event(&mut self, limits: &Limits) -> Option<Event> {
        let roll = self.rng.below(12);
        if roll == 0 || self.asked.is_empty() {
            if self.rng.chance(200) {
                return self.cancel_run();
            }
            self.workers += 1;
            let worker = Token::new(self.workers);
            // Most charters as large as a run may hold, some a byte larger.
            let brief = limits.run.run_bytes - 900 + self.rng.below(901);
            return Some(Event::Start { reply_to: ReplyTo::new(worker), worker, charter: charter(brief) });
        }
        if roll == 1 && !self.runs.is_empty() && self.rng.chance(100) {
            return self.cancel_run();
        }
        let asked = self.asked.swap_remove(index(&mut self.rng, self.asked.len()));
        Some(match asked {
            Asked::Complete { owner, cancelled: true, .. } if self.rng.chance(700) => Event::Cancelled { owner },
            Asked::Complete { owner, finish, agents, .. } => match self.rng.below(10) {
                0 => Event::Failed { owner, failure: Failure::Overloaded },
                _ => Event::Completed { owner, completion: self.completion(limits, finish, agents) },
            },
            Asked::Io { owner, cancelled: true, .. } if self.rng.chance(700) => {
                Event::Done { owner, done: Done::Cancelled }
            }
            Asked::Io { owner, op, .. } => Event::Done { owner, done: self.done(limits, op) },
            Asked::Read { owner } => {
                let read = match self.rng.below(4) {
                    0 => run::Read::Missing,
                    _ => run::Read::Text { text: bytes(u64::from(limits.run.guide_bytes)), whole: false },
                };
                Event::Read { owner, read }
            }
            Asked::Probe { owner } => Event::Probed { owner, executable: self.rng.chance(700) },
            Asked::Check { owner, aborted: true } if self.rng.chance(700) => Event::Aborted { owner },
            Asked::Check { owner, .. } => {
                let exit = run::Exit::Code { code: u8::from(self.rng.chance(500)) };
                let ran = run::Ran { exit, output: bytes(u64::from(limits.run.check_tail)), cut: 1000 };
                Event::Checked { owner, ran }
            }
            Asked::Push { owner, cancelled: true } if self.rng.chance(700) => Event::HostCancelled { owner },
            Asked::Push { owner, .. } => {
                let push = [run::Push::Done, run::Push::Moved, run::Push::Failed][index(&mut self.rng, 3)];
                Event::Pushed { owner, push }
            }
        })
    }

    fn cancel_run(&mut self) -> Option<Event> {
        if self.runs.is_empty() {
            return None;
        }
        let run = self.runs[index(&mut self.rng, self.runs.len())];
        Some(Event::Cancel { run })
    }

    /// A completion of up to two calls, of the tools or the run's, or text,
    /// holding up to a quarter of what a session may.
    fn completion(&mut self, limits: &Limits, finish: bool, agents: bool) -> Completion {
        let most = limits.session.session_bytes / 4;
        let usage = Usage { input_tokens: 100, output_tokens: 20, cache_read_tokens: 50, cache_write_tokens: 50 };
        if self.rng.chance(150) {
            let text = Said::Text { text: bytes(self.rng.below(most)) };
            return Completion { content: Box::new([text]), stop: Stop::EndTurn, usage };
        }
        let count = 1 + self.rng.below(2);
        let content = (0..count)
            .map(|at| {
                let size = self.rng.below(most / 2);
                let call = self.call(limits, size, finish, agents);
                Said::ToolCall { id: format!("c{at}").into_bytes().into(), name: bytes(4), input: bytes(size), call }
            })
            .collect();
        Completion { content, stop: Stop::ToolUse, usage }
    }

    fn call(&mut self, limits: &Limits, size: u64, finish: bool, agents: bool) -> Decoded {
        let lib = || path(&[b"src", b"lib.rs"]);
        let call = match self.rng.below(9) {
            0 => Call::Read { path: lib(), skip: 0, lines: None },
            1 => Call::List { path: path(&[b"src"]) },
            2 => Call::Search { path: path(&[b"src"]), pattern: bytes(3), glob: None },
            3 => Call::Write { path: lib(), content: bytes(size) },
            4 => Call::Edit { path: lib(), old: bytes(1), new: bytes(size), all: false },
            5 => Call::Shell { command: bytes(8), timeout: None },
            6 if finish => return Decoded::Served { ask: self.finish(limits) },
            7 if agents => {
                let families = Families {
                    tools: Tools { inspect: true, modify: self.rng.chance(500), shell: false },
                    forge: false,
                    agents: self.rng.chance(500),
                };
                return Decoded::Served { ask: Ask::SubAgent { brief: bytes(size), families, llm: None, share: None } };
            }
            _ => return Decoded::Invalid { problem: Problem::Missing { field: bytes(size.min(16)) } },
        };
        Decoded::Owned { call }
    }

    /// A finish with a change or a verdict, as large as an outcome may be.
    fn finish(&mut self, limits: &Limits) -> Ask {
        let most = limits.run.outcome_bytes;
        let outcome = if self.rng.chance(500) {
            Declared::Change(Change { title: bytes(1), body: bytes(self.rng.below(most)) })
        } else {
            let child = || Child {
                kind: (*b"nit").into(),
                fields: Box::new([Field { name: (*b"path").into(), value: bytes(8) }]),
            };
            let children = (0..self.rng.below(3)).map(|_| child()).collect();
            let name = (*b"request-changes").into();
            Declared::Verdict(Verdict { name, body: bytes(self.rng.below(most / 2)), children })
        };
        Ask::Finish { outcome }
    }

    /// A terminal io may end an operation of `op` with, as large as the
    /// tools take.
    fn done(&mut self, limits: &Limits, op: OpKind) -> Done {
        let tools = &limits.session.tools;
        let version = Version::new([self.rng.below(3), 0, 0, 0]);
        if self.rng.chance(100) {
            return [Done::Failed { fault: Fault::Other }, Done::TimedOut][index(&mut self.rng, 2)].clone();
        }
        match op {
            OpKind::Load => Done::Loaded { content: bytes(self.rng.below(u64::from(tools.file_bytes) + 1)), version },
            OpKind::Scan => {
                let entries = (0..tools.list_entries)
                    .map(|entry| Entry { name: name(format!("e{entry}").as_bytes()), kind: Kind::File });
                Done::Scanned { entries: entries.collect(), more: 3 }
            }
            OpKind::Store => Done::Stored { version },
            OpKind::Spawn => {
                let (head, tail) = (u64::from(tools.shell_head), u64::from(tools.shell_tail));
                Done::Exited { exit: Exit::Code { code: 1 }, head: bytes(head), tail: bytes(tail), dropped: 100 }
            }
            OpKind::Search => {
                let hit = |line| Hit { path: bytes(6), line, text: bytes(u64::from(tools.search_bytes) / 4) };
                Done::Found { hits: (0..tools.search_hits).map(hit).collect(), more: 1, timed_out: false }
            }
        }
    }
}

/// Whether `prompt` offers the run's finish and sub-agents.
fn served(prompt: &Prompt) -> (bool, bool) {
    let mut offered = (false, false);
    for tool in &prompt.served {
        match tool {
            Served::Finish => offered.0 = true,
            Served::SubAgent => offered.1 = true,
        }
    }
    offered
}

/// Drives a domain under `limits` for `rounds` rounds from `seed`, as the
/// loop would: each round resumes what is ready, takes an event, and fires
/// what is due, then reaches the reclaim point; checking the peak of the heap
/// in every entry point against the worst case.
fn churn(limits: &Limits, seed: u64, rounds: u32) -> [u32; 6] {
    let limits = *limits;
    let bound = worst_case(&limits).expect("the test limits fit");
    let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut out = Queue::with_capacity(max_out(&limits));
    let mut driver = Driver {
        rng: Rng::new(seed),
        asked: Vec::with_capacity(4096),
        runs: Vec::with_capacity(usize::try_from(rounds).expect("small")),
        workers: 0,
        seen: [0; 6],
    };
    let meter = Meter::new();
    let mut domain = Domain::new(&limits, seed);
    let mut measure = |domain: &mut Domain, env: &Env<Limits>, driver: &mut Driver, call: Point| {
        meter.start();
        match call {
            Point::Resume => resume(domain, env, &mut out),
            Point::Fire => fire(domain, env, &mut out),
            Point::Step(event) => step(domain, env, event, &mut out),
        }
        let measured = meter.end();
        driver.drain(&mut out);
        while domain.pop_fact().is_some() {}
        meter.check(measured, bound, limits);
    };
    for _ in 0..rounds {
        env.now = env.now.saturating_add(Duration::from_millis(driver.rng.below(3000)));
        for _ in 0..1000 {
            if !domain.is_ready() {
                break;
            }
            measure(&mut domain, &env, &mut driver, Point::Resume);
        }
        if let Some(event) = driver.event(&limits) {
            measure(&mut domain, &env, &mut driver, Point::Step(event));
        }
        for _ in 0..1000 {
            if !domain.is_due(env.now) {
                break;
            }
            measure(&mut domain, &env, &mut driver, Point::Fire);
        }
        domain.reclaim();
    }
    driver.seen
}

/// An entry point the loop calls.
enum Point {
    Resume,
    Fire,
    Step(Event),
}

#[test]
fn a_domain_driven_at_random_stays_within_its_worst_case_at_every_entry_point() {
    let wider = Limits {
        run: run::Limits { runs: 3, conversations: 8, run_conversations: 4, calls: 8, ..LIMITS.run },
        session: temper_agent_domain::session::Limits {
            sessions: 8,
            messages: 24,
            session_bytes: 8192,
            parallel_tools: 3,
            tools: temper_agent_domain::tools::Limits { kits: 8, calls: 3, ..LIMITS.session.tools },
            ..LIMITS.session
        },
    };
    let mut seen = [0; 6];
    for seed in 0..12 {
        for limits in [LIMITS, wider] {
            let counted = churn(&limits, seed, 3_000);
            for (all, one) in seen.iter_mut().zip(counted) {
                *all += one;
            }
        }
    }
    assert!(seen.iter().all(|count| *count > 10), "every kind of request was made: {seen:?}");
}

//! One real root and its children, driven by one person, one worker and
//! the existing ordered fake store. Requests become later input events
//! (domain/engine.md, section 15).

use crate::commits::Store;
use crate::walking_referee::{FINAL_SPEND, QUESTION, REPORT, TURNS, WalkingReferee};
use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use std::collections::VecDeque;
use temper_engine_domain::{Delivery, JournalLimits, Record, engine, loads};
use temper_engine_domain_accounts as accounts;
use temper_engine_domain_authority as authority;
use temper_engine_domain_brief as brief;
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;

/// Tiny counted child limits plus the root's declared synchronous-route room.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one explicit fixture lists the real child's tiny admission limits together")]
pub fn limits() -> engine::Limits {
    let retry = tasks::Retry { retries: 1, base: Duration::from_millis(10), max: Duration::from_secs(1) };
    let tasks = tasks::Limits {
        tasks: 2,
        funders: 4,
        admissions: 8,
        project_tasks: 2,
        stubs: 4,
        tree_tasks: 2,
        depth: 1,
        delegates: 1,
        batch: 1,
        dependencies: 1,
        inputs: 1,
        spec_bytes: 64,
        parameters: 1,
        result_bytes: 128,
        contract_choices: 1,
        charters: 1,
        authority_grants: 1,
        authority_segments: 1,
        authority_bytes: 32,
        executor_kinds: 1,
        retries: tasks::Retries {
            transient: retry,
            permanent: retry,
            run: retry,
            agent: retry,
            lost: retry,
            invalid: retry,
        },
        facts: 2,
        references: 1,
        inbox_messages: 4,
        inbox_bytes: 256,
        message_bytes: 64,
        questions: 1,
        subscriptions: 1,
        receipts: 4,
        offers: 4,
    };
    let people = people::Limits {
        people: 2,
        sign_ins: 2,
        projects: 1,
        holdings: 2,
        initial_owners: 1,
        requests: 4,
        pending: 2,
        waiters: 2,
        identity_bytes: 32,
        words: 64,
        sign_in_lifetime: Duration::from_secs(60),
        facts: 2,
    };
    let fleet = fleet::Limits {
        workers: 1,
        slots: 1,
        workstreams: 1,
        workstream_bytes: 32,
        attempts: 2,
        calls: 1,
        turns: 2,
        grace: Duration::from_secs(5),
        facts: 2,
    };
    let writes = tasks::max_out(&tasks) * 8 + people::max_out(&people) * 4 + fleet::max_out(&fleet) * 4;
    engine::Limits {
        authority: authority_limits(),
        journal: JournalLimits {
            commits: 3,
            held: 64,
            writes,
            deliveries: 16,
            transcript_bytes: 8192,
            result_bytes: 128,
        },
        loads: loads::Limits { loads: 2, rows: 1, bytes: 16384, reply_bytes: 16384, transcript_bytes: 8192 },
        tasks,
        people,
        fleet,
        brief: brief::Limits {
            briefs: 2,
            sections: 1,
            items: 1,
            parts: 2,
            read_bytes: 256,
            budgets: brief::Budgets {
                task: 128,
                item: 128,
                comments: 128,
                dependencies: 128,
                ci: 128,
                reviews: 128,
                pull: 128,
                attempts: 128,
                plan: 128,
                notes: 128,
                template: 128,
            },
            brief_bytes: 128,
            gather: Duration::from_secs(1),
            facts: 2,
        },
        accounts: accounts::Limits {
            accounts: 1,
            refresh_margin: Duration::from_secs(1),
            backoff_base: Duration::from_millis(10),
            backoff_max: Duration::from_secs(1),
            rejected_interval: Duration::from_millis(10),
            spent_attention: Duration::from_secs(1),
            facts: 2,
        },
    }
}

fn authority_limits() -> authority::Limits {
    authority::Limits {
        projects: 1,
        roles: 1,
        requirements: 1,
        facts: 2,
        grants: 1,
        executors: 1,
        segments: 1,
        segment_bytes: 32,
        implications: 0,
        batch: 1,
        accounts: 1,
        writes: 1,
        landing_rules: 1,
        gates: 1,
        approvals: 1,
        heads: 1,
        verdicts: 1,
        reviews: 1,
    }
}

fn authority_value(spend: u64, kinds: Box<[authority::Executor]>) -> authority::Authority {
    authority::Authority {
        tools: authority::Tools(0),
        grants: Box::new([]),
        delegation: authority::Delegation { kinds, tasks: 2, depth: 1 },
        budget: authority::Budget { spend, deadline: None },
        notes: authority::Scopes(0),
    }
}

/// Real authority policy and an authenticated owner, rather than allow flags.
#[must_use]
pub fn config(seed: u64) -> engine::Config {
    let ceiling = authority_value(1000, Box::new([authority::Executor::Charter(1)]));
    let rules = authority::Rules {
        ceiling: ceiling.clone(),
        period_spend: 1000,
        minimum_run_spend: 1,
        maximum_run_spend: 100,
        implies: authority::Implies::new(Box::new([]), 0).expect("empty implication configuration"),
        requirements: Box::new([]),
        landing: Box::new([]),
    };
    let mut domain = authority::Domain::new(rules, authority_limits()).expect("valid authority configuration");
    let policy = authority::Policy {
        ceiling,
        period_spend: 1000,
        roles: Box::new([authority::Role {
            number: 0,
            authority: authority_value(500, Box::new([authority::Executor::Charter(1)])),
            period_spend: 500,
            requests: authority::Requests::ALL,
            decides: authority::Proposals::ALL,
        }]),
        requirements: Box::new([]),
        landing: Box::new([]),
    };
    let mut out = Queue::with_capacity(authority::POLICY_MAX_OUT);
    authority::step(&mut domain, authority::Event::Policy { project: 1, policy }, &mut out);
    assert_eq!(out.pop(), Some(authority::PolicyFact::Added { project: 1 }), "real policy admitted");
    let mut chat_authority = authority_value(100, Box::new([]));
    chat_authority.delegation.tasks = 0;
    chat_authority.delegation.depth = 0;
    engine::Config {
        deployment: [31; 16],
        seed,
        owners: Box::new([people::InitialOwner { project: 1, identity: people::IdentityKey { forge: 1, user: 7 } }]),
        authority: domain,
        charter: 1,
        period: 1,
        period_budget: 1000,
        person_budget: 500,
        chat_authority,
        account: 1,
        account_generation: 1,
        account_valid: Some(Duration::from_secs(60)),
    }
}

/// Store latency and a lost-completion cut are deterministic world inputs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Deterministic child seed, replayed with the same external stimuli.
    pub seed: u64,
    /// Iterations a submitted commit waits before the fake store applies it.
    pub commit_delay: u32,
    /// Iterations a captured page waits before its load terminal arrives.
    pub page_delay: u32,
    /// Lose the first turn's commit completion and restart while the worker retains it.
    pub restart: bool,
    /// Drain observational child facts or leave their tiny queues saturated.
    pub facts: bool,
}

impl Settings {
    /// The required commit-cut restart story with tiny one-row pages.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings { seed, commit_delay: 0, page_delay: 0, restart: true, facts: true }
    }
}

/// Complete frozen world state between iterations; only the store and scripts
/// outlive replacement of the root process (domain/engine.md, section 15).
#[derive(Debug)]
pub struct World {
    settings: Settings,
    domain: engine::Domain,
    environment: Env<engine::Limits>,
    out: Queue<engine::Request>,
    events: VecDeque<(u32, engine::Event)>,
    /// Durable ordered fake store shared with the journal/load worlds.
    pub store: Store,
    /// Independent expectations derived from person and worker inputs.
    pub referee: WalkingReferee,
    /// Deterministic outside observations, excluding optional diagnostic facts.
    pub trace: Vec<String>,
    /// Exact number of injected process restarts.
    pub restarts: u32,
    /// Real paged store loads made by the root, including restart restoration.
    pub pages: u32,
    iteration: u32,
    commit_wait: u32,
    person_sent: bool,
    session: Option<u64>,
    assignment: Option<engine::Assignment>,
    pending_turn: Option<u32>,
}

impl World {
    /// Construct the real children and schedule startup plus an empty worker hello.
    #[must_use]
    pub fn new(settings: Settings) -> World {
        let limits = limits();
        let mut world = World {
            domain: engine::Domain::new(config(settings.seed), &limits),
            environment: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out: Queue::with_capacity(engine::max_out(&limits)),
            events: VecDeque::new(),
            store: Store::new(),
            referee: WalkingReferee::default(),
            trace: Vec::new(),
            restarts: 0,
            pages: 0,
            iteration: 0,
            commit_wait: 0,
            person_sent: false,
            session: None,
            assignment: None,
            pending_turn: None,
            settings,
        };
        world.queue(engine::Event::Start, 0);
        world.hello();
        world
    }

    fn queue(&mut self, event: engine::Event, delay: u32) {
        self.events.push_back((self.iteration + delay + 1, event));
    }

    fn hello(&mut self) {
        let hosting = self.assignment.as_ref().map_or_else(
            || Box::new([]) as Box<[fleet::Hosted]>,
            |assignment| {
                Box::new([fleet::Hosted {
                    run: Token::new(assignment.task),
                    attempt: Token::new(assignment.attempt),
                    phase: fleet::Phase::Active,
                }])
            },
        );
        self.queue(
            engine::Event::Hello {
                channel: Token::new(7),
                hello: fleet::Hello {
                    graces: Some(Duration::from_millis(100)),
                    slots: 1,
                    workstreams: Box::new([]),
                    hosting,
                },
            },
            0,
        );
    }

    fn turn(&mut self, number: u32) {
        let assignment = self.assignment.as_ref().expect("script has its real assignment");
        let (_, cumulative, transcript) = TURNS.iter().find(|(turn, _, _)| *turn == number).expect("scripted turn");
        let event = engine::Event::Turn {
            channel: Token::new(7),
            task: assignment.task,
            attempt: assignment.attempt,
            turn: engine::Turn { number, cumulative: *cumulative, read: None, transcript: (*transcript).into() },
        };
        self.pending_turn = Some(number);
        self.queue(event, 0);
    }

    fn restart(&mut self) {
        assert!(self.store.pending.is_empty(), "cut has no later submitted commit");
        self.domain = engine::Domain::new(config(self.settings.seed), &self.environment.limits);
        self.events.clear();
        self.restarts += 1;
        self.trace.push("restart after durable turn, before completion".into());
        self.queue(engine::Event::Start, 0);
        self.hello();
    }

    /// Drive one up/ready/timer/reclaim iteration and independent fake effects.
    pub fn iterate(&mut self) {
        self.iteration += 1;
        self.environment.now = Time::from_nanos(u64::from(self.iteration) * 1_000_000);
        self.environment.wall = Wall::from_nanos(u64::from(self.iteration) * 1_000_000);
        engine::resume(&mut self.domain, &self.environment, &mut self.out);
        self.observe();
        if self.events.front().is_some_and(|(due, _)| *due <= self.iteration) {
            let (_, event) = self.events.pop_front().expect("due input");
            self.trace.push(format!("input {event:?}"));
            engine::step(&mut self.domain, &self.environment, event, &mut self.out);
            self.observe();
        }
        engine::fire(&mut self.domain, &self.environment, &mut self.out);
        self.observe();
        if self.settings.facts {
            self.domain.drain_facts();
        }
        self.domain.reclaim();
        if !self.store.pending.is_empty() {
            if self.commit_wait > 0 {
                self.commit_wait -= 1;
            } else {
                let first_turn = self.store.pending.front().expect("pending commit").1.iter().any(
                    |write| matches!(write, temper_engine_domain::Write::Save(Record::Turn(turn)) if turn.turn == 1),
                );
                let number = self.store.apply();
                self.trace.push(format!("applied {number}"));
                self.commit_wait = self.settings.commit_delay;
                if self.settings.restart && self.restarts == 0 && first_turn {
                    self.restart();
                } else {
                    self.queue(engine::Event::Committed { number }, 0);
                }
            }
        }
        if self.domain.ready() && !self.person_sent {
            self.person_sent = true;
            self.queue(
                engine::Event::SignedIn {
                    reply_to: ReplyTo::new(Token::new(101)),
                    identity: people::Identity {
                        key: people::IdentityKey { forge: 1, user: 7 },
                        login: b"person".as_slice().into(),
                        name: b"Person".as_slice().into(),
                    },
                },
                0,
            );
        }
        if self.restarts != 0
            && self.domain.ready()
            && self.pending_turn == Some(1)
            && !self.events.iter().any(|(_, event)| matches!(event, engine::Event::Turn { .. }))
        {
            self.turn(1);
        }
    }

    fn observe(&mut self) {
        assert!(self.out.len() <= engine::max_out(&self.environment.limits));
        while let Some(request) = self.out.pop() {
            self.trace.push(format!("output {request:?}"));
            match request {
                engine::Request::Commit { number, writes } => {
                    self.referee.commit(&writes).expect("independent transaction referee");
                    if self.store.pending.is_empty() {
                        self.commit_wait = self.settings.commit_delay;
                    }
                    self.store.pending.push_back((number, writes));
                }
                engine::Request::Load { owner, range, after, most, .. } => {
                    self.pages += 1;
                    let (rows, next) = self.store.page(range, after, most);
                    self.queue(engine::Event::Loaded { owner, rows, next }, self.settings.page_delay);
                }
                engine::Request::Deliver(delivery) => self.delivery(delivery),
                engine::Request::Account(accounts::Request::Refresh { account, generation }) => {
                    self.queue(engine::Event::Refreshed { account, generation, valid: Duration::from_secs(60) }, 0);
                }
                engine::Request::Account(
                    accounts::Request::Keep { .. }
                    | accounts::Request::Cancel { .. }
                    | accounts::Request::Granted { .. }
                    | accounts::Request::Availability { .. }
                    | accounts::Request::Refused { .. }
                    | accounts::Request::Closed { .. },
                ) => {}
                engine::Request::TurnBusy { turn, .. } => self.turn(turn),
                engine::Request::AnswerBusy { channel, task, attempt } => self.queue(
                    engine::Event::Answer {
                        channel,
                        task,
                        attempt,
                        cumulative: FINAL_SPEND,
                        end: tasks::End::Finished {
                            result: tasks::Result::Report { words: REPORT.into() },
                            cancel_delegates: false,
                        },
                    },
                    1,
                ),
                engine::Request::Stop => panic!("walking story stopped: {:?}", self.trace),
            }
        }
    }

    fn delivery(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::WebReply { sign_in, reply: people::Reply::SignedIn { person, .. }, .. } => {
                let sign_in = sign_in.expect("web sign-in receives its new session");
                self.referee.signed_in(&self.store.rows, person, sign_in).expect("durable sign-in");
                self.session = Some(sign_in);
                self.queue(
                    engine::Event::Ask {
                        reply_to: ReplyTo::new(Token::new(102)),
                        sign_in,
                        key: [5; 16],
                        ask: people::Ask::StartChat { project: 1, words: QUESTION.into() },
                    },
                    0,
                );
            }
            Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::Started { task }), .. } => {
                self.referee.started(&self.store.rows, task).expect("atomic chat task and answer");
            }
            Delivery::Assigned { channel, assignment } => {
                assert_eq!(channel, Token::new(7));
                self.referee.assigned(&self.store.rows, &assignment).expect("committed claim and real brief");
                self.assignment = Some(assignment);
                self.turn(1);
            }
            Delivery::AcknowledgeTurn { channel, task, attempt, turn } => {
                assert_eq!(channel, Token::new(7));
                self.referee.turn_ack(&self.store.rows, task, attempt, turn).expect("durable atomic turn ACK");
                if self.pending_turn != Some(turn) {
                    return;
                }
                self.pending_turn = None;
                if turn == 1 {
                    self.turn(2);
                } else {
                    self.queue(
                        engine::Event::Answer {
                            channel,
                            task,
                            attempt,
                            cumulative: FINAL_SPEND,
                            end: tasks::End::Finished {
                                result: tasks::Result::Report { words: REPORT.into() },
                                cancel_delegates: false,
                            },
                        },
                        0,
                    );
                }
            }
            Delivery::Acknowledge { channel, task, attempt } => {
                assert_eq!(channel, Token::new(7));
                self.referee.answer_ack(&self.store.rows, task, attempt).expect("priced terminal durable before ACK");
            }
            Delivery::TurnBusy { turn, .. } => self.turn(turn),
            Delivery::Result { person, task, words } => self
                .referee
                .result(&self.store.rows, person, task, &words)
                .expect("committed result reaches person once"),
            Delivery::ResultReply { .. } => panic!("unsolicited historical result reply"),
            Delivery::Reply { .. } | Delivery::WebReply { .. } | Delivery::Refuse { .. } | Delivery::Cancel { .. } => {
                panic!("unexpected walking delivery {delivery:?}")
            }
            Delivery::Fleet(_) | Delivery::ReadResult { .. } | Delivery::Load { .. } => {
                panic!("internal handoff leaked to world")
            }
        }
    }

    /// Settle every obligation within a deterministic iteration bound.
    pub fn run(&mut self) {
        for _ in 0..600 {
            if self.settled() {
                return;
            }
            self.iterate();
        }
        panic!("walking story did not settle: {:?}", self.trace);
    }

    fn settled(&self) -> bool {
        self.referee.done()
            && self.events.is_empty()
            && self.store.pending.is_empty()
            && self.out.is_empty()
            && self.domain.quiescent()
    }
}

/// Replay every frozen root, child, store and pending script state in lockstep,
/// including diagnostic queues with identical observation settings
/// (domain/engine.md, section 15; testing-strategy.md, section 6).
#[must_use]
pub fn run_replayed(settings: Settings) -> World {
    let mut first = World::new(settings);
    let mut replay = World::new(settings);
    for _ in 0..600 {
        if first.settled() {
            assert!(replay.settled());
            return first;
        }
        first.iterate();
        replay.iterate();
        assert_eq!(
            format!("{first:?}"),
            format!("{replay:?}"),
            "complete frozen state at iteration {}",
            first.iteration
        );
    }
    panic!("walking replay did not settle");
}

#[cfg(test)]
mod tests {
    use super::{Settings, World};
    use crate::walking_referee::REPORT;
    use skein_lib::Queue;
    use temper_engine_domain::{
        Delivery,
        engine::{self, Request},
    };

    #[test]
    #[should_panic(expected = "committed result reaches person once: \"person received result twice\"")]
    fn a_queued_late_result_prevents_settlement_and_reaches_the_referee() {
        let mut world = World::new(Settings::calm(66));
        world.run();
        let person = world.store.header().people;
        let task = world.assignment.as_ref().expect("script retained assignment").task;
        world.out = Queue::with_capacity(engine::max_out(&world.environment.limits) + 1);
        world.out.push(Request::Deliver(Delivery::Result { person, task, words: REPORT.into() }));
        assert!(!world.settled(), "queued outward work cannot be hidden by completed story counters");
        world.run();
    }
}

use skein_lib::{Queue, Rng, Token, Wall};
use std::collections::{BTreeMap, VecDeque};
use temper_engine_domain::{
    self as root, Decision, Delivery, Deployment, Family, Journal, JournalLimits, Key, Output, Range, Record,
    TurnRecord, Write,
};

pub const LIMITS: JournalLimits =
    JournalLimits { commits: 3, held: 12, writes: 4, deliveries: 4, transcript_bytes: 128, result_bytes: 128 };

pub const HEADER: Deployment = Deployment {
    id: [31; 16],
    tasks: 0,
    people: 0,
    sign_ins: 0,
    messages: 0,
    runs: 0,
    calls: 0,
    forge_rows: 0,
    commits: 0,
};

/// Only the domain's storage contract: numbered transactions apply whole in
/// order. A completion can be lost after applying; a fresh process sees it.
#[derive(Debug)]
pub struct Store {
    pub applied: u64,
    pub rows: BTreeMap<Key, Record>,
    pub pending: VecDeque<(u64, Box<[Write]>)>,
}

impl Store {
    #[must_use]
    pub fn new() -> Store {
        Store {
            applied: 0,
            rows: BTreeMap::from([(Key::Deployment, Record::Deployment(HEADER))]),
            pending: VecDeque::new(),
        }
    }

    pub fn apply(&mut self) -> u64 {
        let (number, writes) = self.pending.pop_front().expect("queued transaction");
        assert_eq!(number, self.applied + 1, "fake store commits in order");
        // A store application is one synchronous fake operation: no engine or
        // referee observes its rows until the whole batch has been applied.
        // Moving writes into the map avoids copying every older row per commit.
        for write in writes {
            match write {
                Write::Save(row) => {
                    self.rows.insert(row.key(), row);
                }
                Write::Erase(key) => {
                    self.rows.remove(&key);
                }
            }
        }
        let Some(Record::Deployment(header)) = self.rows.get(&Key::Deployment) else {
            panic!("durable deployment");
        };
        assert_eq!(header.commits, number, "header and records applied atomically");
        self.applied = number;
        number
    }

    #[must_use]
    pub fn header(&self) -> Deployment {
        let Some(Record::Deployment(header)) = self.rows.get(&Key::Deployment) else {
            panic!("deployment");
        };
        *header
    }

    /// Key-range paging, derived directly from the fake store's durable map.
    /// Each page is a separate request and has a cursor only when rows remain.
    #[must_use]
    pub fn page(&self, range: Range, after: Option<Key>, most: u32) -> (Box<[Record]>, Option<Key>) {
        let count = usize::try_from(most).expect("small page");
        let mut selected: Vec<Record> = self.rows.iter().filter(|(key, _)| {
            let within = match range {
                Range::Calls => matches!(key, Key::Call(call) if call.task != 0 && call.attempt != 0 && call.completion != 0),
                Range::ProposalDecision { proposal } => matches!(key, Key::ProposalDecision(number) if *number == proposal),
                Range::EscalationDecision { task, revision } => matches!(key, Key::EscalationDecision { task: found, revision: current } if *found == task && *current == revision),
                Range::Deployment => **key == Key::Deployment,
                Range::Tasks => match key {
                    Key::Tasks(temper_engine_domain_tasks::Key::Live(_) | temper_engine_domain_tasks::Key::Ledger(_) | temper_engine_domain_tasks::Key::PersonProposal(_)) => true,
                    Key::Tasks(temper_engine_domain_tasks::Key::Ended(_) | temper_engine_domain_tasks::Key::History { .. })
                        | Key::Call(_) | Key::EscalationDecision { .. } | Key::ProposalDecision(_) | Key::Deployment | Key::Turn { .. } | Key::RunProof { .. } | Key::Terminal { .. } | Key::People(_) | Key::Forge(_) => false,
                },
                Range::EndedResults => matches!(key, Key::Tasks(temper_engine_domain_tasks::Key::Ended(number)) if *number != 0),
                Range::RunProofs => matches!(key, Key::RunProof { task } if *task != 0),
                Range::People => matches!(key, Key::People(_)),
                Range::Forge => matches!(key, Key::Forge(_)),
                Range::TaskResult { task } => matches!(key, Key::Tasks(temper_engine_domain_tasks::Key::Ended(number)) if *number == task),
                Range::Turns { task, attempt } => matches!(key, Key::Turn { task: found, attempt: run, turn } if *found == task && *run == attempt && *turn != 0),
                Range::TaskTranscript { task } => matches!(key, Key::Turn { task: found, attempt, turn } if *found == task && *attempt != 0 && *turn != 0),
            };
            within && after.is_none_or(|old| **key > old)
        }).take(count.saturating_add(1)).map(|(_, row)| row.clone()).collect();
        let more = selected.len() > count;
        if more {
            selected.pop();
        }
        let rows: Box<[Record]> = selected.into_boxed_slice();
        let next = if more { Some(rows.last().expect("positive page").key()) } else { None };
        (rows, next)
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

/// Predicts from submitted stories, rather than from the journal's state or
/// its observations. Errors are used by focused tests to break each rule.
#[derive(Debug)]
pub struct Referee {
    header: Deployment,
    writes: VecDeque<Vec<Write>>,
    deliveries: VecDeque<(u64, u32)>,
    pub judged: u32,
}

impl Referee {
    #[must_use]
    pub fn new() -> Referee {
        Referee { header: HEADER, writes: VecDeque::new(), deliveries: VecDeque::new(), judged: 0 }
    }

    pub fn decision(&mut self, turn: u32, payload: &[u8], writing: bool, fresh: bool) {
        if fresh {
            self.header.tasks += 1;
        }
        if writing || fresh {
            self.header.commits += 1;
            let mut writes = vec![Write::Save(Record::Deployment(self.header))];
            if writing {
                writes.push(Write::Save(Record::Turn(TurnRecord {
                    task: 1,
                    attempt: 1,
                    turn,
                    spent: u64::from(turn),
                    read: Some(u64::from(turn)),
                    at: Wall::from_nanos(u64::from(turn)),
                    transcript: payload.into(),
                })));
            }
            self.writes.push_back(writes);
        }
        self.deliveries.push_back((self.header.commits, turn));
    }

    /// Check one observation against submitted decisions and durable storage.
    ///
    /// # Errors
    /// Names the violated commit, durability, ordering or exactly-once rule.
    pub fn observe(&mut self, store: &mut Store, output: Output) -> Result<(), &'static str> {
        match output {
            Output::Commit { number, writes } => {
                let expected = self.writes.pop_front().ok_or("unsolicited commit")?;
                let Write::Save(Record::Deployment(header)) = &expected[0] else {
                    panic!("expected header");
                };
                if number != header.commits {
                    return Err("commit number");
                }
                if writes.as_ref() != expected.as_slice() {
                    return Err("atomic records");
                }
                store.pending.push_back((number, writes));
            }
            Output::Deliver(delivery) => {
                let Delivery::AcknowledgeTurn { channel, task, attempt, turn } = delivery else {
                    return Err("delivery shape");
                };
                let (after, expected) = self.deliveries.front().copied().ok_or("duplicate delivery")?;
                if store.applied < after {
                    return Err("before durability");
                }
                if (channel, task, attempt, turn) != (Token::new(7), 1, 1, expected) {
                    return Err("delivery order or fence");
                }
                self.deliveries.pop_front();
            }
            Output::Stop => return Err("unexpected stop"),
        }
        self.judged += 1;
        Ok(())
    }

    #[must_use]
    pub fn done(&self) -> bool {
        self.writes.is_empty() && self.deliveries.is_empty()
    }
}

impl Default for Referee {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct World {
    pub journal: Journal,
    pub store: Store,
    pub referee: Referee,
    out: Queue<Output>,
    pub trace: Vec<String>,
}

impl World {
    #[must_use]
    pub fn new() -> World {
        World {
            journal: Journal::new(HEADER, &LIMITS),
            store: Store::new(),
            referee: Referee::new(),
            out: Queue::with_capacity(1),
            trace: Vec::new(),
        }
    }

    pub fn decision(&mut self, turn: u32, payload: &[u8], writing: bool, fresh: bool) {
        assert!(root::takes(&self.journal, &LIMITS), "world reserves decision before mutation");
        self.referee.decision(turn, payload, writing, fresh);
        let mut decision = Decision::new(&LIMITS);
        if fresh {
            root::fresh(&mut self.journal, Family::Task).expect("fresh task number");
        }
        if writing {
            decision
                .write(
                    &LIMITS,
                    Write::Save(Record::Turn(TurnRecord {
                        task: 1,
                        attempt: 1,
                        turn,
                        spent: u64::from(turn),
                        read: Some(u64::from(turn)),
                        at: Wall::from_nanos(u64::from(turn)),
                        transcript: payload.into(),
                    })),
                )
                .expect("bounded transcript");
        }
        decision
            .deliver(&LIMITS, Delivery::AcknowledgeTurn { channel: Token::new(7), task: 1, attempt: 1, turn })
            .expect("delivery room");
        root::accept(&mut self.journal, &LIMITS, decision, &mut self.out).expect("reserved whole decision");
        self.observe();
    }

    pub fn apply(&mut self, answer: bool) {
        let number = self.store.apply();
        self.trace.push(format!("applied {number}"));
        if answer {
            root::committed(&mut self.journal, number);
        }
    }

    pub fn ready(&mut self) {
        root::resume(&mut self.journal, &mut self.out);
        self.observe();
    }

    fn observe(&mut self) {
        assert!(self.out.len() <= 1, "one output per ready pass");
        while let Some(output) = self.out.pop() {
            self.trace.push(format!("{output:?}"));
            self.referee.observe(&mut self.store, output).expect("independent referee");
        }
    }

    pub fn settle(&mut self) {
        while !self.store.pending.is_empty() {
            self.apply(true);
        }
        while self.journal.ready() {
            self.ready();
        }
        assert!(self.referee.done(), "all submitted obligations settled");
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn random(seed: u64) -> World {
    let mut world = World::new();
    let mut rng = Rng::new(seed);
    for turn in 1_u32..=24 {
        while !root::takes(&world.journal, &LIMITS) {
            if world.journal.ready() {
                world.ready();
            } else {
                world.apply(true);
            }
        }
        let payload =
            vec![u8::try_from(turn).expect("small turn"); usize::try_from(rng.below(127) + 1).expect("small payload")];
        world.decision(turn, &payload, rng.below(3) != 0, rng.below(5) == 0);
        if !world.store.pending.is_empty() && rng.below(2) == 0 {
            world.apply(true);
        }
        if rng.below(2) == 0 {
            world.ready();
        }
    }
    world.settle();
    world
}

//! What the views' scenarios expect, held by a referee (testing-pyramid.md,
//! 5.2) that sees what the parent tells the views, what they emit, and what
//! the store keeps and forgets, never the views' state:
//!
//! - a watch is refused exactly when its run is not followed, or as busy
//!   exactly when as many watches are open as the limits allow;
//! - a watcher never sees chunks out of order or twice, and is told exactly
//!   how many it missed, right before the chunks it catches up from: what it
//!   is delivered, less what it was told it missed, is what it watches, in
//!   the order the views took it, from its watch to its end;
//! - a watcher has one delivery in flight, takes all that waits in each, and
//!   its watch ends only once that delivery has, for the reason it has: the
//!   person stopped watching, or its run finished and it has had all that
//!   waited;
//! - the store keeps, in the order reported, only what the runs' policies
//!   keep of their reports, each with when it was reported; and the views
//!   never ask it to forget what is within the retention;
//! - liveness: a chunk the views take while its watcher is caught up
//!   reaches it within a bound; a watch ends within a bound of the person
//!   stopping or its run finishing; and nothing is kept past its retention,
//!   beyond a sweep and the store's time, the store permitting.

use std::collections::{BTreeMap, VecDeque};

use temper_engine_model_views::{Capture, Chunk, End, Kind, Limits, Phase, Policy, Record, Refusal, Subject};
use temper_lib::{Duration, Time, Token};
use temper_world::{Expectations, Judge};

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The views take what the parent tells them.
    Started {
        run: u64,
        item: u64,
        policy: Policy,
    },
    Reported {
        run: u64,
        kind: Kind,
        content: Vec<u8>,
    },
    Finished {
        run: u64,
    },
    Phase {
        item: u64,
        repository: u32,
        phase: Phase,
    },
    Watch {
        watcher: u64,
        subject: Subject,
    },
    Unwatch {
        watcher: u64,
    },
    /// The person's stream took the delivery in flight to `watcher`, as the
    /// views learn it.
    Delivered {
        watcher: u64,
    },
    /// The views answer a watch, deliver to it, or end it.
    Watching {
        watcher: u64,
    },
    Refused {
        watcher: u64,
        refusal: Refusal,
    },
    Deliver {
        watcher: u64,
        missed: u64,
        chunks: Vec<Chunk>,
    },
    Ended {
        watcher: u64,
        end: End,
    },
    /// The views ask the store to forget what was reported before `before`.
    Expire {
        before: Time,
    },
    /// The store kept these records of the append it names `append`: all
    /// that were sent, or the first of them.
    Kept {
        append: u64,
        records: Vec<Record>,
    },
    /// The store forgot what was reported before `before`, or failed to.
    Forgot {
        before: Time,
        done: bool,
    },
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// The chunk the views took for the watcher as its `chunk`-th reaches
    /// it.
    Reach { watcher: u64, chunk: u64 },
    /// The watch ends.
    End(u64),
    /// What the store kept of the append is gone.
    Gone(u64),
}

/// This world injects nothing of its own.
#[derive(Debug)]
pub enum Stimulus {}

/// How long the views have to do what the referee expects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounds {
    /// A chunk taken while its watcher is caught up, to reach it.
    pub reach: Duration,
    /// A watch, to end once the person stops or its run finishes.
    pub end: Duration,
    /// Beyond the retention, for the store to forget a record: a sweep and
    /// the store's time to come to it.
    pub forget: Duration,
}

/// The expectations of the views' world.
#[derive(Debug)]
pub struct Views {
    limits: Limits,
    bounds: Bounds,
    /// The runs the views follow, by their own count, with each one's item
    /// and policy.
    runs: BTreeMap<u64, (u64, Policy)>,
    /// The watches open, from the views' answer to their end.
    watchers: BTreeMap<u64, Watch>,
    /// The answer due to the watch being taken.
    answer: Option<(u64, Subject, Option<Refusal>)>,
    /// What the store is to keep, in the order the views took it, until it
    /// keeps it or a later record shows it lost.
    traced: VecDeque<Record>,
    /// When each record the store holds was reported, by append.
    kept: BTreeMap<u64, Vec<Time>>,
    /// Chunks, deliveries, missed chunks and records judged.
    pub chunks: u64,
    pub deliveries: u64,
    pub missed: u64,
    pub records: u64,
}

#[derive(Debug)]
struct Watch {
    subject: Subject,
    /// Whether it still takes chunks: not once the person stopped, or its
    /// run finished; and why it is to end.
    taking: bool,
    ending: Option<End>,
    /// What it is to be delivered, with each chunk's number, oldest first;
    /// and the numbers of those in the delivery in flight.
    waiting: VecDeque<(u64, Chunk)>,
    in_flight: Option<Vec<u64>>,
    chunks: u64,
}

impl Views {
    #[must_use]
    pub fn new(limits: Limits, bounds: Bounds) -> Views {
        Views {
            limits,
            bounds,
            runs: BTreeMap::new(),
            watchers: BTreeMap::new(),
            answer: None,
            traced: VecDeque::new(),
            kept: BTreeMap::new(),
            chunks: 0,
            deliveries: 0,
            missed: 0,
            records: 0,
        }
    }

    /// The chunk taken at `now` goes to each watcher of `first` or `second`
    /// that takes chunks; one caught up is to have it within the bound.
    fn offer(&mut self, first: Subject, second: Subject, chunk: &Chunk, judge: &mut Judge<Expected, Stimulus>) {
        for (&watcher, watch) in &mut self.watchers {
            if !watch.taking || (watch.subject != first && watch.subject != second) {
                continue;
            }
            watch.chunks += 1;
            if watch.in_flight.is_none() && watch.waiting.is_empty() {
                judge.expect(Expected::Reach { watcher, chunk: watch.chunks }, self.bounds.reach);
            }
            watch.waiting.push_back((watch.chunks, copy(chunk)));
        }
    }

    fn reported(&mut self, run: u64, kind: Kind, content: Vec<u8>, judge: &mut Judge<Expected, Stimulus>) {
        let size = u32::try_from(content.len()).expect("small");
        let Some(&(item, policy)) = self.runs.get(&run) else {
            return;
        };
        if size > self.limits.report_bytes {
            return;
        }
        let at = judge.now();
        let chunk = Chunk::Report { run: Token::new(run), kind, at, content: content.clone().into() };
        self.offer(Subject::Run(Token::new(run)), Subject::Item(Token::new(item)), &chunk, judge);
        let content = match policy.capture(kind) {
            Capture::Nothing => return,
            Capture::Shape => None,
            Capture::Content => Some(content.into()),
        };
        self.traced.push_back(Record { run: Token::new(run), kind, at, size, content });
    }

    fn watch(&mut self, watcher: u64, subject: Subject, judge: &mut Judge<Expected, Stimulus>) {
        judge.check(self.answer.is_none(), format_args!("watch {watcher}: the last watch was answered at once"));
        let unknown = match subject {
            Subject::Run(run) => !self.runs.contains_key(&run.raw()),
            Subject::Item(_) | Subject::Board(_) => false,
        };
        let busy = self.watchers.len() >= usize::try_from(self.limits.watchers).expect("small");
        let refusal = if unknown {
            Some(Refusal::Unknown)
        } else if busy {
            Some(Refusal::Busy)
        } else {
            None
        };
        self.answer = Some((watcher, subject, refusal));
    }

    fn answered(&mut self, watcher: u64, refusal: Option<Refusal>, judge: &mut Judge<Expected, Stimulus>) {
        let Some((asked, subject, due)) = self.answer.take() else {
            judge.fail(format_args!("watch {watcher}: answered once, when it was taken"));
            return;
        };
        judge.check(asked == watcher, format_args!("watch {watcher}: answered when it was taken"));
        judge.check(refusal == due, format_args!("watch {watcher}: answered {refusal:?}, not {due:?}"));
        if refusal.is_none() {
            let watch =
                Watch { subject, taking: true, ending: None, waiting: VecDeque::new(), in_flight: None, chunks: 0 };
            self.watchers.insert(watcher, watch);
        }
    }

    /// The watch is to end, for `end`, if it is open and has not been told
    /// so already.
    fn ending(&mut self, watcher: u64, end: End, judge: &mut Judge<Expected, Stimulus>) {
        let Some(watch) = self.watchers.get_mut(&watcher) else {
            return;
        };
        watch.taking = false;
        if watch.ending.is_none() {
            watch.ending = Some(end);
            judge.expect(Expected::End(watcher), self.bounds.end);
        }
        if end == End::Unwatched {
            // What waits for it is dropped, and its reaching withdrawn.
            for (chunk, _) in watch.waiting.drain(..) {
                judge.withdraw(&Expected::Reach { watcher, chunk });
            }
            watch.ending = Some(End::Unwatched);
        }
    }

    fn deliver(&mut self, watcher: u64, missed: u64, chunks: Vec<Chunk>, judge: &mut Judge<Expected, Stimulus>) {
        let Some(watch) = self.watchers.get_mut(&watcher) else {
            judge.fail(format_args!("watcher {watcher}: delivered to while its watch is open"));
            return;
        };
        judge.check(watch.in_flight.is_none(), format_args!("watcher {watcher}: one delivery in flight"));
        judge.check(
            watch.ending != Some(End::Unwatched),
            format_args!("watcher {watcher}: nothing more delivered once the person stopped watching"),
        );
        judge.check(!chunks.is_empty(), format_args!("watcher {watcher}: a delivery delivers something"));
        self.deliveries += 1;
        self.missed += missed;
        let waiting = u64::try_from(watch.waiting.len()).expect("small");
        let told = missed + u64::try_from(chunks.len()).expect("small");
        if told != waiting {
            judge.fail(format_args!(
                "watcher {watcher}: a delivery takes all that waits, told missed or delivered: {missed} missed and {} \
                 delivered of {waiting} waiting",
                chunks.len()
            ));
            return;
        }
        for (chunk, _) in watch.waiting.drain(..usize::try_from(missed).expect("small")) {
            let reaching = judge.is_pending(&Expected::Reach { watcher, chunk });
            judge.check(
                !reaching,
                format_args!("watcher {watcher}: chunk {chunk}, taken while it was caught up, is never missed"),
            );
        }
        let mut in_flight = Vec::new();
        for delivered in chunks {
            let (number, due) = watch.waiting.pop_front().expect("as many waiting as told");
            self.chunks += 1;
            judge.check(
                delivered == due,
                format_args!(
                    "watcher {watcher}: chunk {number} is delivered in order, once, after what it was told it \
                     missed: {delivered:?} is not {due:?}"
                ),
            );
            in_flight.push(number);
        }
        watch.in_flight = Some(in_flight);
    }

    fn delivered(&mut self, watcher: u64, judge: &mut Judge<Expected, Stimulus>) {
        let Some(watch) = self.watchers.get_mut(&watcher) else {
            return;
        };
        for chunk in watch.in_flight.take().unwrap_or_default() {
            judge.meet(&Expected::Reach { watcher, chunk });
        }
    }

    fn ended(&mut self, watcher: u64, end: End, judge: &mut Judge<Expected, Stimulus>) {
        let Some(watch) = self.watchers.remove(&watcher) else {
            judge.fail(format_args!("watcher {watcher}: ends once, while its watch is open"));
            return;
        };
        judge.meet(&Expected::End(watcher));
        judge.check(watch.in_flight.is_none(), format_args!("watcher {watcher}: ends once its delivery has"));
        judge.check(
            watch.ending == Some(end),
            format_args!("watcher {watcher}: ends {end:?} when it is due to end {:?}", watch.ending),
        );
        if end == End::Finished {
            judge
                .check(watch.waiting.is_empty(), format_args!("watcher {watcher}: has all that waited before it ends"));
        }
    }

    fn kept(&mut self, append: u64, records: Vec<Record>, judge: &mut Judge<Expected, Stimulus>) {
        let mut times = Vec::new();
        for record in records {
            self.records += 1;
            // What the views lost, or the store did not take, is skipped.
            let Some(at) = self.traced.iter().position(|due| *due == record) else {
                judge.fail(format_args!(
                    "append {append}: keeps only what the policies keep, in order, once: {record:?} was not due"
                ));
                return;
            };
            self.traced.drain(..=at);
            times.push(record.at);
        }
        let Some(newest) = times.iter().max() else {
            return;
        };
        // A record the store took after its retention is gone a sweep later.
        let due = newest.saturating_add(self.limits.retention).max(judge.now()).saturating_add(self.bounds.forget);
        judge.expect(Expected::Gone(append), due.saturating_since(judge.now()));
        self.kept.insert(append, times);
    }

    fn forgot(&mut self, before: Time, done: bool, judge: &mut Judge<Expected, Stimulus>) {
        let mut gone = Vec::new();
        for (&append, times) in &mut self.kept {
            if done {
                times.retain(|at| *at >= before);
                if times.is_empty() {
                    gone.push(append);
                }
            } else if times.iter().all(|at| *at < before) {
                // The next sweep is to cover it.
                judge.rearm(Expected::Gone(append), self.bounds.forget);
            }
        }
        for append in gone {
            self.kept.remove(&append);
            judge.meet(&Expected::Gone(append));
        }
    }
}

impl Expectations for Views {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        match seen {
            Seen::Started { run, item, policy } => {
                let room = self.runs.len() < usize::try_from(self.limits.runs).expect("small");
                if room || self.runs.contains_key(&run) {
                    self.runs.insert(run, (item, policy));
                }
            }
            Seen::Reported { run, kind, content } => self.reported(run, kind, content, judge),
            Seen::Finished { run } => {
                if self.runs.remove(&run).is_some() {
                    let watchers: Vec<u64> = self
                        .watchers
                        .iter()
                        .filter(|(_, watch)| watch.subject == Subject::Run(Token::new(run)))
                        .map(|(&watcher, _)| watcher)
                        .collect();
                    for watcher in watchers {
                        self.ending(watcher, End::Finished, judge);
                    }
                }
            }
            Seen::Phase { item, repository, phase } => {
                let chunk = Chunk::Phase { item: Token::new(item), phase, at: judge.now() };
                self.offer(Subject::Item(Token::new(item)), Subject::Board(repository), &chunk, judge);
            }
            Seen::Watch { watcher, subject } => self.watch(watcher, subject, judge),
            Seen::Unwatch { watcher } => self.ending(watcher, End::Unwatched, judge),
            Seen::Delivered { watcher } => self.delivered(watcher, judge),
            Seen::Watching { watcher } => self.answered(watcher, None, judge),
            Seen::Refused { watcher, refusal } => self.answered(watcher, Some(refusal), judge),
            Seen::Deliver { watcher, missed, chunks } => self.deliver(watcher, missed, chunks, judge),
            Seen::Ended { watcher, end } => self.ended(watcher, end, judge),
            Seen::Expire { before } => {
                let past = judge.now().as_nanos().saturating_sub(self.limits.retention.as_nanos());
                judge.check(
                    before.as_nanos() <= past,
                    format_args!("an expire forgets nothing within the retention: {before:?}"),
                );
            }
            Seen::Kept { append, records } => self.kept(append, records, judge),
            Seen::Forgot { before, done } => self.forgot(before, done, judge),
        }
    }
}

/// A copy of `chunk`.
#[must_use]
pub fn copy(chunk: &Chunk) -> Chunk {
    match chunk {
        Chunk::Report { run, kind, at, content } => {
            Chunk::Report { run: *run, kind: *kind, at: *at, content: content.clone() }
        }
        Chunk::Phase { item, phase, at } => Chunk::Phase { item: *item, phase: *phase, at: *at },
    }
}

/// A copy of `record`.
#[must_use]
pub fn copy_record(record: &Record) -> Record {
    Record { run: record.run, kind: record.kind, at: record.at, size: record.size, content: record.content.clone() }
}

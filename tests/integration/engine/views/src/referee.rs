//! What the views' scenarios expect, held by a referee (testing-pyramid.md,
//! 5.2) that sees what the parent tells the views, what they emit, and what
//! the store keeps and forgets, never the views' state:
//!
//! - a watch is refused exactly when its run is not followed (as turned
//!   away, if it was and is remembered), its snapshot is past the limits, or
//!   as many watches are open as the limits allow; or as busy when the
//!   watches ended at this instant may still hold every slot;
//! - a watcher's first delivery is its snapshot; it never sees chunks out of
//!   order or twice: each delivery holds the newest of the chunks that came
//!   since the last, and tells exactly how many it missed since the last it
//!   took, of those, of what a delivery it did not take held, and of reports
//!   dropped that it would have had;
//! - a watcher has one delivery in flight, takes all that waits in each, and
//!   its watch ends only once that delivery has, for the reason it has: the
//!   person stopped watching, or its run finished and it has had all that
//!   waited;
//! - the store keeps, in the order reported, only what the runs' policies
//!   keep of their reports, each with its attempt and when it was reported;
//!   and the views never ask it to forget what is within the retention;
//! - liveness: a chunk the views take while its watcher is caught up, or
//!   that waits when its watcher's delivery ends, reaches it within a bound,
//!   unless the delivery carrying it is not taken; a watch ends within a
//!   bound of the person stopping or its run finishing; and nothing is kept
//!   past its retention, beyond a sweep and the store's time, the store
//!   permitting, across restarts of the engine too.
//!
//! It also counts what the views should count as lost, for the world to
//! hold their counters to.

use std::collections::{BTreeMap, VecDeque};

use temper_engine_model_views::{Capture, Chunk, End, Kind, Limits, Policy, Record, Refusal, Subject};
use temper_lib::{Duration, Time, Token};
use temper_world::{Expectations, Judge};

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The views take what the parent tells them.
    Started {
        run: u64,
        attempt: u64,
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
        phase: u32,
    },
    Watch {
        watcher: u64,
        subject: Subject,
        snapshot: Vec<u8>,
    },
    Unwatch {
        watcher: u64,
    },
    /// The delivery in flight to `watcher` ended, taken by the person's
    /// stream or not, as the views learn it.
    Delivered {
        watcher: u64,
        done: bool,
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
    /// The engine restarted: the views start anew, every watch is gone with
    /// its stream, and the store keeps what it holds.
    Restarted,
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// The chunk the views took for the watcher as its `chunk`-th reaches
    /// it.
    Reach { watcher: u64, chunk: u64 },
    /// What waits for the watcher goes out, once its delivery has ended.
    Next(u64),
    /// The watch ends.
    End(u64),
    /// What the store kept of the append is gone.
    Gone(u64),
}

/// What the referee injects that belongs to no fake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stimulus {
    /// The engine restarts.
    Restart,
}

/// How long the views have to do what the referee expects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounds {
    /// A chunk, to reach its watcher once it goes out: the slowest stream,
    /// or the parent's bound on a delivery.
    pub reach: Duration,
    /// A watch, to end once the person stops or its run finishes.
    pub end: Duration,
    /// Beyond the retention, for the store to forget a record: a sweep and
    /// the store's time to come to it.
    pub forget: Duration,
}

/// What the views should count as lost, as the referee saw it.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    /// Runs turned away, and reports dropped.
    pub runs: u64,
    pub reports: u64,
    /// Chunks the referee saw lost; and those dropped from what waited when
    /// a watch stopped or the engine restarted, some of which the views may
    /// have counted lost before.
    pub chunks: u64,
    pub abandoned: u64,
    /// Records the policies keep, taken by the views.
    pub traced: u64,
}

/// The expectations of the views' world.
#[derive(Debug)]
pub struct Views {
    limits: Limits,
    bounds: Bounds,
    /// The runs the views follow, by their own count, with each one's
    /// attempt, item and policy; and those turned away that they remember,
    /// with their items.
    runs: BTreeMap<u64, (u64, u64, Policy)>,
    unfollowed: BTreeMap<u64, u64>,
    /// The watches open, from the views' answer to their end; and when the
    /// last ended, with how many ended then.
    watchers: BTreeMap<u64, Watch>,
    ended: (Time, u64),
    /// The answer due to the watch being taken.
    answer: Option<Answer>,
    /// What the store is to keep, in the order the views took it, until it
    /// keeps it or a later record shows it lost.
    traced: VecDeque<Record>,
    /// When each record the store holds was reported, by append.
    kept: BTreeMap<u64, Vec<Time>>,
    pub tally: Tally,
    /// Chunks, deliveries, missed chunks and records judged.
    pub chunks: u64,
    pub deliveries: u64,
    pub missed: u64,
    pub records: u64,
}

/// A watch being taken: its snapshot, and its answer due; or busy too, when
/// the slots may all be held.
#[derive(Debug)]
struct Answer {
    watcher: u64,
    subject: Subject,
    snapshot: Vec<u8>,
    due: Option<Refusal>,
    or_busy: bool,
}

#[derive(Debug)]
struct Watch {
    subject: Subject,
    /// Whether it still takes chunks: not once the person stopped, or its
    /// run finished; and why it is to end.
    taking: bool,
    ending: Option<End>,
    /// What came for it since its last delivery, with each chunk's number,
    /// oldest first; how many it missed besides; and the numbers of the
    /// chunks in the delivery in flight, with the missed it told.
    waiting: VecDeque<(u64, Chunk)>,
    misses: u64,
    in_flight: Option<(Vec<u64>, u64)>,
    chunks: u64,
}

impl Views {
    #[must_use]
    pub fn new(limits: Limits, bounds: Bounds) -> Views {
        Views {
            limits,
            bounds,
            runs: BTreeMap::new(),
            unfollowed: BTreeMap::new(),
            watchers: BTreeMap::new(),
            ended: (Time::ZERO, 0),
            answer: None,
            traced: VecDeque::new(),
            kept: BTreeMap::new(),
            tally: Tally::default(),
            chunks: 0,
            deliveries: 0,
            missed: 0,
            records: 0,
        }
    }

    fn room(&self) -> usize {
        usize::try_from(self.limits.runs).expect("small")
    }

    fn started(&mut self, run: u64, attempt: u64, item: u64, policy: Policy) {
        if self.runs.contains_key(&run) || self.runs.len() < self.room() {
            self.runs.insert(run, (attempt, item, policy));
            self.unfollowed.remove(&run);
            return;
        }
        self.tally.runs += 1;
        if self.unfollowed.contains_key(&run) || self.unfollowed.len() < self.room() {
            self.unfollowed.insert(run, item);
        }
    }

    /// A chunk taken at `now` goes to each watcher of `first` or `second`:
    /// one that takes chunks has it, and one caught up is to have it within
    /// the bound; one whose run finished counts it missed.
    fn offer(&mut self, first: Subject, second: Subject, chunk: &Chunk, judge: &mut Judge<Expected, Stimulus>) {
        for (&watcher, watch) in &mut self.watchers {
            if watch.subject != first && watch.subject != second {
                continue;
            }
            if !watch.taking {
                if watch.ending == Some(End::Finished) {
                    watch.misses += 1;
                    self.tally.chunks += 1;
                }
                continue;
            }
            watch.chunks += 1;
            if watch.in_flight.is_none() && watch.waiting.is_empty() {
                judge.expect(Expected::Reach { watcher, chunk: watch.chunks }, self.bounds.reach);
            }
            watch.waiting.push_back((watch.chunks, copy(chunk)));
        }
    }

    /// A report each watcher of `first` or `second` would have had was
    /// dropped: they miss it, unless they stopped watching.
    fn miss(&mut self, first: Subject, second: Subject) {
        for watch in self.watchers.values_mut() {
            let stopped = watch.ending == Some(End::Unwatched);
            if (watch.subject == first || watch.subject == second) && !stopped {
                watch.misses += 1;
                self.tally.chunks += 1;
            }
        }
    }

    fn reported(&mut self, run: u64, kind: Kind, content: Vec<u8>, judge: &mut Judge<Expected, Stimulus>) {
        let size = u32::try_from(content.len()).expect("small");
        let within = size <= self.limits.report_bytes;
        let Some(&(attempt, item, policy)) = self.runs.get(&run) else {
            self.tally.reports += 1;
            if let Some(&item) = self.unfollowed.get(&run) {
                self.miss(Subject::Item(Token::new(item)), Subject::Item(Token::new(item)));
            }
            return;
        };
        let (run, attempt) = (Token::new(run), Token::new(attempt));
        if !within {
            self.tally.reports += 1;
            self.miss(Subject::Run(run), Subject::Item(Token::new(item)));
            return;
        }
        let at = judge.now();
        let chunk = Chunk::Report { run, attempt, kind, at, content: content.clone().into() };
        self.offer(Subject::Run(run), Subject::Item(Token::new(item)), &chunk, judge);
        let content = match policy.capture(kind) {
            Capture::Nothing => return,
            Capture::Shape => None,
            Capture::Content => Some(content.into()),
        };
        self.tally.traced += 1;
        self.traced.push_back(Record { run, attempt, kind, at, size, content });
    }

    fn watch(&mut self, watcher: u64, subject: Subject, snapshot: Vec<u8>, judge: &mut Judge<Expected, Stimulus>) {
        judge.check(self.answer.is_none(), format_args!("watch {watcher}: the last watch was answered at once"));
        let open = u64::try_from(self.watchers.len()).expect("small");
        let ended = if self.ended.0 == judge.now() { self.ended.1 } else { 0 };
        let due = match subject {
            Subject::Run(run) if self.unfollowed.contains_key(&run.raw()) => Some(Refusal::Unfollowed),
            Subject::Run(run) if !self.runs.contains_key(&run.raw()) => Some(Refusal::Unknown),
            Subject::Run(_) | Subject::Item(_) | Subject::Board(_) => {
                if snapshot.len() > usize::try_from(self.limits.snapshot_bytes).expect("small") {
                    Some(Refusal::Oversized)
                } else if open >= u64::from(self.limits.watchers) {
                    Some(Refusal::Busy)
                } else {
                    None
                }
            }
        };
        // With every slot held by watches open or ended at this instant,
        // busy is right too.
        let or_busy = open + ended >= 2 * u64::from(self.limits.watchers);
        self.answer = Some(Answer { watcher, subject, snapshot, due, or_busy });
    }

    fn answered(&mut self, watcher: u64, refusal: Option<Refusal>, judge: &mut Judge<Expected, Stimulus>) {
        let Some(answer) = self.answer.take() else {
            judge.fail(format_args!("watch {watcher}: answered once, when it was taken"));
            return;
        };
        judge.check(answer.watcher == watcher, format_args!("watch {watcher}: answered when it was taken"));
        let due = answer.due;
        let right = refusal == due || (due.is_none() && answer.or_busy && refusal == Some(Refusal::Busy));
        judge.check(right, format_args!("watch {watcher}: answered {refusal:?}, not {due:?}"));
        if refusal.is_some() {
            return;
        }
        // Its snapshot first.
        let first = Chunk::Snapshot { at: judge.now(), content: answer.snapshot.into() };
        judge.expect(Expected::Reach { watcher, chunk: 1 }, self.bounds.reach);
        let watch = Watch {
            subject: answer.subject,
            taking: true,
            ending: None,
            waiting: VecDeque::from([(1, first)]),
            misses: 0,
            in_flight: None,
            chunks: 1,
        };
        self.watchers.insert(watcher, watch);
    }

    /// The watch is to end, for `end`, if it is open and has not been told
    /// so already.
    fn ending(&mut self, watcher: u64, end: End, judge: &mut Judge<Expected, Stimulus>) {
        let Some(watch) = self.watchers.get_mut(&watcher) else {
            return;
        };
        watch.taking = false;
        if watch.ending.is_none() {
            judge.expect(Expected::End(watcher), self.bounds.end);
        }
        if end == End::Unwatched || watch.ending.is_none() {
            watch.ending = Some(end);
        }
        if end == End::Unwatched {
            // What waits for it is dropped, and its reaching withdrawn.
            judge.withdraw(&Expected::Next(watcher));
            self.tally.abandoned += u64::try_from(watch.waiting.len()).expect("small");
            for (chunk, _) in watch.waiting.drain(..) {
                judge.withdraw(&Expected::Reach { watcher, chunk });
            }
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
        let (waiting, count) = (watch.waiting.len(), chunks.len());
        let dropped = u64::try_from(waiting.saturating_sub(count)).expect("small");
        if count > waiting || missed != dropped + watch.misses {
            judge.fail(format_args!(
                "watcher {watcher}: a delivery takes the newest of what waits and tells the rest missed: {missed} \
                 missed and {count} delivered of {waiting} waiting and {} missed besides",
                watch.misses
            ));
            return;
        }
        self.tally.chunks += dropped;
        for (chunk, _) in watch.waiting.drain(..waiting - count) {
            let reaching = judge.is_pending(&Expected::Reach { watcher, chunk });
            judge.check(!reaching, format_args!("watcher {watcher}: chunk {chunk}, due to reach it, is never missed"));
        }
        judge.meet(&Expected::Next(watcher));
        let mut in_flight = Vec::new();
        for delivered in chunks {
            let (number, due) = watch.waiting.pop_front().expect("as many waiting as delivered");
            // Once out, it is to reach the watcher in time.
            if !judge.is_pending(&Expected::Reach { watcher, chunk: number }) {
                judge.expect(Expected::Reach { watcher, chunk: number }, self.bounds.reach);
            }
            self.chunks += 1;
            judge.check(
                delivered == due,
                format_args!(
                    "watcher {watcher}: chunk {number} is delivered in order, once: {delivered:?} is not {due:?}"
                ),
            );
            in_flight.push(number);
        }
        watch.misses = 0;
        watch.in_flight = Some((in_flight, missed));
    }

    fn delivered(&mut self, watcher: u64, done: bool, judge: &mut Judge<Expected, Stimulus>) {
        let Some(watch) = self.watchers.get_mut(&watcher) else {
            return;
        };
        let Some((numbers, told)) = watch.in_flight.take() else {
            return;
        };
        let count = u64::try_from(numbers.len()).expect("small");
        for chunk in numbers {
            if done {
                judge.meet(&Expected::Reach { watcher, chunk });
            } else {
                judge.withdraw(&Expected::Reach { watcher, chunk });
            }
        }
        // A stopped watch ends with it, and misses nothing.
        if !done && watch.ending != Some(End::Unwatched) {
            watch.misses += told + count;
            self.tally.chunks += count;
        }
        // What waits goes out now.
        if !watch.waiting.is_empty() {
            judge.expect(Expected::Next(watcher), self.bounds.reach);
        }
    }

    fn ended(&mut self, watcher: u64, end: End, judge: &mut Judge<Expected, Stimulus>) {
        let Some(watch) = self.watchers.remove(&watcher) else {
            judge.fail(format_args!("watcher {watcher}: ends once, while its watch is open"));
            return;
        };
        self.ended = if self.ended.0 == judge.now() { (judge.now(), self.ended.1 + 1) } else { (judge.now(), 1) };
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

    /// The engine restarted: the views follow nothing and hold no watch;
    /// what the store holds stays, to be forgotten in time.
    fn restarted(&mut self, judge: &mut Judge<Expected, Stimulus>) {
        for (watcher, watch) in std::mem::take(&mut self.watchers) {
            judge.withdraw(&Expected::End(watcher));
            judge.withdraw(&Expected::Next(watcher));
            self.tally.abandoned += u64::try_from(watch.waiting.len()).expect("small");
            for (chunk, _) in watch.waiting {
                judge.withdraw(&Expected::Reach { watcher, chunk });
            }
            for chunk in watch.in_flight.map(|(numbers, _)| numbers).unwrap_or_default() {
                judge.withdraw(&Expected::Reach { watcher, chunk });
            }
        }
        self.runs.clear();
        self.unfollowed.clear();
        self.answer = None;
        self.ended = (Time::ZERO, 0);
        // The new views sweep a period from now: what the store keeps is
        // forgotten in time from the restart on.
        for (&append, times) in &self.kept {
            let newest = times.iter().max().expect("an append kept holds a record");
            let due = newest.saturating_add(self.limits.retention).max(judge.now()).saturating_add(self.bounds.forget);
            judge.rearm(Expected::Gone(append), due.saturating_since(judge.now()));
        }
    }
}

impl Expectations for Views {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        match seen {
            Seen::Started { run, attempt, item, policy } => self.started(run, attempt, item, policy),
            Seen::Reported { run, kind, content } => self.reported(run, kind, content, judge),
            Seen::Finished { run } => {
                self.unfollowed.remove(&run);
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
            Seen::Watch { watcher, subject, snapshot } => self.watch(watcher, subject, snapshot, judge),
            Seen::Unwatch { watcher } => self.ending(watcher, End::Unwatched, judge),
            Seen::Delivered { watcher, done } => self.delivered(watcher, done, judge),
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
            Seen::Restarted => self.restarted(judge),
        }
    }
}

/// A copy of `chunk`.
#[must_use]
pub fn copy(chunk: &Chunk) -> Chunk {
    match chunk {
        Chunk::Snapshot { at, content } => Chunk::Snapshot { at: *at, content: content.clone() },
        Chunk::Report { run, attempt, kind, at, content } => {
            Chunk::Report { run: *run, attempt: *attempt, kind: *kind, at: *at, content: content.clone() }
        }
        Chunk::Phase { item, phase, at } => Chunk::Phase { item: *item, phase: *phase, at: *at },
    }
}

/// A copy of `record`.
#[must_use]
pub fn copy_record(record: &Record) -> Record {
    Record {
        run: record.run,
        attempt: record.attempt,
        kind: record.kind,
        at: record.at,
        size: record.size,
        content: record.content.clone(),
    }
}

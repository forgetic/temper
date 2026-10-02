//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Duration, Env, List, Queue, Rng, Time, Token};

use crate::api::{
    Access, AgentFailure, Answer, Assignment, Bounce, Budget, Cause, Charter, Failure, Hello, Hosted, Landing, Outcome,
    Preparation, Refusal, Reply, RunFailure, Start, Tools, Verdict, Work, Workspace,
};
use crate::{Config, Event, MAX_OUT, Model, Origin, Request, charter, fire, resume, step, workspace};

/// One item in the workstream `parser`, over two writable repositories; one
/// worker's worth of everything else, and nothing at random unless a test
/// asks for it.
fn config() -> Config {
    Config {
        items: 1,
        window: Duration::from_secs(1),
        workers: 2,
        workstreams: Box::new([copy_of(b"parser")]),
        repositories: Box::new([origin(b"temper", b"ai/temper"), origin(b"docs", b"ai/docs")]),
        spread_min: 2,
        spread_max: 2,
        commits: 0,
        branches: 0,
        writable: 1000,
        saves: 0,
        invalid: 0,
        brief_min: 64,
        brief_max: 64,
        turns_min: 4,
        turns_max: 4,
        tokens_min: 1000,
        tokens_max: 1000,
        time_min: Duration::from_secs(60),
        time_max: Duration::from_secs(60),
        max_tokens: 1024,
        changes: 1000,
        checks: 0,
        verdicts: 0,
        agents: 0,
        attempts: 4,
        transient: 1000,
        permanent: 0,
        backoff_min: Duration::from_secs(1),
        backoff_max: Duration::from_secs(1),
        wakes: 2,
        wake_min: Duration::from_secs(10),
        wake_max: Duration::from_secs(10),
        resumes: 1000,
        overbook: 0,
        inbound: 0,
        inbound_min: Duration::from_secs(1),
        inbound_max: Duration::from_secs(1),
        event_min: 16,
        event_max: 16,
        resends: 0,
        cancels: 0,
        late_cancels: 0,
        stale: 0,
        cancel_min: Duration::from_secs(2),
        cancel_max: Duration::from_secs(2),
        calls: 4,
        relay_min: Duration::from_secs(1),
        relay_max: Duration::from_secs(1),
        relay_errors: 0,
        answer_min: 8,
        answer_max: 8,
        grace: Duration::from_secs(30),
        keeps: 1000,
    }
}

fn origin(name: &[u8], remote: &[u8]) -> Origin {
    Origin { name: copy_of(name), remote: copy_of(remote) }
}

/// The worker most tests use.
const WORKER: Token = Token::new(1);

/// The model, its environment, and room for one step's output.
struct Harness {
    model: Model,
    env: Env<Config>,
    out: Queue<Request>,
}

impl Harness {
    fn new(config: Config) -> Harness {
        Harness {
            model: Model::new(&config, 7),
            env: Env { now: Time::ZERO, limits: config },
            out: Queue::with_capacity(MAX_OUT),
        }
    }

    /// Steps the model with `event`, which emits nothing.
    fn step(&mut self, event: Event) {
        step(&mut self.model, &self.env, event, &mut self.out);
        assert!(self.out.is_empty(), "events emit nothing");
    }

    /// Moves time to the next deadline and fires the alarm due then.
    fn next(&mut self) -> Option<Request> {
        self.env.now = self.model.next_deadline().expect("an alarm is armed");
        fire(&mut self.model, &self.env, &mut self.out);
        self.out.pop()
    }

    /// Takes the first entry of the ready list.
    fn resume(&mut self) -> Option<Request> {
        assert!(self.model.is_ready(), "something is ready");
        resume(&mut self.model, &self.env, &mut self.out);
        self.out.pop()
    }

    fn hello(&mut self, worker: Token, slots: u32, hosting: &[Hosted]) {
        let hello = Hello { slots, workstreams: Box::new([]), hosting: hosting.into() };
        self.step(Event::Hello { worker, hello });
    }

    /// Says hello with a slot, lets the first item fall due, and takes its
    /// assignment.
    fn assign_first(&mut self) -> Assignment {
        self.hello(WORKER, 1, &[]);
        assert_eq!(self.next(), None, "a due item waits on the ready list");
        assigned(self.resume())
    }

    fn answer(&mut self, assignment: &Assignment, answer: Answer) {
        self.step(Event::Answered { run: assignment.run, attempt: assignment.attempt, answer });
    }

    /// The next due item's assignment, after its retry or wake alarm.
    fn assign_next(&mut self) -> Assignment {
        assert_eq!(self.next(), None, "a due item waits on the ready list");
        assigned(self.resume())
    }
}

fn assigned(request: Option<Request>) -> Assignment {
    let Some(Request::Assign { worker, assignment }) = request else {
        panic!("expected an assignment, not {request:?}");
    };
    assert_eq!(worker, WORKER);
    assignment
}

fn nothing() -> Work {
    Work { landed: Box::new([]), saved: None }
}

fn failed(failure: Failure) -> Answer {
    Answer::Failed { failure, work: nothing() }
}

fn hosted(assignment: &Assignment) -> Hosted {
    Hosted { run: assignment.run, attempt: assignment.attempt }
}

fn cancel(assignment: &Assignment) -> Request {
    Request::Cancel { worker: WORKER, run: assignment.run, attempt: assignment.attempt }
}

fn relay(assignment: &Assignment, call: u64) -> Event {
    Event::Relay { run: assignment.run, attempt: assignment.attempt, call: Token::new(call), body: copy_of(b"read") }
}

#[test]
fn an_item_waits_for_a_worker_to_say_hello_and_is_assigned_within_its_slots() {
    let mut h = Harness::new(Config { items: 2, ..config() });
    assert_eq!(h.next(), None);
    assert_eq!(h.next(), None);
    assert!(!h.model.is_ready(), "no worker has said hello");
    h.hello(WORKER, 1, &[]);
    let first = assigned(h.resume());
    assert_eq!(first.attempt, Token::new(1));
    assert!(!h.model.is_ready(), "the one slot is taken");
    assert_eq!(h.model.outstanding(), 1);
    h.answer(&first, Answer::Ended { outcome: copy_of(b"done"), work: nothing() });
    let second = assigned(h.resume());
    assert_ne!(second.run, first.run);
    assert_eq!(second.attempt, Token::new(2));
    let tally = h.model.tally();
    assert_eq!((tally.hellos, tally.assigned, tally.ended, tally.endings.finished), (1, 2, 1, 1));
}

#[test]
fn an_assignment_carries_the_items_workspace_and_charter() {
    let mut h = Harness::new(Config { saves: 1000, ..config() });
    let assignment = h.assign_first();
    let Workspace { key, repositories } = &assignment.workspace;
    assert_eq!(&**key, b"parser");
    assert_eq!(repositories.len(), 2);
    for repository in repositories {
        assert_eq!(repository.start, Start::Base { branch: copy_of(b"main") });
        let push = copy_of(b"temper/parser");
        assert_eq!(repository.access, Access::Writable { push, identity: copy_of(b"temper") });
    }
    assert_eq!(assignment.save.as_deref(), Some(&b"saved/parser/0"[..]));
    assert_eq!(assignment.snapshot, None);
    let charter = Decoder { bytes: assignment.charter, at: 0 }.charter();
    assert_eq!(charter.brief.len(), 64);
    assert!(charter.outcome.change, "a writable workspace may take a change");
}

#[test]
fn an_item_due_with_no_free_slot_is_sometimes_assigned_all_the_same() {
    let mut h = Harness::new(Config { items: 2, overbook: 1000, ..config() });
    h.hello(WORKER, 1, &[]);
    assert_eq!(h.next(), None);
    assigned(h.resume());
    let overbooked = assigned(h.next());
    assert_eq!(overbooked.attempt, Token::new(2));
    assert_eq!(h.model.tally().overbooked, 1);
}

#[test]
fn a_busy_refusal_is_retried_with_a_new_attempt_until_no_attempts_are_left() {
    let mut h = Harness::new(Config { attempts: 2, ..config() });
    let first = h.assign_first();
    h.answer(&first, Answer::Refused(Refusal::Busy));
    let second = h.assign_next();
    assert_eq!((second.run, second.attempt), (first.run, Token::new(2)));
    h.answer(&second, Answer::Refused(Refusal::Busy));
    h.model.reclaim();
    assert_eq!(h.model.items(), 0);
    let tally = h.model.tally();
    assert_eq!((tally.busy, tally.retries, tally.endings.held), (2, 1, 1));
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn an_invalid_refusal_closes_the_item() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.answer(&assignment, Answer::Refused(Refusal::Invalid));
    assert_eq!(h.model.tally().endings.rejected, 1);
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn an_ended_run_is_recorded_with_what_it_landed() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    let work = Work { landed: Box::new([0, 1]), saved: None };
    h.answer(&assignment, Answer::Ended { outcome: copy_of(b"done"), work });
    let tally = h.model.tally();
    assert_eq!((tally.ended, tally.landed, tally.endings.finished), (1, 2, 1));
}

#[test]
fn a_failure_is_retried_by_the_chance_of_its_class() {
    let mut h = Harness::new(config());
    let first = h.assign_first();
    h.answer(&first, failed(Failure::Unprepared(Preparation::Transient)));
    let second = h.assign_next();
    h.answer(&second, failed(Failure::Agent(AgentFailure::Exited)));
    let third = h.assign_next();
    h.answer(&third, failed(Failure::Run(RunFailure::Model)));
    let tally = h.model.tally();
    assert_eq!((tally.failed, tally.retries, tally.endings.held), (3, 2, 1));
}

#[test]
fn a_parked_run_is_woken_and_resumed_from_its_snapshot() {
    let mut h = Harness::new(config());
    let first = h.assign_first();
    h.answer(&first, Answer::Parked { snapshot: Some(copy_of(b"state")), work: nothing() });
    let second = h.assign_next();
    assert_eq!(second.snapshot.as_deref(), Some(&b"state"[..]));
    let tally = h.model.tally();
    assert_eq!((tally.parked, tally.wakes, tally.resumed), (1, 1, 1));
}

#[test]
fn a_wake_may_start_the_run_fresh_from_its_saved_work() {
    let mut h = Harness::new(Config { resumes: 0, saves: 1000, ..config() });
    let first = h.assign_first();
    let work = Work { landed: Box::new([]), saved: Some(Box::new([Landing::Landed, Landing::Unchanged])) };
    h.answer(&first, Answer::Parked { snapshot: Some(copy_of(b"state")), work });
    let second = h.assign_next();
    assert_eq!(second.snapshot, None);
    let starts: List<Start> = {
        let mut starts = List::with_capacity(2);
        for repository in &second.workspace.repositories {
            starts.push(repository.start.clone()).unwrap();
        }
        starts
    };
    let saved = Start::Saved { branch: copy_of(b"saved/parser/0") };
    assert_eq!(starts.as_slice(), &[saved, Start::Base { branch: copy_of(b"main") }]);
    assert_eq!((h.model.tally().saved, h.model.tally().resumed), (1, 0));
}

#[test]
fn a_parked_run_with_no_wakes_left_closes() {
    let mut h = Harness::new(Config { wakes: 0, ..config() });
    let assignment = h.assign_first();
    h.answer(&assignment, Answer::Parked { snapshot: None, work: nothing() });
    assert_eq!(h.model.tally().endings.parked, 1);
}

#[test]
fn a_cancelled_run_closes_once_it_answers() {
    let mut h = Harness::new(Config { cancels: 1000, ..config() });
    let assignment = h.assign_first();
    assert_eq!(h.next(), Some(cancel(&assignment)));
    h.answer(&assignment, failed(Failure::Cancelled(Cause::Engine)));
    let tally = h.model.tally();
    assert_eq!((tally.cancels, tally.endings.cancelled), (1, 1));
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn a_late_cancel_names_an_attempt_that_has_answered() {
    let mut h = Harness::new(Config { late_cancels: 1000, ..config() });
    let assignment = h.assign_first();
    h.answer(&assignment, Answer::Ended { outcome: copy_of(b"done"), work: nothing() });
    assert_eq!(h.next(), Some(cancel(&assignment)));
    assert_eq!(h.model.tally().late_cancels, 1);
}

#[test]
fn a_retry_sends_stale_traffic_to_the_attempt_it_replaced() {
    let mut h = Harness::new(Config { stale: 1000, ..config() });
    let first = h.assign_first();
    h.answer(&first, Answer::Refused(Refusal::Busy));
    h.assign_next();
    let Some(Request::Inbound { worker, run, attempt, event }) = h.next() else {
        panic!("expected a stale inbound event");
    };
    assert_eq!((worker, run, attempt, event.len()), (WORKER, first.run, first.attempt, 16));
    assert_eq!(h.model.tally().stale, 1);
}

#[test]
fn inbound_events_go_to_a_live_attempt_and_one_bounced_for_room_is_sent_again() {
    let mut h = Harness::new(Config { inbound: 2, resends: 1000, ..config() });
    let assignment = h.assign_first();
    for _ in 0_u32..2 {
        let Some(Request::Inbound { attempt, .. }) = h.next() else {
            panic!("expected an inbound event");
        };
        assert_eq!(attempt, assignment.attempt);
    }
    assert_eq!(h.model.next_deadline(), None, "no more events than configured");
    let (run, attempt) = (assignment.run, assignment.attempt);
    h.step(Event::Bounced { run, attempt, bounce: Bounce::TooLarge });
    assert_eq!(h.model.next_deadline(), None, "an event too large is not sent again");
    h.step(Event::Bounced { run, attempt, bounce: Bounce::Full });
    let Some(Request::Inbound { .. }) = h.next() else {
        panic!("expected the event sent again");
    };
    let tally = h.model.tally();
    assert_eq!((tally.inbound, tally.bounced, tally.resent), (3, 2, 1));
}

#[test]
fn a_relayed_call_is_answered_once_after_its_latency() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.step(relay(&assignment, 5));
    let reply = Reply::Answer { body: copy_of(b"Issue 42") };
    let (run, attempt, call) = (assignment.run, assignment.attempt, Token::new(5));
    assert_eq!(h.next(), Some(Request::Relayed { worker: WORKER, run, attempt, call, reply }));
    h.model.reclaim();
    assert_eq!(h.model.calls(), 0);
    assert_eq!(h.model.next_deadline(), None);
    h.step(relay(&assignment, 5));
    assert_eq!(h.model.tally().relayed, 2, "a call's name is free again once it is answered");
}

#[test]
fn a_relayed_call_may_be_answered_with_an_error() {
    let mut h = Harness::new(Config { relay_errors: 1000, ..config() });
    let assignment = h.assign_first();
    h.step(relay(&assignment, 5));
    let Some(Request::Relayed { reply, .. }) = h.next() else {
        panic!("expected a relayed answer");
    };
    assert_eq!(reply, Reply::Error);
    assert_eq!(h.model.tally().errors, 1);
}

#[test]
fn the_calls_of_a_cancelled_attempt_are_dropped() {
    let mut h = Harness::new(Config { cancels: 1000, ..config() });
    let assignment = h.assign_first();
    assert_eq!(h.next(), Some(cancel(&assignment)));
    h.step(relay(&assignment, 5));
    assert_eq!(h.model.tally().dropped, 1);
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn a_relayed_answer_waits_while_contact_is_lost() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.step(relay(&assignment, 5));
    h.step(Event::Lost { worker: WORKER });
    assert_eq!(h.next(), None, "nothing goes to a worker out of contact");
    h.hello(WORKER, 1, &[hosted(&assignment)]);
    let Some(Request::Relayed { .. }) = h.next() else {
        panic!("expected the answer once contact is back");
    };
}

#[test]
fn losing_contact_past_the_grace_presumes_attempts_lost_and_retries_them() {
    let mut h = Harness::new(config());
    let first = h.assign_first();
    h.step(Event::Lost { worker: WORKER });
    assert_eq!(h.next(), None, "the grace runs out");
    assert_eq!(h.model.outstanding(), 0);
    h.answer(&first, Answer::Ended { outcome: copy_of(b"done"), work: nothing() });
    h.hello(WORKER, 1, &[]);
    let second = h.assign_next();
    assert_eq!((second.run, second.attempt), (first.run, Token::new(2)));
    let tally = h.model.tally();
    assert_eq!((tally.lost, tally.late, tally.retries, tally.ended), (1, 1, 1, 0));
}

#[test]
fn a_reconnect_keeps_what_the_worker_reports() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.step(Event::Lost { worker: WORKER });
    h.hello(WORKER, 1, &[hosted(&assignment)]);
    assert!(!h.model.is_ready(), "nothing to cancel");
    assert_eq!(h.model.next_deadline(), None, "the grace is over");
    assert_eq!(h.model.outstanding(), 1);
}

#[test]
fn a_reconnect_may_cancel_what_the_worker_reports() {
    let mut h = Harness::new(Config { keeps: 0, ..config() });
    let assignment = h.assign_first();
    h.step(Event::Lost { worker: WORKER });
    h.hello(WORKER, 1, &[hosted(&assignment)]);
    assert_eq!(h.resume(), Some(cancel(&assignment)));
    h.answer(&assignment, failed(Failure::Cancelled(Cause::Engine)));
    assert_eq!(h.model.tally().endings.cancelled, 1);
}

#[test]
fn a_reconnect_without_a_run_presumes_it_lost() {
    let mut h = Harness::new(config());
    let first = h.assign_first();
    h.step(Event::Lost { worker: WORKER });
    h.hello(WORKER, 1, &[]);
    let second = h.assign_next();
    assert_eq!(second.run, first.run);
    assert_eq!(h.model.tally().lost, 1);
}

#[test]
fn a_reconnect_reporting_an_attempt_presumed_lost_cancels_it() {
    let mut h = Harness::new(config());
    let first = h.assign_first();
    h.step(Event::Lost { worker: WORKER });
    assert_eq!(h.next(), None, "the grace runs out");
    h.hello(WORKER, 1, &[hosted(&first)]);
    assert_eq!(h.resume(), Some(cancel(&first)));
    let second = h.assign_next();
    assert_eq!(second.attempt, Token::new(2));
}

#[test]
fn facts_are_only_counted() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.step(Event::Fact { run: assignment.run, attempt: assignment.attempt });
    h.answer(&assignment, Answer::Ended { outcome: copy_of(b"done"), work: nothing() });
    h.step(Event::Fact { run: assignment.run, attempt: assignment.attempt });
    assert_eq!(h.model.tally().facts, 2);
}

#[test]
#[should_panic(expected = "an attempt is answered once")]
fn an_attempt_answered_twice_is_a_bug() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.answer(&assignment, Answer::Refused(Refusal::Invalid));
    h.answer(&assignment, Answer::Refused(Refusal::Invalid));
}

#[test]
#[should_panic(expected = "the engine hears only of attempts it made")]
fn an_attempt_never_made_is_a_bug() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.step(Event::Fact { run: assignment.run, attempt: Token::new(2) });
}

#[test]
#[should_panic(expected = "only an attempt the engine cancelled is cancelled by it")]
fn a_cancel_the_engine_never_sent_is_a_bug() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.answer(&assignment, failed(Failure::Cancelled(Cause::Engine)));
}

#[test]
#[should_panic(expected = "a run names its calls in flight apart")]
fn a_call_named_twice_in_flight_is_a_bug() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.step(relay(&assignment, 5));
    h.step(relay(&assignment, 5));
}

#[test]
#[should_panic(expected = "a worker hosts only the attempts it was given")]
fn a_worker_reporting_anothers_attempt_is_a_bug() {
    let mut h = Harness::new(config());
    let assignment = h.assign_first();
    h.hello(Token::new(2), 1, &[hosted(&assignment)]);
}

#[test]
#[should_panic(expected = "nothing lands in a read-only repository")]
fn a_landing_in_a_read_only_repository_is_a_bug() {
    let mut h = Harness::new(Config { writable: 0, ..config() });
    let assignment = h.assign_first();
    let work = Work { landed: Box::new([0]), saved: None };
    h.answer(&assignment, Answer::Ended { outcome: copy_of(b"done"), work });
}

#[test]
fn a_seed_replays_to_the_same_assignments() {
    let one = Harness::new(Config { items: 4, branches: 500, commits: 300, saves: 500, ..config() }).assign_first();
    let two = Harness::new(Config { items: 4, branches: 500, commits: 300, saves: 500, ..config() }).assign_first();
    assert_eq!(one, two);
}

#[test]
fn a_charter_decodes_to_what_was_drawn() {
    let config = Config { verdicts: 1000, agents: 1000, checks: 1000, ..config() };
    let mut rng = Rng::new(3);
    for writable in [true, false] {
        let drawn = charter::draw(&mut rng, &config, writable);
        assert_eq!(drawn.outcome.change, writable, "only a writable workspace may take a change");
        let decoded = Decoder { bytes: charter::encode(&drawn), at: 0 }.charter();
        assert_eq!(decoded, drawn);
    }
}

#[test]
fn workspaces_beyond_the_rules_are_drawn_by_chance_and_only_so() {
    for (invalid, keeps) in [(0, true), (1000, false)] {
        let config = Config { invalid, commits: 300, branches: 300, ..config() };
        let mut rng = Rng::new(5);
        let streams = workspace::streams(&mut rng, &config);
        for place in 0..64 {
            let (drawn, _) = workspace::draw(&mut rng, &config, &streams, place);
            assert_eq!(keeps_the_rules(&drawn), keeps, "{drawn:?}");
        }
    }
}

/// Whether `workspace` keeps the rules a worker holds it to: a key, and its
/// repositories named apart, each by one safe path component.
fn keeps_the_rules(workspace: &Workspace) -> bool {
    if workspace.key.is_empty() {
        return false;
    }
    for (place, repository) in workspace.repositories.iter().enumerate() {
        let name = &*repository.name;
        let unsafe_name = name.is_empty()
            || name == b"."
            || name == b".."
            || name.eq_ignore_ascii_case(b".git")
            || name.contains(&b'/')
            || name.contains(&0);
        if unsafe_name {
            return false;
        }
        for other in &workspace.repositories[..place] {
            if other.name == repository.name {
                return false;
            }
        }
    }
    true
}

/// Reads a charter back from its encoding, as a world will.
struct Decoder {
    bytes: Box<[u8]>,
    at: usize,
}

impl Decoder {
    fn charter(&mut self) -> Charter {
        let brief = self.bytes();
        let tools = Tools { read: self.flag(), write: self.flag(), shell: self.flag() };
        let forge = self.flag();
        let agents = self.flag();
        let outlets = self.names();
        let change = self.flag();
        let checks = self.flag();
        let count = self.word();
        let mut verdicts = List::with_capacity(count);
        for _ in 0..count {
            let verdict = Verdict {
                name: self.bytes(),
                min_children: self.word(),
                max_children: self.word(),
                kinds: self.names(),
                fields: self.names(),
            };
            verdicts.push(verdict).unwrap();
        }
        let outcome = Outcome { change, checks, verdicts: verdicts.into_boxed() };
        let budget = Budget {
            turns: self.word(),
            input_tokens: self.long(),
            output_tokens: self.long(),
            cache_read_tokens: self.long(),
            cache_write_tokens: self.long(),
            wall_time: Duration::from_nanos(self.long()),
        };
        let charter = Charter {
            brief,
            tools,
            forge,
            agents,
            outlets,
            outcome,
            budget,
            endpoint: self.word(),
            model: self.bytes(),
            max_tokens: self.word(),
            models: self.names(),
        };
        assert_eq!(self.at, self.bytes.len(), "a charter is all its bytes");
        charter
    }

    fn names(&mut self) -> Box<[Box<[u8]>]> {
        let count = self.word();
        let mut names = List::with_capacity(count);
        for _ in 0..count {
            names.push(self.bytes()).unwrap();
        }
        names.into_boxed()
    }

    fn bytes(&mut self) -> Box<[u8]> {
        let len = usize::try_from(self.word()).unwrap();
        copy_of(self.raw(len))
    }

    fn flag(&mut self) -> bool {
        match self.raw(1) {
            [0] => false,
            [1] => true,
            other => panic!("a flag is 0 or 1, not {other:?}"),
        }
    }

    fn word(&mut self) -> u32 {
        u32::from_le_bytes(self.raw(4).try_into().unwrap())
    }

    fn long(&mut self) -> u64 {
        u64::from_le_bytes(self.raw(8).try_into().unwrap())
    }

    fn raw(&mut self, len: usize) -> &[u8] {
        let start = self.at;
        self.at = start.checked_add(len).unwrap();
        &self.bytes[start..self.at]
    }
}

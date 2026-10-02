//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Duration, Env, List, Queue, Time, Token};

use crate::channel::{Ask, Down, Finish, Push, Reply, RunFailure, Up};
use crate::{
    Bounce, End, Event, Fact, Fault, Invalid, Limits, MAX_OUT, Model, Request, Signal, Spawn, fire, step, worst_case,
};

const LIMITS: Limits = Limits {
    agents: 2,
    charter_bytes: 16,
    snapshot_bytes: 8,
    event_bytes: 8,
    events: 2,
    calls: 2,
    call_bytes: 8,
    answer_bytes: 8,
    fact_bytes: 8,
    outcome_bytes: 8,
    detail_bytes: 4,
    no_progress: Duration::from_secs(10),
    wall_time: Duration::from_secs(100),
    grace: Duration::from_secs(5),
    kill_after: Duration::from_secs(2),
    facts: 64,
};

fn bytes(text: &[u8]) -> Box<[u8]> {
    copy_of(text)
}

/// The model, its environment, and room for one step's output.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
}

/// An agent as a test drives it: the client's token for it, its own, and
/// io's for its process.
#[derive(Clone, Copy, Debug)]
struct Names {
    client: Token,
    agent: Token,
    process: Token,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        Harness { model: Model::new(&limits), env: Env { now: Time::ZERO, limits }, out: Queue::with_capacity(MAX_OUT) }
    }

    /// Steps `event`, returning what it emitted, oldest first.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Moves the clock to `secs` and fires what is due then, one alarm.
    fn fire_at(&mut self, secs: u64) -> Box<[Request]> {
        self.at(secs);
        assert!(self.model.is_due(self.env.now), "an alarm is due at {secs}s");
        fire(&mut self.model, &self.env, &mut self.out);
        self.drain()
    }

    fn at(&mut self, secs: u64) {
        self.env.now = Time::ZERO.saturating_add(Duration::from_secs(secs));
    }

    fn drain(&mut self) -> Box<[Request]> {
        let mut requests = List::with_capacity(MAX_OUT);
        for _ in 0..MAX_OUT {
            let Some(request) = self.out.pop() else { break };
            requests.push(request).expect("room for MAX_OUT");
        }
        assert!(self.out.is_empty(), "a step emits at most MAX_OUT");
        requests.into_boxed()
    }

    fn facts(&mut self) -> Box<[Fact]> {
        let room = self.env.limits.facts;
        let mut facts = List::with_capacity(room);
        for _ in 0..room {
            let Some(fact) = self.model.pop_fact() else { break };
            facts.push(fact).expect("room for the facts");
        }
        facts.into_boxed()
    }

    fn spawn(&mut self, client: u64, charter: &[u8], snapshot: Option<Box<[u8]>>) -> Box<[Request]> {
        let spawn = Spawn { workspace: token(client, 100), charter: bytes(charter), snapshot };
        self.step(Event::Spawn { client: Token::new(client), spawn })
    }

    /// Spawns an agent for `client`: its process is being spawned.
    fn spawning(&mut self, client: u64) -> Token {
        let emitted = self.spawn(client, b"charter", Some(bytes(b"snap")));
        let [Request::Spawn { owner, workspace }] = &*emitted else {
            panic!("expected a spawn, got {emitted:?}");
        };
        assert_eq!(*workspace, token(client, 100));
        *owner
    }

    /// Spawns an agent for `client`, whose start message has gone down: its
    /// run is live, and its next message is being read.
    fn live(&mut self, client: u64) -> Names {
        let owner = self.spawning(client);
        let process = token(client, 200);
        let emitted = self.step(Event::Spawned { owner, process });
        let a = Names { client: Token::new(client), agent: owner, process };
        let start = Down::Start { charter: bytes(b"charter"), snapshot: Some(bytes(b"snap")) };
        assert_eq!(
            &*emitted,
            [
                Request::Started { client: a.client, agent: owner },
                Request::Wait { owner, process },
                Request::Reap { owner, process },
                Request::Send { owner, process, message: start },
                read(a),
            ]
        );
        assert!(self.step(Event::Sent { owner }).is_empty(), "nothing waits to go down");
        a
    }

    fn say(&mut self, a: Names, message: Up) -> Box<[Request]> {
        self.step(Event::Received { owner: a.agent, message })
    }

    /// The run makes the host call `call`, which goes to the client.
    fn call(&mut self, a: Names, call: u64) {
        let ask = Ask::Relay { body: bytes(b"read") };
        let emitted = self.say(a, Up::Call { call: Token::new(call), ask });
        let called =
            Request::Called { client: a.client, call: Token::new(call), ask: Ask::Relay { body: bytes(b"read") } };
        assert_eq!(&*emitted, [called, read(a)]);
    }

    fn deliver(&mut self, a: Names, event: &[u8]) -> Box<[Request]> {
        self.step(Event::Deliver { agent: a.agent, event: bytes(event) })
    }

    fn answer(&mut self, a: Names, call: u64, reply: Reply) -> Box<[Request]> {
        self.step(Event::Answer { agent: a.agent, call: Token::new(call), reply })
    }

    fn stop(&mut self, a: Names) -> Box<[Request]> {
        self.step(Event::Stop { agent: a.agent })
    }

    fn sent(&mut self, a: Names) -> Box<[Request]> {
        self.step(Event::Sent { owner: a.agent })
    }

    fn finish(&mut self, a: Names) -> Box<[Request]> {
        self.say(a, Up::Finish { finish: Finish::Ended { outcome: bytes(b"done") } })
    }

    /// The run of a live agent ends: it is exiting.
    fn exiting(&mut self, a: Names) {
        let emitted = self.finish(a);
        assert_eq!(&*emitted, [finished(a), read(a)]);
    }

    /// The process exits, its channel ends and its tree empties: the agent
    /// has gone.
    fn goes(&mut self, a: Names) {
        assert!(self.step(Event::Exited { owner: a.agent }).is_empty(), "the channel is still read");
        assert!(self.step(Event::Hangup { owner: a.agent }).is_empty(), "the tree is not empty yet");
        let emitted = self.step(Event::Reaped { owner: a.agent, detail: bytes(b"bye") });
        assert_eq!(&*emitted, [gone(a, b"bye")]);
        self.model.reclaim();
        assert_eq!(self.model.agents(), 0, "its slot is free");
        assert_eq!(self.model.next_deadline(), None, "and no alarm is left");
    }
}

fn token(base: u64, offset: u64) -> Token {
    Token::new(base.checked_add(offset).expect("a test's tokens are small"))
}

fn read(a: Names) -> Request {
    Request::Read { owner: a.agent, process: a.process }
}

fn send(a: Names, message: Down) -> Request {
    Request::Send { owner: a.agent, process: a.process, message }
}

fn signal(a: Names, signal: Signal) -> Request {
    Request::Signal { owner: a.agent, process: a.process, signal }
}

fn faulted(a: Names, fault: Fault) -> Request {
    Request::Faulted { client: a.client, fault }
}

fn finished(a: Names) -> Request {
    Request::Finished { client: a.client, finish: Finish::Ended { outcome: bytes(b"done") } }
}

fn gone(a: Names, detail: &[u8]) -> Request {
    Request::Gone { client: a.client, end: End::Stopped, detail: bytes(detail) }
}

fn secs(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

// The entrance.

#[test]
fn a_spawn_beyond_the_slots_is_refused_as_busy() {
    let mut h = Harness::new(LIMITS);
    h.spawning(1);
    h.spawning(2);
    let emitted = h.spawn(3, b"charter", None);
    assert_eq!(&*emitted, [Request::Gone { client: Token::new(3), end: End::Busy, detail: bytes(b"") }]);
    assert_eq!(h.model.agents(), 2);
}

#[test]
fn a_spawn_beyond_the_limits_is_refused_as_invalid() {
    let mut h = Harness::new(LIMITS);
    let emitted = h.spawn(1, &[b'c'; 17], None);
    assert_eq!(
        &*emitted,
        [Request::Gone { client: Token::new(1), end: End::Invalid(Invalid::Charter), detail: bytes(b"") }]
    );
    let emitted = h.spawn(2, b"charter", Some(bytes(&[b's'; 9])));
    let end = End::Invalid(Invalid::Snapshot);
    assert_eq!(&*emitted, [Request::Gone { client: Token::new(2), end, detail: bytes(b"") }]);
    assert_eq!(h.model.agents(), 0, "nothing was taken");
    let emitted = h.spawn(3, &[b'c'; 16], Some(bytes(&[b's'; 8])));
    let [Request::Spawn { .. }] = &*emitted else {
        panic!("exactly the limits fit: {emitted:?}");
    };
}

// Spawning.

#[test]
fn a_spawned_process_starts_its_run_with_the_start_message_first() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    assert_eq!(h.model.next_deadline(), Some(secs(10)), "the watchdog runs");
    assert_eq!(&*h.facts(), [Fact::Started { client: a.client }], "it tells it started");
}

#[test]
fn a_process_that_cannot_be_spawned_is_gone_with_the_tail_of_its_detail() {
    let mut h = Harness::new(LIMITS);
    let owner = h.spawning(1);
    let emitted = h.step(Event::Unspawned { owner, detail: bytes(b"no such program") });
    let end = End::Unspawned;
    assert_eq!(&*emitted, [Request::Gone { client: Token::new(1), end, detail: bytes(b"gram") }]);
    h.model.reclaim();
    assert_eq!(h.model.agents(), 0);
}

// Live: what the run says.

#[test]
fn a_live_run_calls_tells_waits_and_finishes() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.call(a, 7);
    let emitted = h.say(a, Up::Fact { fact: bytes(b"tool") });
    assert_eq!(&*emitted, [Request::Told { client: a.client, fact: bytes(b"tool") }, read(a)]);
    let emitted = h.say(a, Up::Waiting);
    assert_eq!(&*emitted, [Request::Waiting { client: a.client }, read(a)]);
    let emitted = h.say(a, Up::Finish { finish: Finish::Parked { snapshot: Some(bytes(b"snap")) } });
    let finish = Finish::Parked { snapshot: Some(bytes(b"snap")) };
    assert_eq!(&*emitted, [Request::Finished { client: a.client, finish }, read(a)]);
    // Its call's answer is dropped: the run no longer listens.
    assert!(h.answer(a, 7, Reply::Unavailable).is_empty());
    assert_eq!(h.model.next_deadline(), Some(secs(5)), "the grace runs");
    h.goes(a);
}

#[test]
fn a_run_may_fail_as_it_reports_it() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    let emitted = h.say(a, Up::Finish { finish: Finish::Failed { failure: RunFailure::Budget } });
    let finish = Finish::Failed { failure: RunFailure::Budget };
    assert_eq!(&*emitted, [Request::Finished { client: a.client, finish }, read(a)]);
}

#[test]
fn a_call_beyond_the_runs_limit_is_answered_busy_before_anything_more_is_read() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.call(a, 1);
    h.call(a, 2);
    // The third goes down as busy at once, and the next read waits for it.
    let emitted = h.say(a, Up::Call { call: Token::new(3), ask: Ask::Push { message: bytes(b"m") } });
    let busy = Down::Answer { call: Token::new(3), reply: Reply::Busy };
    assert_eq!(&*emitted, [send(a, busy), read(a)]);
    // While a send is in flight, a busy answer waits, and nothing is read.
    let emitted = h.answer(a, 1, Reply::Pushed(Push::Done));
    assert!(emitted.is_empty(), "the answer waits for the send in flight: {emitted:?}");
    let emitted = h.say(a, Up::Call { call: Token::new(4), ask: Ask::Push { message: bytes(b"m") } });
    assert!(emitted.is_empty(), "busy waits behind the send in flight, and no read: {emitted:?}");
    let emitted = h.sent(a);
    assert_eq!(&*emitted, [send(a, Down::Answer { call: Token::new(4), reply: Reply::Busy }), read(a)]);
    let emitted = h.sent(a);
    assert_eq!(&*emitted, [send(a, Down::Answer { call: Token::new(1), reply: Reply::Pushed(Push::Done) })]);
    assert!(h.sent(a).is_empty());
    // Call 1's answer has gone down: its name is free again, and the run has
    // room for one more call.
    h.call(a, 1);
}

#[test]
fn a_call_reusing_a_name_in_flight_breaks_the_rules() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.call(a, 1);
    h.call(a, 2);
    let emitted = h.answer(a, 2, Reply::Relayed { answer: bytes(b"ok") });
    let answer = Down::Answer { call: Token::new(2), reply: Reply::Relayed { answer: bytes(b"ok") } };
    assert_eq!(&*emitted, [send(a, answer)]);
    // Call 1's answer waits behind it: its name is still in flight.
    assert!(h.answer(a, 1, Reply::Unavailable).is_empty());
    let emitted = h.say(a, Up::Call { call: Token::new(1), ask: Ask::Relay { body: bytes(b"x") } });
    assert_eq!(&*emitted, [faulted(a, Fault::Rules), signal(a, Signal::Terminate), read(a)]);
}

#[test]
fn payloads_beyond_the_limits_break_the_rules() {
    let breaches = [
        Up::Call { call: Token::new(1), ask: Ask::Relay { body: bytes(&[b'x'; 9]) } },
        Up::Call { call: Token::new(1), ask: Ask::Push { message: bytes(&[b'x'; 9]) } },
        Up::Fact { fact: bytes(&[b'x'; 9]) },
        Up::Finish { finish: Finish::Ended { outcome: bytes(&[b'x'; 9]) } },
        Up::Finish { finish: Finish::Parked { snapshot: Some(bytes(&[b'x'; 9])) } },
    ];
    for message in breaches {
        let mut h = Harness::new(LIMITS);
        let a = h.live(1);
        let emitted = h.say(a, message);
        assert_eq!(&*emitted, [faulted(a, Fault::Rules), signal(a, Signal::Terminate), read(a)]);
        assert_eq!(h.model.next_deadline(), Some(secs(2)), "killed past kill_after");
    }
}

#[test]
fn a_malformed_message_breaks_the_rules_and_ends_reading() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    let emitted = h.step(Event::Malformed { owner: a.agent });
    assert_eq!(&*emitted, [faulted(a, Fault::Rules), signal(a, Signal::Terminate)]);
    assert!(h.step(Event::Signalled { owner: a.agent }).is_empty());
    assert!(h.step(Event::Exited { owner: a.agent }).is_empty());
    let emitted = h.step(Event::Reaped { owner: a.agent, detail: bytes(b"x") });
    assert_eq!(&*emitted, [gone(a, b"x")], "the channel up ended with the malformed message");
}

#[test]
fn a_hangup_before_the_finish_is_an_exit_without_answering() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    let emitted = h.step(Event::Hangup { owner: a.agent });
    assert_eq!(&*emitted, [faulted(a, Fault::Exited), signal(a, Signal::Terminate)]);
}

// Live: the client's side.

#[test]
fn inbound_events_go_down_in_order_or_are_bounced() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    let emitted = h.deliver(a, b"one");
    assert_eq!(&*emitted, [send(a, Down::Event { event: bytes(b"one") })]);
    assert!(h.deliver(a, b"two").is_empty(), "it waits");
    assert!(h.deliver(a, b"three").is_empty(), "it waits");
    let emitted = h.deliver(a, b"four");
    assert_eq!(&*emitted, [Request::Bounced { client: a.client, bounce: Bounce::Full }]);
    let emitted = h.deliver(a, &[b'x'; 9]);
    assert_eq!(&*emitted, [Request::Bounced { client: a.client, bounce: Bounce::TooLarge }]);
    assert_eq!(&*h.sent(a), [send(a, Down::Event { event: bytes(b"two") })]);
    assert_eq!(&*h.sent(a), [send(a, Down::Event { event: bytes(b"three") })]);
    assert!(h.sent(a).is_empty());
}

#[test]
fn an_answer_too_large_goes_down_as_such() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.call(a, 1);
    let emitted = h.answer(a, 1, Reply::Relayed { answer: bytes(&[b'x'; 9]) });
    assert_eq!(&*emitted, [send(a, Down::Answer { call: Token::new(1), reply: Reply::TooLarge })]);
}

#[test]
fn a_stop_cancels_the_run_behind_what_waits_and_its_finish_is_still_heard() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    assert_eq!(&*h.deliver(a, b"one"), [send(a, Down::Event { event: bytes(b"one") })]);
    assert!(h.stop(a).is_empty(), "the cancel waits behind the event");
    assert!(h.stop(a).is_empty(), "a second stop is the first");
    assert_eq!(&*h.sent(a), [send(a, Down::Cancel)]);
    assert!(h.sent(a).is_empty());
    assert_eq!(h.model.next_deadline(), Some(secs(5)), "the grace runs, the watchdog no longer");
    let emitted = h.deliver(a, b"two");
    assert_eq!(&*emitted, [Request::Bounced { client: a.client, bounce: Bounce::Ending }]);
    h.exiting(a);
    assert!(h.stop(a).is_empty(), "a stop after the finish is harmless");
    h.goes(a);
    assert!(h.stop(a).is_empty(), "a stop for an agent gone is dropped");
    assert!(h.deliver(a, b"x").is_empty(), "so is an event");
    assert!(h.answer(a, 1, Reply::Busy).is_empty(), "and an answer");
}

// The watchdog and the wall time.

#[test]
fn silence_past_the_no_progress_deadline_stops_the_run() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.at(4);
    h.say(a, Up::Fact { fact: bytes(b"f") });
    assert_eq!(h.model.next_deadline(), Some(secs(14)), "progress moves the deadline");
    let emitted = h.fire_at(14);
    assert_eq!(&*emitted, [faulted(a, Fault::NoProgress), signal(a, Signal::Terminate)]);
    assert_eq!(h.model.next_deadline(), Some(secs(16)), "killed past kill_after");
    assert_eq!(&*h.fire_at(16), [signal(a, Signal::Kill)]);
    assert_eq!(h.model.next_deadline(), None, "nothing is left to time");
    assert!(h.step(Event::Signalled { owner: a.agent }).is_empty());
    assert!(h.step(Event::Signalled { owner: a.agent }).is_empty());
    assert!(h.step(Event::Exited { owner: a.agent }).is_empty());
    assert!(h.step(Event::Hangup { owner: a.agent }).is_empty());
    assert_eq!(&*h.step(Event::Reaped { owner: a.agent, detail: bytes(b"k") }), [gone(a, b"k")]);
    let facts = h.facts();
    let client = a.client;
    assert_eq!(
        &*facts,
        [
            Fact::Started { client },
            Fact::Faulted { client, fault: Fault::NoProgress },
            Fact::Terminated { client },
            Fact::Killed { client },
            Fact::Gone { client, end: End::Stopped },
        ]
    );
}

#[test]
fn the_clock_pauses_while_a_call_waits_for_the_client() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.call(a, 1);
    h.call(a, 2);
    assert_eq!(h.model.next_deadline(), Some(secs(100)), "only the wall time runs");
    h.at(50);
    h.answer(a, 1, Reply::Unavailable);
    assert_eq!(h.model.next_deadline(), Some(secs(100)), "a call still waits");
    h.at(60);
    h.answer(a, 2, Reply::Unavailable);
    assert_eq!(h.model.next_deadline(), Some(secs(70)), "the clock runs afresh");
}

#[test]
fn the_clock_pauses_while_the_run_waits_for_an_inbound_event() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.say(a, Up::Waiting);
    h.at(1);
    h.say(a, Up::Fact { fact: bytes(b"yielded") });
    assert_eq!(h.model.next_deadline(), Some(secs(100)), "facts do not end the wait");
    h.at(80);
    h.deliver(a, b"hello");
    assert_eq!(h.model.next_deadline(), Some(secs(90)), "an event ends it");
    let emitted = h.fire_at(90);
    assert_eq!(&*emitted, [faulted(a, Fault::NoProgress), signal(a, Signal::Terminate)]);
}

#[test]
fn a_long_operation_holds_the_clock_until_its_deadline() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.at(2);
    h.say(a, Up::Long { span: Duration::from_secs(60) });
    assert_eq!(h.model.next_deadline(), Some(secs(72)), "held until 62, then the clock runs");
    h.at(30);
    h.say(a, Up::Fact { fact: bytes(b"f") });
    assert_eq!(h.model.next_deadline(), Some(secs(72)), "progress does not cut it short");
    h.at(70);
    h.say(a, Up::Fact { fact: bytes(b"done") });
    assert_eq!(h.model.next_deadline(), Some(secs(80)));
}

#[test]
fn the_wall_time_stops_a_run_whatever_it_does() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.say(a, Up::Waiting);
    let emitted = h.fire_at(100);
    assert_eq!(&*emitted, [faulted(a, Fault::WallTime), signal(a, Signal::Terminate)]);
}

// Draining: an exit before the finish.

#[test]
fn a_finish_read_after_the_exit_is_heard() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    assert!(h.step(Event::Exited { owner: a.agent }).is_empty(), "what it wrote is drained");
    assert_eq!(h.model.next_deadline(), Some(secs(5)), "for the grace at most");
    assert_eq!(
        &*h.say(a, Up::Fact { fact: bytes(b"f") }),
        [Request::Told { client: a.client, fact: bytes(b"f") }, read(a)]
    );
    let call = Up::Call { call: Token::new(1), ask: Ask::Relay { body: bytes(b"r") } };
    assert_eq!(&*h.say(a, call), [read(a)], "a call is dropped: nothing would hear its answer");
    assert_eq!(&*h.say(a, Up::Waiting), [read(a)]);
    assert_eq!(&*h.deliver(a, b"x"), [Request::Bounced { client: a.client, bounce: Bounce::Ending }]);
    assert_eq!(&*h.finish(a), [finished(a), read(a)]);
    assert!(h.step(Event::Hangup { owner: a.agent }).is_empty());
    assert_eq!(&*h.step(Event::Reaped { owner: a.agent, detail: bytes(b"") }), [gone(a, b"")]);
}

#[test]
fn an_exit_without_a_finish_is_a_fault_at_the_hangup_or_the_grace() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.step(Event::Exited { owner: a.agent });
    let emitted = h.step(Event::Hangup { owner: a.agent });
    assert_eq!(&*emitted, [faulted(a, Fault::Exited)]);
    assert_eq!(h.model.next_deadline(), Some(secs(5)), "it is waited for within the grace");
    assert_eq!(&*h.step(Event::Reaped { owner: a.agent, detail: bytes(b"") }), [gone(a, b"")]);

    // Children holding the channel open: the grace ends the drain.
    let b = h.live(2);
    h.step(Event::Exited { owner: b.agent });
    let emitted = h.fire_at(5);
    assert_eq!(&*emitted, [faulted(b, Fault::Exited), signal(b, Signal::Terminate)]);
}

#[test]
fn a_run_that_stops_reading_its_channel_is_drained() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.deliver(a, b"e");
    assert!(h.step(Event::Unsent { owner: a.agent }).is_empty());
    let emitted = h.say(a, Up::Fact { fact: bytes(&[b'x'; 9]) });
    assert_eq!(&*emitted, [faulted(a, Fault::Rules), signal(a, Signal::Terminate), read(a)]);
}

#[test]
fn a_draining_agent_stopped_by_its_client_is_no_longer_faulted() {
    let mut h = Harness::new(Limits { agents: 3, ..LIMITS });
    let a = h.live(1);
    h.step(Event::Exited { owner: a.agent });
    assert!(h.stop(a).is_empty());
    assert!(h.answer(a, 1, Reply::Busy).is_empty(), "an answer is dropped");
    assert!(h.step(Event::Hangup { owner: a.agent }).is_empty(), "no fault is told");
    h.step(Event::Reaped { owner: a.agent, detail: bytes(b"") });

    let b = h.live(2);
    h.step(Event::Exited { owner: b.agent });
    h.stop(b);
    assert_eq!(&*h.fire_at(5), [signal(b, Signal::Terminate)]);
    let c = h.live(3);
    h.step(Event::Exited { owner: c.agent });
    h.stop(c);
    assert_eq!(&*h.step(Event::Malformed { owner: c.agent }), [signal(c, Signal::Terminate)]);
}

#[test]
fn a_malformed_message_while_draining_is_a_fault_while_live() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.step(Event::Exited { owner: a.agent });
    let emitted = h.step(Event::Malformed { owner: a.agent });
    assert_eq!(&*emitted, [faulted(a, Fault::Rules), signal(a, Signal::Terminate)]);
}

// Cancelled.

#[test]
fn a_cancelled_run_still_calls_and_tells_until_it_finishes() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.call(a, 1);
    h.stop(a);
    h.sent(a);
    h.call(a, 2);
    assert_eq!(
        &*h.say(a, Up::Fact { fact: bytes(b"f") }),
        [Request::Told { client: a.client, fact: bytes(b"f") }, read(a)]
    );
    assert_eq!(&*h.say(a, Up::Waiting), [read(a)], "the watchdog no longer runs");
    assert_eq!(&*h.say(a, Up::Long { span: Duration::from_secs(1) }), [read(a)]);
    let emitted = h.answer(a, 1, Reply::Unavailable);
    assert_eq!(&*emitted, [send(a, Down::Answer { call: Token::new(1), reply: Reply::Unavailable })]);
    h.sent(a);
    assert_eq!(
        &*h.say(a, Up::Call { call: Token::new(3), ask: Ask::Relay { body: bytes(b"r") } }),
        [Request::Called { client: a.client, call: Token::new(3), ask: Ask::Relay { body: bytes(b"r") } }, read(a)]
    );
    let emitted = h.say(a, Up::Call { call: Token::new(4), ask: Ask::Relay { body: bytes(b"r") } });
    assert_eq!(&*emitted, [send(a, Down::Answer { call: Token::new(4), reply: Reply::Busy }), read(a)]);
    assert!(h.sent(a).is_empty());
    h.exiting(a);
    h.goes(a);
}

#[test]
fn a_cancelled_run_that_breaks_the_rules_is_terminated_untold() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.stop(a);
    let emitted = h.say(a, Up::Fact { fact: bytes(&[b'x'; 9]) });
    assert_eq!(&*emitted, [signal(a, Signal::Terminate), read(a)]);
    assert!(h.facts().contains(&Fact::Faulted { client: a.client, fault: Fault::Rules }), "a fact tells of it");
    let b = h.live(2);
    h.stop(b);
    assert_eq!(&*h.step(Event::Malformed { owner: b.agent }), [signal(b, Signal::Terminate)]);
}

#[test]
fn a_cancelled_run_past_the_grace_is_terminated_then_killed() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.stop(a);
    h.sent(a);
    assert_eq!(&*h.fire_at(5), [signal(a, Signal::Terminate)]);
    assert_eq!(&*h.say(a, Up::Fact { fact: bytes(b"late") }), [read(a)], "what it says now is dropped");
    assert_eq!(&*h.fire_at(7), [signal(a, Signal::Kill)]);
    assert_eq!(&*h.stop(a), [], "a stop changes nothing");
    assert!(h.answer(a, 1, Reply::Busy).is_empty(), "an answer is dropped");
    assert_eq!(&*h.say(a, Up::Waiting), [read(a)], "what it says is dropped");
    assert_eq!(&*h.deliver(a, b"x"), [Request::Bounced { client: a.client, bounce: Bounce::Ending }]);
    assert!(h.step(Event::Malformed { owner: a.agent }).is_empty());
    assert!(h.step(Event::Exited { owner: a.agent }).is_empty());
    assert!(h.step(Event::Reaped { owner: a.agent, detail: bytes(b"") }).is_empty(), "signals in flight");
    assert!(h.step(Event::Signalled { owner: a.agent }).is_empty(), "a signal in flight");
    assert_eq!(&*h.step(Event::Signalled { owner: a.agent }), [gone(a, b"")]);
}

#[test]
fn a_cancelled_run_that_hangs_up_or_exits_is_waited_for() {
    let mut h = Harness::new(Limits { agents: 3, ..LIMITS });
    let a = h.live(1);
    h.stop(a);
    assert!(h.step(Event::Hangup { owner: a.agent }).is_empty(), "no fault after a stop");
    assert_eq!(&*h.deliver(a, b"x"), [Request::Bounced { client: a.client, bounce: Bounce::Ending }]);
    assert_eq!(&*h.fire_at(5), [signal(a, Signal::Terminate)]);

    let b = h.live(2);
    h.at(10);
    h.stop(b);
    h.sent(b);
    assert!(h.step(Event::Exited { owner: b.agent }).is_empty());
    assert_eq!(&*h.finish(b), [finished(b), read(b)], "its finish is still heard");
    assert!(h.step(Event::Hangup { owner: b.agent }).is_empty());
    assert_eq!(&*h.step(Event::Reaped { owner: b.agent, detail: bytes(b"") }), [gone(b, b"")]);

    let c = h.live(3);
    h.stop(c);
    assert!(h.step(Event::Unsent { owner: c.agent }).is_empty(), "the cancel could not go down");
    assert!(h.step(Event::Hangup { owner: c.agent }).is_empty());
}

// Exiting.

#[test]
fn anything_after_the_finish_breaks_the_rules() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.exiting(a);
    assert_eq!(&*h.deliver(a, b"x"), [Request::Bounced { client: a.client, bounce: Bounce::Ending }]);
    let emitted = h.say(a, Up::Fact { fact: bytes(b"more") });
    assert_eq!(&*emitted, [signal(a, Signal::Terminate), read(a)], "terminated, the fault untold");
    let b = h.live(2);
    h.exiting(b);
    assert_eq!(&*h.step(Event::Malformed { owner: b.agent }), [signal(b, Signal::Terminate)]);
}

#[test]
fn a_finished_run_that_outstays_the_grace_is_terminated() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.exiting(a);
    assert!(h.step(Event::Exited { owner: a.agent }).is_empty(), "children may still run");
    assert_eq!(&*h.fire_at(5), [signal(a, Signal::Terminate)]);
    assert!(h.step(Event::Hangup { owner: a.agent }).is_empty());
    assert!(h.step(Event::Reaped { owner: a.agent, detail: bytes(b"") }).is_empty(), "a signal in flight");
    assert_eq!(&*h.step(Event::Signalled { owner: a.agent }), [gone(a, b"")]);
}

#[test]
fn an_agent_has_gone_only_once_nothing_asked_of_io_is_in_flight() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.deliver(a, b"e");
    h.exiting(a);
    assert!(h.step(Event::Exited { owner: a.agent }).is_empty());
    assert!(h.step(Event::Reaped { owner: a.agent, detail: bytes(b"longdetail") }).is_empty(), "still read");
    assert!(h.step(Event::Hangup { owner: a.agent }).is_empty(), "a send in flight");
    assert_eq!(&*h.step(Event::Unsent { owner: a.agent }), [gone(a, b"tail")]);
}

#[test]
fn a_terminated_tree_reads_its_channel_to_the_end() {
    let mut h = Harness::new(LIMITS);
    let a = h.live(1);
    h.step(Event::Hangup { owner: a.agent });
    assert!(h.step(Event::Exited { owner: a.agent }).is_empty());
    assert!(h.stop(a).is_empty());
    assert!(h.answer(a, 1, Reply::Busy).is_empty());
    assert_eq!(&*h.fire_at(2), [signal(a, Signal::Kill)]);
    h.step(Event::Signalled { owner: a.agent });
    h.step(Event::Signalled { owner: a.agent });
    assert_eq!(&*h.step(Event::Reaped { owner: a.agent, detail: bytes(b"") }), [gone(a, b"")]);
}

// The worst case and facts.

#[test]
fn the_worst_case_is_bounded_or_refused() {
    assert!(worst_case(&LIMITS).is_some(), "the test limits fit");
    assert!(worst_case(&Limits { agents: 1 << 20, ..LIMITS }).is_some(), "a million agents sum");
    assert_eq!(worst_case(&Limits { agents: u32::MAX, ..LIMITS }), None, "two alarms each do not fit a u32");
    assert_eq!(worst_case(&Limits { charter_bytes: u64::MAX, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { events: u32::MAX, event_bytes: u64::MAX, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { calls: u32::MAX, answer_bytes: u64::MAX, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { events: u32::MAX, calls: u32::MAX, ..LIMITS }), None, "the outbox's room");
}

#[test]
fn facts_beyond_their_room_are_dropped_and_counted() {
    let mut h = Harness::new(Limits { facts: 1, ..LIMITS });
    let a = h.live(1);
    h.stop(a);
    assert_eq!(h.model.facts_lost(), 1);
    assert_eq!(&*h.facts(), [Fact::Started { client: a.client }]);
}

//! Full duplex frame machine. Connections own timers and authenticate workers.
use crate::{
    Limits, Sizes, codec, sizes,
    wire::{Channel, Message, Open, Refuse},
};
use alloc::boxed::Box;
use skein_lib::{Queue, Writer, stream};

pub const MAX_UP: u32 = 2;
pub const MAX_DOWN: u32 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Endpoint {
    Engine,
    WorkerLink,
    WorkerAgent,
    Agent,
}
impl Endpoint {
    #[must_use]
    pub const fn receives(self, kind: u16) -> bool {
        if kind == 3 || kind == 16 {
            return true;
        }
        match self {
            Endpoint::Engine => kind == 1 || kind == 4 || (kind >= 0x101 && kind <= 0x107),
            Endpoint::WorkerLink => kind == 2 || kind == 4 || (kind >= 0x181 && kind <= 0x186),
            Endpoint::WorkerAgent => kind == 2 || (kind >= 0x201 && kind <= 0x209),
            Endpoint::Agent => kind == 1 || (kind >= 0x281 && kind <= 0x285),
        }
    }
    #[must_use]
    pub const fn sends(self, kind: u16) -> bool {
        match self {
            Endpoint::Engine => Endpoint::WorkerLink.receives(kind),
            Endpoint::WorkerLink => Endpoint::Engine.receives(kind),
            Endpoint::WorkerAgent => Endpoint::Agent.receives(kind),
            Endpoint::Agent => Endpoint::WorkerAgent.receives(kind),
        }
    }
    const fn channel(self) -> Channel {
        match self {
            Endpoint::Engine | Endpoint::WorkerLink => Channel::Link,
            Endpoint::WorkerAgent | Endpoint::Agent => Channel::Agent,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Opening,
    Authorizing,
    Terms,
    Hello,
    Starting,
    Open,
    Finished,
    Closing,
    Closed,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fault {
    Framing,
    Limits,
    Version,
    End,
    Stream(stream::Fault),
    Closed,
    OutputFull,
}
#[derive(Debug)]
pub enum Event {
    Message(Message),
    Ready {
        version: u16,
    },
    Sent,
    Unsent,
    /// Agent stdin ended; output remains usable while its domain winds down.
    ReadEnded,
    Closed {
        fault: Fault,
    },
}
#[derive(Debug)]
pub enum Request {
    /// Start or send a frame. Frozen opening/terms may be sent before Ready.
    Send(Message),
    /// Demand one versioned message. `WorkerAgent` uses one demand per domain Read.
    Read,
    /// Accept the received Open after the parent's authentication succeeds.
    Accept {
        version: u16,
    },
    Refuse(Refuse),
    Close,
}
#[derive(Debug)]
enum Input {
    Header,
    Body { kind: u16, length: u32, body: Writer },
    Held(Message),
    Stopped,
}
#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent stream demand and opening flags")]
pub struct Machine {
    endpoint: Endpoint,
    phase: Phase,
    version: u16,
    input: Input,
    reading: bool,
    read_ended: bool,
    received_frames: u64,
    outstanding: bool,
    demand_read: stream::Read,
    demand_room: u32,
    demand_sent: u32,
    grant: u32,
    pending_bytes: u32,
    sent_bytes: u32,
    output: Queue<Box<[u8]>>,
    cap: u32,
    sent_open: bool,
    sent_terms: bool,
    got_terms: bool,
    first_sent: bool,
    fault: Option<Fault>,
}
impl Machine {
    #[must_use]
    pub fn new(endpoint: Endpoint, limits: &Limits, sizes: &Sizes) -> Option<Machine> {
        if !limits.valid() || !sizes.valid() {
            return None;
        }
        let cap = sizes::output_cap(endpoint, sizes)?;
        Some(Machine {
            endpoint,
            phase: Phase::Opening,
            version: 0,
            input: Input::Header,
            reading: true,
            read_ended: false,
            received_frames: 0,
            outstanding: false,
            demand_read: stream::Read::Nothing,
            demand_room: 0,
            demand_sent: 0,
            grant: 0,
            pending_bytes: 0,
            sent_bytes: 0,
            output: Queue::with_capacity(sizes::output_slots(endpoint, sizes)?),
            cap,
            sent_open: false,
            sent_terms: false,
            got_terms: false,
            first_sent: false,
            fault: None,
        })
    }
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }
    #[must_use]
    pub const fn endpoint(&self) -> Endpoint {
        self.endpoint
    }
    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
    }
    /// Validated frames include pings; connections use this to reset silence.
    #[must_use]
    pub const fn received_frames(&self) -> u64 {
        self.received_frames
    }
    /// A Send is admitted only while its final frame fits the conservative credit.
    #[must_use]
    pub fn room(&self) -> u32 {
        self.grant.saturating_sub(self.pending_bytes)
    }
    #[must_use]
    pub const fn pending_bytes(&self) -> u32 {
        self.pending_bytes
    }
    /// Ready work advances one frame or states a currently absent demand.
    /// Blocked output with a demand outstanding is not ready work.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        if self.phase == Phase::Closed || self.phase == Phase::Closing {
            return false;
        }
        let held = match &self.input {
            Input::Held(_) => true,
            Input::Header | Input::Body { .. } | Input::Stopped => false,
        };
        let fits = match self.output.iter().next() {
            Some(frame) => u32::try_from(frame.len()).expect("frame length is u32") <= self.grant,
            None => false,
        };
        held || fits
            || (self.phase == Phase::Finished && self.output.is_empty())
            || (!self.outstanding && (self.reading && !self.read_ended || self.grant < self.cap))
    }
    /// Conservative output still held here or owed a lower room grant.
    #[must_use]
    pub fn queued_bytes(&self) -> u32 {
        self.pending_bytes.checked_add(self.sent_bytes).expect("output remains within its cap")
    }
}

fn emit(out: &mut Queue<Event>, event: Event) {
    out.push(event);
}
fn send(out: &mut Queue<stream::Down>, request: stream::Down) {
    out.push(request);
}
fn queue(machine: &mut Machine, message: &Message, sizes: &Sizes) -> Option<()> {
    let frame = codec::encode(message, sizes)?;
    let length = u32::try_from(frame.len()).ok()?;
    // Opening reserve is usable before the initial whole-cap grant arrives.
    let used = machine.pending_bytes.checked_add(length)?;
    if used.checked_add(machine.sent_bytes)? > machine.cap {
        return None;
    }
    machine.output.try_push(frame).ok()?;
    machine.pending_bytes = used;
    Some(())
}
fn stop(machine: &mut Machine, fault: Fault, up: &mut Queue<Event>, down: &mut Queue<stream::Down>) {
    if machine.phase == Phase::Closed {
        return;
    }
    let finishing = machine.phase == Phase::Closing;
    machine.phase = Phase::Closing;
    machine.input = Input::Stopped;
    machine.reading = false;
    machine.fault = Some(fault);
    if machine.outstanding {
        send(down, stream::Down::Demand { read: stream::Read::Nothing, room: 0 });
        machine.outstanding = false;
    }
    for _ in 0..machine.output.len() {
        let frame = machine.output.pop().expect("the initial queue length bounds draining");
        drop(frame);
    }
    machine.pending_bytes = 0;
    let reason = match fault {
        Fault::Framing => Some(6),
        Fault::Limits => Some(3),
        Fault::Version => Some(1),
        Fault::End | Fault::Stream(_) | Fault::Closed | Fault::OutputFull => None,
    };
    if let Some(reason) = reason
        && !finishing
    {
        let refusal = Message::Refuse { refuse: Refuse { reason, text: Box::from([]) } };
        // Refusals are best effort when the stream has already granted room.
        let frozen = Sizes::STARTING;
        if let Some(frame) = codec::encode(&refusal, &frozen) {
            let length = u32::try_from(frame.len()).expect("a frozen refusal is bounded");
            if length <= machine.grant {
                machine.grant = machine.grant.checked_sub(length).expect("refusal fits credit");
                send(down, stream::Down::Send(frame));
            }
        }
    }
    if !finishing {
        send(down, stream::Down::Finish);
    }
    machine.phase = Phase::Closed;
    emit(up, Event::Closed { fault });
}
fn allowed(machine: &Machine, kind: u16) -> bool {
    if !machine.endpoint.receives(kind) {
        return false;
    }
    if kind == 3 {
        return true;
    }
    match machine.phase {
        Phase::Opening => match machine.endpoint {
            Endpoint::Engine | Endpoint::Agent => kind == 1,
            Endpoint::WorkerLink | Endpoint::WorkerAgent => machine.sent_open && kind == 2,
        },
        Phase::Terms => kind == 16,
        Phase::Hello => kind == 0x101 || kind == 4,
        Phase::Starting => kind == 0x281,
        Phase::Open => kind > 16 || kind == 4,
        Phase::Authorizing | Phase::Finished | Phase::Closing | Phase::Closed => false,
    }
}
fn opening_ok(open: &Open, endpoint: Endpoint) -> bool {
    if open.channel != endpoint.channel() || open.lowest > open.highest {
        return false;
    }
    match endpoint {
        Endpoint::Engine | Endpoint::WorkerLink => true,
        Endpoint::WorkerAgent | Endpoint::Agent => open.name.is_empty() && open.secret.is_empty(),
    }
}
fn terms(machine: &mut Machine, sizes: &Sizes) -> Option<()> {
    let terms = sizes::terms(machine.endpoint, sizes)?;
    queue(machine, &Message::Terms { terms }, sizes)?;
    machine.sent_terms = true;
    Some(())
}
fn accept(machine: &mut Machine, version: u16, sizes: &Sizes) -> Option<()> {
    if version != 1 {
        return None;
    }
    queue(machine, &Message::Accept { version }, sizes)?;
    terms(machine, sizes)?;
    machine.version = version;
    machine.phase = Phase::Terms;
    machine.reading = true;
    Some(())
}
#[expect(clippy::too_many_lines, reason = "exhaustive frame transitions mirror channel.md opening table")]
fn deliver(
    machine: &mut Machine,
    message: Message,
    sizes: &Sizes,
    up: &mut Queue<Event>,
    down: &mut Queue<stream::Down>,
) {
    machine.received_frames = machine.received_frames.saturating_add(1);
    match message {
        Message::Open { open } => {
            if !opening_ok(&open, machine.endpoint) {
                stop(machine, Fault::Framing, up, down);
                return;
            }
            if open.lowest > 1 || open.highest < 1 {
                stop(machine, Fault::Version, up, down);
                return;
            }
            match machine.endpoint {
                Endpoint::Engine => {
                    machine.phase = Phase::Authorizing;
                    machine.reading = false;
                    emit(up, Event::Message(Message::Open { open }));
                }
                Endpoint::Agent => {
                    if accept(machine, 1, sizes).is_none() {
                        stop(machine, Fault::OutputFull, up, down);
                    }
                }
                Endpoint::WorkerLink | Endpoint::WorkerAgent => {
                    stop(machine, Fault::Framing, up, down);
                }
            }
        }
        Message::Accept { version } => {
            if version != 1 {
                stop(machine, Fault::Version, up, down);
                return;
            }
            machine.version = version;
            machine.phase = Phase::Terms;
            if terms(machine, sizes).is_none() {
                stop(machine, Fault::OutputFull, up, down);
            }
        }
        Message::Terms { terms } => {
            if sizes::check_terms(machine.endpoint, &terms, sizes).is_none() {
                stop(machine, Fault::Limits, up, down);
                return;
            }
            machine.got_terms = true;
            if !machine.sent_terms {
                stop(machine, Fault::Framing, up, down);
                return;
            }
            machine.phase = match machine.endpoint {
                Endpoint::Engine => Phase::Hello,
                Endpoint::Agent => Phase::Starting,
                Endpoint::WorkerLink | Endpoint::WorkerAgent => Phase::Open,
            };
            machine.reading = machine.endpoint != Endpoint::WorkerAgent;
            emit(up, Event::Ready { version: machine.version });
        }
        Message::Refuse { refuse } => {
            emit(up, Event::Message(Message::Refuse { refuse }));
            stop(machine, Fault::Closed, up, down);
        }
        Message::Ping => {}
        Message::Hello { slots, workstreams, hosting } => {
            if machine.phase != Phase::Hello {
                stop(machine, Fault::Framing, up, down);
                return;
            }
            machine.phase = Phase::Open;
            machine.reading = false;
            emit(up, Event::Message(Message::Hello { slots, workstreams, hosting }));
        }
        Message::AgentStart { charter, snapshot, repositories, endpoints, grants } => {
            if machine.phase != Phase::Starting {
                stop(machine, Fault::Framing, up, down);
                return;
            }
            machine.phase = Phase::Open;
            machine.reading = false;
            emit(up, Event::Message(Message::AgentStart { charter, snapshot, repositories, endpoints, grants }));
        }
        Message::Finish { finish } => {
            // The worker domain drains stdout and enforces the agent's last-word
            // rule after this delivery; keep subsequent Reads available.
            machine.reading = false;
            emit(up, Event::Message(Message::Finish { finish }));
        }
        value @ (Message::Answer { .. }
        | Message::Relay { .. }
        | Message::Bounced { .. }
        | Message::Told { .. }
        | Message::Rejected { .. }
        | Message::Exhausted { .. }
        | Message::Assign { .. }
        | Message::Inbound { .. }
        | Message::Cancel { .. }
        | Message::Relayed { .. }
        | Message::Acknowledge { .. }
        | Message::Grant { .. }
        | Message::AgentCall { .. }
        | Message::Withdraw { .. }
        | Message::Fact { .. }
        | Message::Long { .. }
        | Message::LongDone
        | Message::Waiting { .. }
        | Message::AgentRejected { .. }
        | Message::AgentExhausted { .. }
        | Message::AgentEvent { .. }
        | Message::AgentAnswer { .. }
        | Message::AgentCancel
        | Message::AgentGrant { .. }) => {
            machine.reading = false;
            emit(up, Event::Message(value));
        }
    }
}
fn read(machine: &Machine, limits: &Limits) -> stream::Read {
    if !machine.reading || machine.read_ended {
        return stream::Read::Nothing;
    }
    match &machine.input {
        Input::Header => stream::Read::Fill(8),
        Input::Body { length, body, .. } => {
            let done = u32::try_from(body.written()).expect("body length is u32");
            let rest = length.checked_sub(done).expect("body never exceeds its header");
            stream::Read::Fill(rest.min(limits.chunk))
        }
        Input::Held(_) | Input::Stopped => stream::Read::Nothing,
    }
}
/// Flush at most one frame and state at most one demand per invocation.
pub fn poll(
    machine: &mut Machine,
    limits: &Limits,
    sizes: &Sizes,
    up: &mut Queue<Event>,
    down: &mut Queue<stream::Down>,
) {
    if machine.phase == Phase::Closed || machine.phase == Phase::Closing {
        return;
    }
    let source = core::mem::replace(&mut machine.input, Input::Stopped);
    machine.input = match source {
        Input::Held(message) => {
            deliver(machine, message, sizes, up, down);
            Input::Header
        }
        Input::Header => Input::Header,
        Input::Body { kind, length, body } => Input::Body { kind, length, body },
        Input::Stopped => Input::Stopped,
    };
    if machine.phase == Phase::Closed {
        machine.input = Input::Stopped;
        return;
    }
    let fits = match machine.output.iter().next() {
        Some(frame) => frame.len() <= usize::try_from(machine.grant).expect("u32 fits usize"),
        None => false,
    };
    if fits {
        let frame = machine.output.pop().expect("the front exists");
        let length = u32::try_from(frame.len()).expect("a frame length is u32");
        machine.pending_bytes = machine.pending_bytes.checked_sub(length).expect("queued length counted");
        machine.grant = machine.grant.checked_sub(length).expect("frame fits grant");
        machine.sent_bytes = machine.sent_bytes.checked_add(length).expect("output fits cap");
        if machine.outstanding && machine.demand_room > 0 {
            machine.demand_sent = machine.demand_sent.checked_add(length).expect("post-demand sends fit held credit");
        }
        send(down, stream::Down::Send(frame));
    }
    if machine.phase == Phase::Finished && machine.output.is_empty() {
        if machine.outstanding {
            send(down, stream::Down::Demand { read: stream::Read::Nothing, room: 0 });
            machine.outstanding = false;
        }
        send(down, stream::Down::Finish);
        machine.phase = Phase::Closing;
        return;
    }
    if !machine.outstanding {
        let read = read(machine, limits);
        let room = if machine.grant < machine.cap { machine.cap } else { 0 };
        if read != stream::Read::Nothing || room > 0 {
            machine.demand_read = read;
            machine.demand_room = room;
            machine.demand_sent = 0;
            machine.outstanding = true;
            send(down, stream::Down::Demand { read, room });
        }
    }
}
/// Consume one exact read answer. Input allocation follows a validated header.
fn receive(machine: &mut Machine, sizes: &Sizes, bytes: &[u8]) -> Option<()> {
    if !machine.outstanding {
        return None;
    }
    machine.outstanding = false;
    let exact = match machine.demand_read {
        stream::Read::Fill(count) => count,
        stream::Read::Nothing | stream::Read::Scan { .. } | stream::Read::Line { .. } => 0,
    };
    if u32::try_from(bytes.len()).ok() != Some(exact) {
        return None;
    }
    let source = core::mem::replace(&mut machine.input, Input::Stopped);
    machine.input = match source {
        Input::Header => {
            let header = codec::header(bytes, sizes)?;
            if !allowed(machine, header.kind) {
                return None;
            }
            if header.length == 0 {
                Input::Held(codec::body(header.kind, &[], sizes)?)
            } else {
                Input::Body {
                    kind: header.kind,
                    length: header.length,
                    body: Writer::new(usize::try_from(header.length).expect("u32 fits usize")),
                }
            }
        }
        Input::Body { kind, length, mut body } => {
            body.put(bytes).ok()?;
            if body.room() == 0 {
                let bytes = body.finish();
                Input::Held(codec::body(kind, &bytes, sizes)?)
            } else {
                Input::Body { kind, length, body }
            }
        }
        Input::Held(_) | Input::Stopped => return None,
    };
    Some(())
}
/// One exact stream delivery, never a loop over buffered frames.
pub fn up(
    machine: &mut Machine,
    limits: &Limits,
    sizes: &Sizes,
    event: stream::Up,
    out: &mut Queue<Event>,
    down: &mut Queue<stream::Down>,
) {
    if machine.phase == Phase::Closed {
        return;
    }
    if machine.phase == Phase::Closing {
        match event {
            stream::Up::Bytes(_) | stream::Up::Room | stream::Up::End => return,
            stream::Up::Failed(fault) => {
                stop(machine, Fault::Stream(fault), out, down);
                return;
            }
        }
    }
    if machine.phase == Phase::Finished || machine.read_ended {
        match &event {
            stream::Up::Bytes(_) => {
                // A withdrawn read can already have answered into the parent
                // queue. It cannot answer a newer room-only demand.
                if machine.outstanding && machine.demand_read != stream::Read::Nothing {
                    machine.outstanding = false;
                }
                poll(machine, limits, sizes, out, down);
                return;
            }
            stream::Up::End => {
                end_read(machine, down);
                poll(machine, limits, sizes, out, down);
                return;
            }
            stream::Up::Room | stream::Up::Failed(_) => {}
        }
    }
    match event {
        stream::Up::Bytes(bytes) => {
            if receive(machine, sizes, &bytes).is_none() {
                stop(machine, Fault::Framing, out, down);
                return;
            }
        }
        stream::Up::Room => {
            if !machine.outstanding || machine.demand_room == 0 {
                stop(machine, Fault::Framing, out, down);
                return;
            }
            machine.outstanding = false;
            // Room may already be queued above the stream while held credit is
            // consumed. Debit every Send since Demand, even if the lower side
            // produced Room later; conservatism is preferable to double credit.
            machine.grant = machine.demand_room.checked_sub(machine.demand_sent).expect("sent only held credit");
            machine.sent_bytes = machine.demand_sent;
        }
        stream::Up::End => {
            if machine.endpoint == Endpoint::Agent && machine.phase == Phase::Open {
                if !machine.read_ended {
                    machine.read_ended = true;
                    machine.reading = false;
                    if machine.outstanding && machine.demand_room == 0 {
                        // EOF closes the read half. The lower side may retain
                        // its unanswerable read demand until explicitly withdrawn.
                        send(down, stream::Down::Demand { read: stream::Read::Nothing, room: 0 });
                        machine.outstanding = false;
                    }
                    machine.input = Input::Stopped;
                    emit(out, Event::ReadEnded);
                }
            } else {
                stop(machine, Fault::End, out, down);
            }
            return;
        }
        stream::Up::Failed(fault) => {
            stop(machine, Fault::Stream(fault), out, down);
            return;
        }
    }
    poll(machine, limits, sizes, out, down);
}
#[expect(clippy::too_many_lines, reason = "exhaustive request validation and queueing for both directions")]
pub fn down(
    machine: &mut Machine,
    limits: &Limits,
    sizes: &Sizes,
    request: Request,
    out: &mut Queue<Event>,
    lower: &mut Queue<stream::Down>,
) {
    match request {
        Request::Read => {
            if machine.phase != Phase::Closed
                && machine.phase != Phase::Closing
                && machine.phase != Phase::Finished
                && !machine.read_ended
            {
                machine.reading = true;
            }
        }
        Request::Close => {
            stop(machine, Fault::Closed, out, lower);
            return;
        }
        Request::Accept { version } => {
            if machine.phase == Phase::Closed || machine.phase == Phase::Closing || machine.phase == Phase::Finished {
                return;
            }
            if machine.phase != Phase::Authorizing || accept(machine, version, sizes).is_none() {
                stop(machine, Fault::Framing, out, lower);
                return;
            }
        }
        Request::Refuse(refuse) => {
            if machine.phase == Phase::Closed || machine.phase == Phase::Closing || machine.phase == Phase::Finished {
                return;
            }
            if queue(machine, &Message::Refuse { refuse }, sizes).is_none() {
                stop(machine, Fault::OutputFull, out, lower);
                return;
            }
            machine.phase = Phase::Finished;
            end_read(machine, lower);
        }
        Request::Send(message) => {
            if machine.phase == Phase::Closed || machine.phase == Phase::Closing || machine.phase == Phase::Finished {
                emit(out, Event::Unsent);
                return;
            }
            let kind = message.kind();
            if !machine.endpoint.sends(kind) {
                stop(machine, Fault::Framing, out, lower);
                return;
            }
            match &message {
                Message::Open { open } => {
                    if machine.phase != Phase::Opening
                        || machine.sent_open
                        || !opening_ok(open, machine.endpoint)
                        || open.lowest > 1
                        || open.highest < 1
                    {
                        stop(machine, Fault::Framing, out, lower);
                        return;
                    }
                    machine.sent_open = true;
                }
                Message::Hello { .. } | Message::AgentStart { .. } => {
                    if machine.phase != Phase::Open || machine.first_sent {
                        stop(machine, Fault::Framing, out, lower);
                        return;
                    }
                    machine.first_sent = true;
                }
                Message::Accept { .. } | Message::Terms { .. } | Message::Refuse { .. } => {
                    stop(machine, Fault::Framing, out, lower);
                    return;
                }
                Message::Ping => {
                    if machine.phase != Phase::Open && machine.phase != Phase::Hello {
                        stop(machine, Fault::Framing, out, lower);
                        return;
                    }
                }
                Message::Finish { .. } => {
                    if machine.phase != Phase::Open {
                        stop(machine, Fault::Framing, out, lower);
                        return;
                    }
                }
                Message::Answer { .. }
                | Message::Relay { .. }
                | Message::Bounced { .. }
                | Message::Told { .. }
                | Message::Rejected { .. }
                | Message::Exhausted { .. }
                | Message::Assign { .. }
                | Message::Inbound { .. }
                | Message::Cancel { .. }
                | Message::Relayed { .. }
                | Message::Acknowledge { .. }
                | Message::Grant { .. }
                | Message::AgentCall { .. }
                | Message::Withdraw { .. }
                | Message::Fact { .. }
                | Message::Long { .. }
                | Message::LongDone
                | Message::Waiting { .. }
                | Message::AgentRejected { .. }
                | Message::AgentExhausted { .. }
                | Message::AgentEvent { .. }
                | Message::AgentAnswer { .. }
                | Message::AgentCancel
                | Message::AgentGrant { .. } => {
                    if machine.phase != Phase::Open {
                        stop(machine, Fault::Framing, out, lower);
                        return;
                    }
                    match machine.endpoint {
                        Endpoint::WorkerLink | Endpoint::WorkerAgent => {
                            if !machine.first_sent {
                                stop(machine, Fault::Framing, out, lower);
                                return;
                            }
                        }
                        Endpoint::Engine | Endpoint::Agent => {}
                    }
                }
            }
            if queue(machine, &message, sizes).is_none() {
                emit(out, Event::Unsent);
                stop(machine, Fault::OutputFull, out, lower);
                return;
            }
            if kind == 0x207 {
                machine.phase = Phase::Finished;
                end_read(machine, lower);
            }
            emit(out, Event::Sent);
        }
    }
    poll(machine, limits, sizes, out, lower);
}

/// Closing the input half leaves a joined read/room demand available for its
/// one answer; a read-only demand is explicitly withdrawn before asking room.
fn end_read(machine: &mut Machine, lower: &mut Queue<stream::Down>) {
    machine.reading = false;
    machine.input = Input::Stopped;
    if machine.outstanding && machine.demand_room == 0 {
        send(lower, stream::Down::Demand { read: stream::Read::Nothing, room: 0 });
        machine.outstanding = false;
    }
}

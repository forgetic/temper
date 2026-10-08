//! Compatibility names for the engine boundary. The root is split by the parts of
//! jig's domain/root.md; new callers use the crate-level entry points.
pub use crate::boundary::*;
pub use crate::domain::{Domain, fire, max_out};
pub use crate::limits::{BriefBudgets, BriefLimits, Limits, worst_case};
pub use crate::route::{
    Approval, Freshness, Gate, HostDelivery, HostMessage, HostRequest, LandingRule, adopt_repository_ask,
};
pub use crate::translate::{Config, LandingPolicy};
pub use jig_core::{Delegate, Dependency, Model, ProcedureAction, RunCharter, RunPolicy};

use crate::{Delivery, Key, Range, Record, Write};
use alloc::boxed::Box;
use jig_core_accounts as accounts;
use jig_core_fleet as fleet;
use jig_core_people as people;
use jig_core_tasks as tasks;
use jig_core_views as views;
use skein_lib::{Env, Queue, ReplyTo, Token};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_client as forge_client;

/// Compatibility constructors; every input forwards to the typed peer union.
#[derive(Debug)]
pub enum Event {
    Released(Released),
    HostCall {
        channel: Token,
        task: u64,
        attempt: u64,
        call: fleet::Call,
    },
    DecodedHostCall {
        to: ReplyTo,
        body: Call,
    },
    RenderedHostCall {
        to: ReplyTo,
        answer: jig_core::SettledAnswer,
    },
    Inbound {
        task: u64,
        attempt: u64,
        message: HostMessage,
    },
    ForgeAnswered {
        call: Token,
        cost: u32,
        result: Result<forge_client::api::Answer, forge_client::api::Error>,
    },
    ForgeHint {
        hint: forge_client::api::Hint,
    },
    Watch {
        watcher: Token,
        sign_in: u64,
        key: [u8; 16],
        subject: views::Subject,
    },
    Unwatch {
        watcher: Token,
    },
    ViewDelivered {
        watcher: Token,
        done: bool,
    },
    StartRecurring {
        project: u32,
        authority: tasks::Authority,
        template: tasks::RecurringTemplate,
    },
    Period {
        project: u32,
        period: u64,
        budget: u64,
    },
    ProcedureStep {
        task: u64,
        step: u64,
        connector: u16,
        code: u32,
        action: ProcedureAction,
    },
    Call {
        channel: Token,
        task: u64,
        attempt: u64,
        call: Token,
        body: Call,
    },
    ReadEscalation {
        /// One web-issued reply right.
        reply_to: ReplyTo,
        /// Root-issued session, checked against both clocks.
        sign_in: u64,
        /// Positive chat task known from Started.
        task: u64,
    },
    Start,
    Committed {
        /// Store-echoed positive commit number, at most the latest issued commit; older cumulative
        /// answers are inert.
        number: u64,
    },
    Uncommitted {
        /// Store-echoed issued commit that failed; failures at/before durable progress or after
        /// stop are inert.
        number: u64,
    },
    Loaded {
        /// Echoed generational load token, fenced after its one terminal and reclaim.
        owner: Token,
        /// Owned decoded rows, at most `loads::Limits::rows` and `loads::Limits::reply_bytes`
        /// before restoration.
        rows: Box<[Record]>,
        /// Exclusive last-row continuation in the requested range, or terminal page.
        next: Option<Key>,
    },
    Unloaded {
        /// Echoed generational load token, fenced after its one terminal and reclaim.
        owner: Token,
    },
    SignedIn {
        /// Web-issued right to this one sign-in reply, returned busy immediately or moved through
        /// people to a durable terminal.
        reply_to: ReplyTo,
        /// Protocol-authenticated forge/user key and display bytes, bounded by people
        /// `identity_bytes`.
        identity: people::Identity,
    },
    Ask {
        /// Web-issued right to one keyed request terminal; duplicates can join bounded people
        /// waiters, each still replied to once.
        reply_to: ReplyTo,
        /// Root-issued durable secret-free session number; authenticated and expired by people.
        sign_in: u64,
        /// Web-issued fixed request key, scoped by person and persisted with the exact typed ask.
        key: [u8; 16],
        /// Typed chat request with words bounded by people words before routing.
        ask: people::Ask,
    },
    Hello {
        /// Protocol-issued opaque channel identity; at most fleet `workers` are retained cold, and
        /// fleet admits the live channel.
        channel: Token,
        /// Worker slot/host/workstream report, bounded by fleet limits before cold retention.
        hello: fleet::Hello,
    },
    Lost {
        /// Protocol-issued identity of the channel that closed; duplicates/unknown
        /// channels consume no queued handoff room.
        channel: Token,
    },
    Turn {
        /// Worker protocol sender; fleet validates it against the current attempt before handing
        /// the owned body to tasks.
        channel: Token,
        /// Positive durable task number owning this turn; fleet treats it as an opaque run
        /// identity.
        task: u64,
        /// Positive root-issued activation number; fleet fences stale worker bodies.
        attempt: u64,
        /// Owned numbered transcript and cumulative charge; the root bounds bytes before payload
        /// retention.
        turn: Turn,
    },
    Answer {
        /// Worker protocol sender checked by fleet against the hosted attempt.
        channel: Token,
        /// Durable task number for the answered attempt; it is not a fresh task allocation.
        task: u64,
        /// Positive root-issued activation number; fleet fences stale worker bodies.
        attempt: u64,
        /// Whole priced spend for this attempt; tasks checks monotonicity and semantic admission
        /// atomically, while root/fleet owns transport replay evidence and fencing.
        cumulative: u64,
        /// Owned task terminal; root retained payload bytes are at most twice tasks `result_bytes`,
        /// then tasks checks result/contract admission against its stricter result bound.
        end: tasks::End,
        /// Full set of repository tags with saved work after this terminal, when the worker made
        /// a save; absent preserves the previous set.
        saved: Option<Box<[u32]>>,
        /// Last heads actually pushed by the worker, indexed by deployment repository tag.
        pushed: Box<[forge::Pushed]>,
    },
    ReadResult {
        /// Web-issued right moved into one bounded result waiter; consumes one terminal
        /// result/refusal reply.
        reply_to: ReplyTo,
        /// Root-issued durable secret-free session number; authenticated and expired by people.
        sign_in: u64,
        /// Positive ended task number selected by the reader; its stored requester must match the
        /// authenticated person.
        task: u64,
    },
    ReadInbox {
        reply_to: ReplyTo,
        sign_in: u64,
        /// Positive maximum entries, capped by the configured per-person inbox limit.
        most: u32,
    },
    ViewInbox {
        reply_to: ReplyTo,
        sign_in: u64,
        most: u32,
        before: Option<crate::InboxCursor>,
    },
    Refreshed {
        /// Configured secret-free account number; bounded by accounts account room.
        account: u32,
        /// Echoed account refresh generation; stale completions change nothing.
        generation: u64,
        /// Remaining credential lifetime, represented by a u64 duration; no token bytes cross the
        /// domain.
        valid: skein_lib::Duration,
    },
    RefreshFailed {
        /// Configured secret-free account number; bounded by accounts account room.
        account: u32,
        /// Echoed account refresh generation; stale completions change nothing.
        generation: u64,
        /// Terminal account failure class, including retry delay where required.
        failure: accounts::Failure,
    },
}

/// Compatibility output surface for existing protocol adapters.
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    ToChild(Released),
    Host(Box<HostRequest>),
    Forge {
        call: Token,
        repository: forge_client::api::Repository,
        op: forge_client::api::Op,
    },
    View(views::Request),
    WatchRefused {
        watcher: Token,
        refusal: people::Refusal,
    },
    CallBusy {
        channel: Token,
        task: u64,
        attempt: u64,
        call: Token,
    },
    Commit {
        /// Positive ordered commit number allocated by the journal, echoed by the store terminal;
        /// checked `u64` exhaustion stops admission.
        number: u64,
        /// Owned unique-key transaction, at most journal writes including its deployment header;
        /// applied atomically.
        writes: Box<[Write]>,
    },
    Load {
        /// Fresh generational load identity issued by root loads; store echoes it once through
        /// `Loaded` or `Unloaded`.
        owner: Token,
        /// Closed store-key family; every page key is checked for membership.
        range: Range,
        /// Exclusive cursor in this range, or its beginning.
        after: Option<Key>,
        /// Positive page demand, at most the configured load row bound.
        most: u32,
        /// Hard decoded-page byte limit, enforced before and after protocol decoding.
        bytes: u32,
    },
    Deliver(
        /// Owned bounded effect, released once after the commit it follows.
        Delivery,
    ),
    Account(
        /// Fixed-size accounts request/notice; token values are filled only by the protocol and
        /// never retained here.
        accounts::Request,
    ),
    TurnBusy {
        /// Worker protocol destination echoed from its refused turn; a fixed-size identity, without
        /// admitting another worker.
        channel: Token,
        /// Task number echoed from the refused turn; this notice does not validate or allocate the
        /// task.
        task: u64,
        /// Attempt number echoed for the worker to fence the retry; no activation is admitted by
        /// this notice.
        attempt: u64,
        /// Turn number echoed from the refused body, represented by `u32`; worker retains that body
        /// for retry.
        turn: u32,
    },
    AnswerBusy {
        /// Worker protocol destination echoed from the refused terminal; no new worker is retained.
        channel: Token,
        /// Task number echoed from the refused answer; no task or charge is admitted.
        task: u64,
        /// Attempt number echoed for the worker to fence its retained terminal and retry.
        attempt: u64,
    },
    Stop,
}

/// Forward one legacy protocol input into the single admitted root.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event) {
    let event = match event {
        Event::Released(value) => crate::Event::Released(value),
        Event::HostCall { channel, task, attempt, call } => {
            crate::Event::Worker(WorkerInput::HostCall { channel, task, attempt, call })
        }
        Event::DecodedHostCall { to, body } => crate::Event::Worker(WorkerInput::DecodedHostCall { to, body }),
        Event::RenderedHostCall { to, answer } => crate::Event::Worker(WorkerInput::RenderedHostCall { to, answer }),
        Event::Inbound { task, attempt, message } => {
            crate::Event::Worker(WorkerInput::Inbound { task, attempt, message })
        }
        Event::ForgeAnswered { call, cost, result } => {
            crate::Event::Forge(Box::new(forge::Event::Client(forge_client::Event::Answered { call, cost, result })))
        }
        Event::ForgeHint { hint } => crate::Event::Forge(Box::new(forge::Event::Hint { hint })),
        Event::Watch { watcher, sign_in, key, subject } => {
            crate::Event::Party(PartyInput::Watch { watcher, sign_in, key, subject })
        }
        Event::Unwatch { watcher } => crate::Event::Party(PartyInput::Unwatch { watcher }),
        Event::ViewDelivered { watcher, done } => crate::Event::Party(PartyInput::ViewDelivered { watcher, done }),
        Event::StartRecurring { project, authority, template } => {
            crate::Event::Party(PartyInput::StartRecurring { project, authority, template })
        }
        Event::Period { project, period, budget } => {
            crate::Event::Party(PartyInput::Period { project, period, budget })
        }
        Event::ProcedureStep { task, step, connector, code, action } => {
            crate::Event::Party(PartyInput::ProcedureStep { task, step, connector, code, action })
        }
        Event::Call { channel, task, attempt, call, body } => {
            crate::Event::Worker(WorkerInput::Call { channel, task, attempt, call, body })
        }
        Event::ReadEscalation { reply_to, sign_in, task } => {
            crate::Event::Party(PartyInput::ReadEscalation { reply_to, sign_in, task })
        }
        Event::Start => crate::Event::Restart,
        Event::Committed { number } => crate::Event::Store(StoreInput::Committed { number }),
        Event::Uncommitted { number } => crate::Event::Store(StoreInput::Uncommitted { number }),
        Event::Loaded { owner, rows, next } => crate::Event::Store(StoreInput::Loaded { owner, rows, next }),
        Event::Unloaded { owner } => crate::Event::Store(StoreInput::Unloaded { owner }),
        Event::SignedIn { reply_to, identity } => crate::Event::Party(PartyInput::SignedIn { reply_to, identity }),
        Event::Ask { reply_to, sign_in, key, ask } => {
            crate::Event::Party(PartyInput::Ask { reply_to, sign_in, key, ask })
        }
        Event::Hello { channel, hello } => crate::Event::Worker(WorkerInput::Hello { channel, hello }),
        Event::Lost { channel } => crate::Event::Worker(WorkerInput::Lost { channel }),
        Event::Turn { channel, task, attempt, turn } => {
            crate::Event::Worker(WorkerInput::Turn { channel, task, attempt, turn })
        }
        Event::Answer { channel, task, attempt, cumulative, end, saved, pushed } => {
            crate::Event::Worker(WorkerInput::Answer { channel, task, attempt, cumulative, end, saved, pushed })
        }
        Event::ReadResult { reply_to, sign_in, task } => {
            crate::Event::Party(PartyInput::ReadResult { reply_to, sign_in, task })
        }
        Event::ReadInbox { reply_to, sign_in, most } => {
            crate::Event::Party(PartyInput::ReadInbox { reply_to, sign_in, most })
        }
        Event::ViewInbox { reply_to, sign_in, most, before } => {
            crate::Event::Party(PartyInput::ViewInbox { reply_to, sign_in, most, before })
        }
        Event::Refreshed { account, generation, valid } => {
            crate::Event::Account(accounts::Event::Refreshed { account, generation, valid })
        }
        Event::RefreshFailed { account, generation, failure } => {
            crate::Event::Account(accounts::Event::Failed { account, generation, failure })
        }
    };
    crate::step(domain, env, event);
}

/// Drain the same journal through the existing protocol output vocabulary.
pub fn release(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let mut released = Queue::with_capacity(max_out(&env.limits));
    let completed_step = domain.door_pass;
    crate::release(domain, env, &mut released);
    if released.is_empty() && !completed_step {
        crate::step(domain, env, crate::Event::Resume);
        crate::release(domain, env, &mut released);
    }
    for _ in 0..released.len() {
        out.push(request(released.pop().expect("root output count")));
    }
}

fn request(request: crate::Request) -> Request {
    match request {
        crate::Request::ToChild(value) => Request::ToChild(value),
        crate::Request::Worker(WorkerRequest::Host(value)) => Request::Host(value),
        crate::Request::Forge { call, repository, op } => Request::Forge { call, repository, op },
        crate::Request::Party(PartyRequest::View(value)) => Request::View(value),
        crate::Request::Party(PartyRequest::WatchRefused { watcher, refusal }) => {
            Request::WatchRefused { watcher, refusal }
        }
        crate::Request::Worker(WorkerRequest::CallBusy { channel, task, attempt, call }) => {
            Request::CallBusy { channel, task, attempt, call }
        }
        crate::Request::Store(StoreRequest::Commit { number, writes }) => Request::Commit { number, writes },
        crate::Request::Store(StoreRequest::Load { owner, range, after, most, bytes }) => {
            Request::Load { owner, range, after, most, bytes }
        }
        crate::Request::Account(value) => Request::Account(value),
        crate::Request::Worker(WorkerRequest::TurnBusy { channel, task, attempt, turn }) => {
            Request::TurnBusy { channel, task, attempt, turn }
        }
        crate::Request::Worker(WorkerRequest::AnswerBusy { channel, task, attempt }) => {
            Request::AnswerBusy { channel, task, attempt }
        }
        crate::Request::Stop => Request::Stop,
        crate::Request::Worker(WorkerRequest::Deliver(delivery))
        | crate::Request::Party(PartyRequest::Deliver(delivery)) => Request::Deliver(delivery),
    }
}

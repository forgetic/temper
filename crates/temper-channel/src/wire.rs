//! Frozen opening and the two channel vocabularies.
use crate::{
    Sizes,
    primitives::{self as p, Encoder},
};
use alloc::boxed::Box;
use skein_lib::{Duration, List, Reader};
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Term {
    pub kind: u16,
    pub largest: u32,
}
pub(crate) fn put_term(out: &mut Encoder, value: &Term, _sizes: &Sizes) -> Option<()> {
    let kind = &value.kind;
    out.u16(*kind)?;
    let largest = &value.largest;
    out.u32(*largest)?;
    Some(())
}
pub(crate) fn get_term(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Term> {
    Some(Term { kind: input.u16()?, largest: input.u32()? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Open {
    pub channel: Channel,
    pub lowest: u16,
    pub highest: u16,
    pub name: Box<[u8]>,
    pub secret: Box<[u8]>,
}
pub(crate) fn put_open(out: &mut Encoder, value: &Open, sizes: &Sizes) -> Option<()> {
    let channel = &value.channel;
    put_channel(out, channel, sizes)?;
    let lowest = &value.lowest;
    out.u16(*lowest)?;
    let highest = &value.highest;
    out.u16(*highest)?;
    let name = &value.name;
    out.bytes(name, sizes.worker_name_bytes)?;
    let secret = &value.secret;
    out.bytes(secret, sizes.secret_bytes)?;
    Some(())
}
pub(crate) fn get_open(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Open> {
    Some(Open {
        channel: get_channel(input, sizes)?,
        lowest: input.u16()?,
        highest: input.u16()?,
        name: p::bytes(input, sizes.worker_name_bytes)?,
        secret: p::bytes(input, sizes.secret_bytes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Refuse {
    pub reason: u16,
    pub text: Box<[u8]>,
}
pub(crate) fn put_refuse(out: &mut Encoder, value: &Refuse, sizes: &Sizes) -> Option<()> {
    let reason = &value.reason;
    out.u16(*reason)?;
    let text = &value.text;
    out.bytes(text, sizes.refuse_bytes)?;
    Some(())
}
pub(crate) fn get_refuse(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Refuse> {
    Some(Refuse { reason: input.u16()?, text: p::bytes(input, sizes.refuse_bytes)? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Grant {
    pub account: u32,
    pub generation: u64,
    pub valid: Duration,
    pub token: Box<[u8]>,
    pub account_id: Box<[u8]>,
}
pub(crate) fn put_grant(out: &mut Encoder, value: &Grant, sizes: &Sizes) -> Option<()> {
    let account = &value.account;
    out.u32(*account)?;
    let generation = &value.generation;
    out.u64(*generation)?;
    let valid = &value.valid;
    out.u64(valid.as_nanos())?;
    let token = &value.token;
    out.bytes(token, sizes.token_bytes)?;
    let account_id = &value.account_id;
    out.bytes(account_id, sizes.token_bytes)?;
    Some(())
}
pub(crate) fn get_grant(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Grant> {
    Some(Grant {
        account: input.u32()?,
        generation: input.u64()?,
        valid: Duration::from_nanos(input.u64()?),
        token: p::bytes(input, sizes.token_bytes)?,
        account_id: p::bytes(input, sizes.token_bytes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Workspace {
    pub key: Box<[u8]>,
    pub repositories: Box<[Repository]>,
}
pub(crate) fn put_workspace(out: &mut Encoder, value: &Workspace, sizes: &Sizes) -> Option<()> {
    let key = &value.key;
    out.bytes(key, sizes.name_bytes)?;
    let repositories = &value.repositories;
    let count_1 = u32::try_from(repositories.len()).ok()?;
    if count_1 > sizes.repositories {
        return None;
    }
    out.u32(count_1)?;
    for value in repositories {
        put_repository(out, value, sizes)?;
    }
    Some(())
}
pub(crate) fn get_workspace(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Workspace> {
    Some(Workspace {
        key: p::bytes(input, sizes.name_bytes)?,
        repositories: {
            let count_2 = p::count(input, sizes.repositories, 22)?;
            let mut values_2 = List::with_capacity(count_2);
            for _ in 0..count_2 {
                let value = get_repository(input, sizes)?;
                values_2.push(value).expect("the validated count reserves capacity");
            }
            values_2.into_boxed()
        },
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Repository {
    pub tag: u32,
    pub name: Box<[u8]>,
    pub remote: Box<[u8]>,
    pub start: Start,
    pub access: Access,
    pub identity: u32,
}
pub(crate) fn put_repository(out: &mut Encoder, value: &Repository, sizes: &Sizes) -> Option<()> {
    let tag = &value.tag;
    out.u32(*tag)?;
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let remote = &value.remote;
    out.bytes(remote, sizes.name_bytes)?;
    let start = &value.start;
    put_start(out, start, sizes)?;
    let access = &value.access;
    put_access(out, access, sizes)?;
    let identity = &value.identity;
    out.u32(*identity)?;
    Some(())
}
pub(crate) fn get_repository(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Repository> {
    Some(Repository {
        tag: input.u32()?,
        name: p::bytes(input, sizes.name_bytes)?,
        remote: p::bytes(input, sizes.name_bytes)?,
        start: get_start(input, sizes)?,
        access: get_access(input, sizes)?,
        identity: input.u32()?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hosting {
    pub run: u64,
    pub attempt: u64,
    pub phase: HostingPhase,
}
pub(crate) fn put_hosting(out: &mut Encoder, value: &Hosting, sizes: &Sizes) -> Option<()> {
    let run = &value.run;
    out.u64(*run)?;
    let attempt = &value.attempt;
    out.u64(*attempt)?;
    let phase = &value.phase;
    put_hosting_phase(out, phase, sizes)?;
    Some(())
}
pub(crate) fn get_hosting(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Hosting> {
    Some(Hosting { run: input.u64()?, attempt: input.u64()?, phase: get_hosting_phase(input, sizes)? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Landed {
    pub tag: u32,
    pub commit: [u8; 32],
}
pub(crate) fn put_landed(out: &mut Encoder, value: &Landed, _sizes: &Sizes) -> Option<()> {
    let tag = &value.tag;
    out.u32(*tag)?;
    let commit = &value.commit;
    out.raw(commit)?;
    Some(())
}
pub(crate) fn get_landed(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Landed> {
    Some(Landed { tag: input.u32()?, commit: *input.bytes(32)?.first_chunk::<32>()? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Work {
    pub landed: Box<[Landed]>,
    pub saved: Option<Box<[Landing]>>,
}
pub(crate) fn put_work(out: &mut Encoder, value: &Work, sizes: &Sizes) -> Option<()> {
    let landed = &value.landed;
    let count_3 = u32::try_from(landed.len()).ok()?;
    if count_3 > sizes.repositories {
        return None;
    }
    out.u32(count_3)?;
    for value in landed {
        put_landed(out, value, sizes)?;
    }
    let saved = &value.saved;
    match saved {
        Some(value) => {
            out.u8(1)?;
            let count_4 = u32::try_from(value.len()).ok()?;
            if count_4 > sizes.repositories {
                return None;
            }
            out.u32(count_4)?;
            for value in value {
                put_landing(out, value, sizes)?;
            }
        }
        None => {
            out.u8(0)?;
        }
    }
    Some(())
}
pub(crate) fn get_work(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Work> {
    Some(Work {
        landed: {
            let count_5 = p::count(input, sizes.repositories, 36)?;
            let mut values_5 = List::with_capacity(count_5);
            for _ in 0..count_5 {
                let value = get_landed(input, sizes)?;
                values_5.push(value).expect("the validated count reserves capacity");
            }
            values_5.into_boxed()
        },
        saved: match input.u8()? {
            0 => None,
            1 => Some({
                let count_6 = p::count(input, sizes.repositories, 1)?;
                let mut values_6 = List::with_capacity(count_6);
                for _ in 0..count_6 {
                    let value = get_landing(input, sizes)?;
                    values_6.push(value).expect("the validated count reserves capacity");
                }
                values_6.into_boxed()
            }),
            _ => return None,
        },
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PushFailure {
    pub repository: Option<u32>,
    pub reason: PushReason,
    pub output: Box<[u8]>,
    pub cut: u64,
}
pub(crate) fn put_push_failure(out: &mut Encoder, value: &PushFailure, sizes: &Sizes) -> Option<()> {
    let repository = &value.repository;
    match repository {
        Some(value) => {
            out.u8(1)?;
            out.u32(*value)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let reason = &value.reason;
    put_push_reason(out, reason, sizes)?;
    let output = &value.output;
    out.bytes(output, sizes.diagnostic_bytes)?;
    let cut = &value.cut;
    out.u64(*cut)?;
    Some(())
}
pub(crate) fn get_push_failure(input: &mut Reader<'_>, sizes: &Sizes) -> Option<PushFailure> {
    Some(PushFailure {
        repository: match input.u8()? {
            0 => None,
            1 => Some(input.u32()?),
            _ => return None,
        },
        reason: get_push_reason(input, sizes)?,
        output: p::bytes(input, sizes.diagnostic_bytes)?,
        cut: input.u64()?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentRepository {
    pub name: Box<[u8]>,
    pub writable: bool,
}
pub(crate) fn put_agent_repository(out: &mut Encoder, value: &AgentRepository, sizes: &Sizes) -> Option<()> {
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let writable = &value.writable;
    out.bool(*writable)?;
    Some(())
}
pub(crate) fn get_agent_repository(input: &mut Reader<'_>, sizes: &Sizes) -> Option<AgentRepository> {
    Some(AgentRepository { name: p::bytes(input, sizes.name_bytes)?, writable: p::boolean(input)? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EndpointDescriptor {
    pub endpoint: u32,
    pub provider: Provider,
    pub host: Box<[u8]>,
    pub address: Address,
    pub port: u16,
    pub path: Box<[u8]>,
    pub account: u32,
    pub effort: Box<[u8]>,
    pub thinking: Option<u32>,
}
pub(crate) fn put_endpoint_descriptor(out: &mut Encoder, value: &EndpointDescriptor, sizes: &Sizes) -> Option<()> {
    let endpoint = &value.endpoint;
    out.u32(*endpoint)?;
    let provider = &value.provider;
    put_provider(out, provider, sizes)?;
    let host = &value.host;
    out.bytes(host, sizes.name_bytes)?;
    let address = &value.address;
    put_address(out, address, sizes)?;
    let port = &value.port;
    out.u16(*port)?;
    let path = &value.path;
    out.bytes(path, sizes.name_bytes)?;
    let account = &value.account;
    out.u32(*account)?;
    let effort = &value.effort;
    out.bytes(effort, sizes.name_bytes)?;
    let thinking = &value.thinking;
    match thinking {
        Some(value) => {
            out.u8(1)?;
            out.u32(*value)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    Some(())
}
pub(crate) fn get_endpoint_descriptor(input: &mut Reader<'_>, sizes: &Sizes) -> Option<EndpointDescriptor> {
    Some(EndpointDescriptor {
        endpoint: input.u32()?,
        provider: get_provider(input, sizes)?,
        host: p::bytes(input, sizes.name_bytes)?,
        address: get_address(input, sizes)?,
        port: input.u16()?,
        path: p::bytes(input, sizes.name_bytes)?,
        account: input.u32()?,
        effort: p::bytes(input, sizes.name_bytes)?,
        thinking: match input.u8()? {
            0 => None,
            1 => Some(input.u32()?),
            _ => return None,
        },
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Channel {
    Link,
    Agent,
}
pub(crate) fn put_channel(out: &mut Encoder, value: &Channel, _sizes: &Sizes) -> Option<()> {
    match value {
        Channel::Link => {
            out.u8(1)?;
        }
        Channel::Agent => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_channel(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Channel> {
    Some(match input.u8()? {
        1 => Channel::Link,
        2 => Channel::Agent,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RefusalReason {
    Version,
    Unauthorized,
    Limits,
    Busy,
    Replaced,
    Framing,
    Other,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Start {
    Base { branch: Box<[u8]> },
    Branch { branch: Box<[u8]> },
    Commit { commit: [u8; 32] },
    Saved { branch: Box<[u8]> },
}
pub(crate) fn put_start(out: &mut Encoder, value: &Start, sizes: &Sizes) -> Option<()> {
    match value {
        Start::Base { branch } => {
            out.u8(0)?;
            out.bytes(branch, sizes.name_bytes)?;
        }
        Start::Branch { branch } => {
            out.u8(1)?;
            out.bytes(branch, sizes.name_bytes)?;
        }
        Start::Commit { commit } => {
            out.u8(2)?;
            out.raw(commit)?;
        }
        Start::Saved { branch } => {
            out.u8(3)?;
            out.bytes(branch, sizes.name_bytes)?;
        }
    }
    Some(())
}
pub(crate) fn get_start(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Start> {
    Some(match input.u8()? {
        0 => Start::Base { branch: p::bytes(input, sizes.name_bytes)? },
        1 => Start::Branch { branch: p::bytes(input, sizes.name_bytes)? },
        2 => Start::Commit { commit: *input.bytes(32)?.first_chunk::<32>()? },
        3 => Start::Saved { branch: p::bytes(input, sizes.name_bytes)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Access {
    ReadOnly,
    Writable { push: Box<[u8]> },
}
pub(crate) fn put_access(out: &mut Encoder, value: &Access, sizes: &Sizes) -> Option<()> {
    match value {
        Access::ReadOnly => {
            out.u8(0)?;
        }
        Access::Writable { push } => {
            out.u8(1)?;
            out.bytes(push, sizes.name_bytes)?;
        }
    }
    Some(())
}
pub(crate) fn get_access(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Access> {
    Some(match input.u8()? {
        0 => Access::ReadOnly,
        1 => Access::Writable { push: p::bytes(input, sizes.name_bytes)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostingPhase {
    Preparing,
    Starting,
    Active,
    Waiting,
    Ending,
    /// The worker retains the answer until the engine acknowledges it.
    Answered,
}
pub(crate) fn put_hosting_phase(out: &mut Encoder, value: &HostingPhase, _sizes: &Sizes) -> Option<()> {
    match value {
        HostingPhase::Preparing => {
            out.u8(0)?;
        }
        HostingPhase::Starting => {
            out.u8(1)?;
        }
        HostingPhase::Active => {
            out.u8(2)?;
        }
        HostingPhase::Waiting => {
            out.u8(3)?;
        }
        HostingPhase::Ending => {
            out.u8(4)?;
        }
        HostingPhase::Answered => {
            out.u8(5)?;
        }
    }
    Some(())
}
pub(crate) fn get_hosting_phase(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<HostingPhase> {
    Some(match input.u8()? {
        0 => HostingPhase::Preparing,
        1 => HostingPhase::Starting,
        2 => HostingPhase::Active,
        3 => HostingPhase::Waiting,
        4 => HostingPhase::Ending,
        5 => HostingPhase::Answered,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Landing {
    Explained { failure: PushFailure },
    Landed { commit: [u8; 32] },
    Moved,
    Failed,
    Refused,
    Unchanged,
}
pub(crate) fn put_landing(out: &mut Encoder, value: &Landing, sizes: &Sizes) -> Option<()> {
    match value {
        Landing::Explained { failure } => {
            out.u8(0)?;
            put_push_failure(out, failure, sizes)?;
        }
        Landing::Landed { commit } => {
            out.u8(1)?;
            out.raw(commit)?;
        }
        Landing::Moved => {
            out.u8(2)?;
        }
        Landing::Failed => {
            out.u8(3)?;
        }
        Landing::Refused => {
            out.u8(4)?;
        }
        Landing::Unchanged => {
            out.u8(5)?;
        }
    }
    Some(())
}
pub(crate) fn get_landing(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Landing> {
    Some(match input.u8()? {
        0 => Landing::Explained { failure: get_push_failure(input, sizes)? },
        1 => Landing::Landed { commit: *input.bytes(32)?.first_chunk::<32>()? },
        2 => Landing::Moved,
        3 => Landing::Failed,
        4 => Landing::Refused,
        5 => Landing::Unchanged,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PushReason {
    MissingRepository,
    MissingBranch,
    MissingCommit,
    Refused,
    Unreachable,
    Broken,
    TimedOut,
    Cancelled,
    Unavailable,
    Busy,
    TooLarge,
    Nothing,
    Unknown,
}
pub(crate) fn put_push_reason(out: &mut Encoder, value: &PushReason, _sizes: &Sizes) -> Option<()> {
    match value {
        PushReason::MissingRepository => {
            out.u8(0)?;
        }
        PushReason::MissingBranch => {
            out.u8(1)?;
        }
        PushReason::MissingCommit => {
            out.u8(2)?;
        }
        PushReason::Refused => {
            out.u8(3)?;
        }
        PushReason::Unreachable => {
            out.u8(4)?;
        }
        PushReason::Broken => {
            out.u8(5)?;
        }
        PushReason::TimedOut => {
            out.u8(6)?;
        }
        PushReason::Cancelled => {
            out.u8(7)?;
        }
        PushReason::Unavailable => {
            out.u8(8)?;
        }
        PushReason::Busy => {
            out.u8(9)?;
        }
        PushReason::TooLarge => {
            out.u8(10)?;
        }
        PushReason::Nothing => {
            out.u8(11)?;
        }
        PushReason::Unknown => {
            out.u8(12)?;
        }
    }
    Some(())
}
pub(crate) fn get_push_reason(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<PushReason> {
    Some(match input.u8()? {
        0 => PushReason::MissingRepository,
        1 => PushReason::MissingBranch,
        2 => PushReason::MissingCommit,
        3 => PushReason::Refused,
        4 => PushReason::Unreachable,
        5 => PushReason::Broken,
        6 => PushReason::TimedOut,
        7 => PushReason::Cancelled,
        8 => PushReason::Unavailable,
        9 => PushReason::Busy,
        10 => PushReason::TooLarge,
        11 => PushReason::Nothing,
        12 => PushReason::Unknown,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Push {
    Done,
    Moved,
    Failed { failure: PushFailure },
    Nothing,
}
pub(crate) fn put_push(out: &mut Encoder, value: &Push, sizes: &Sizes) -> Option<()> {
    match value {
        Push::Done => {
            out.u8(0)?;
        }
        Push::Moved => {
            out.u8(1)?;
        }
        Push::Failed { failure } => {
            out.u8(2)?;
            put_push_failure(out, failure, sizes)?;
        }
        Push::Nothing => {
            out.u8(3)?;
        }
    }
    Some(())
}
pub(crate) fn get_push(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Push> {
    Some(match input.u8()? {
        0 => Push::Done,
        1 => Push::Moved,
        2 => Push::Failed { failure: get_push_failure(input, sizes)? },
        3 => Push::Nothing,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AssignmentRefusal {
    Busy,
    Invalid { invalid: Invalid },
}
pub(crate) fn put_assignment_refusal(out: &mut Encoder, value: &AssignmentRefusal, sizes: &Sizes) -> Option<()> {
    match value {
        AssignmentRefusal::Busy => {
            out.u8(0)?;
        }
        AssignmentRefusal::Invalid { invalid } => {
            out.u8(1)?;
            put_invalid(out, invalid, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_assignment_refusal(input: &mut Reader<'_>, sizes: &Sizes) -> Option<AssignmentRefusal> {
    Some(match input.u8()? {
        0 => AssignmentRefusal::Busy,
        1 => AssignmentRefusal::Invalid { invalid: get_invalid(input, sizes)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Invalid {
    Repositories,
    Duplicate,
    Name,
    Charter,
    Snapshot,
}
pub(crate) fn put_invalid(out: &mut Encoder, value: &Invalid, _sizes: &Sizes) -> Option<()> {
    match value {
        Invalid::Repositories => {
            out.u8(0)?;
        }
        Invalid::Duplicate => {
            out.u8(1)?;
        }
        Invalid::Name => {
            out.u8(2)?;
        }
        Invalid::Charter => {
            out.u8(3)?;
        }
        Invalid::Snapshot => {
            out.u8(4)?;
        }
    }
    Some(())
}
pub(crate) fn get_invalid(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Invalid> {
    Some(match input.u8()? {
        0 => Invalid::Repositories,
        1 => Invalid::Duplicate,
        2 => Invalid::Name,
        3 => Invalid::Charter,
        4 => Invalid::Snapshot,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Failure {
    Unprepared { preparation: Preparation },
    Run { failure: RunFailure },
    Agent { failure: AgentFailure },
    Cancelled { reason: CancelReason },
}
pub(crate) fn put_failure(out: &mut Encoder, value: &Failure, sizes: &Sizes) -> Option<()> {
    match value {
        Failure::Unprepared { preparation } => {
            out.u8(0)?;
            put_preparation(out, preparation, sizes)?;
        }
        Failure::Run { failure } => {
            out.u8(1)?;
            put_run_failure(out, failure, sizes)?;
        }
        Failure::Agent { failure } => {
            out.u8(2)?;
            put_agent_failure(out, failure, sizes)?;
        }
        Failure::Cancelled { reason } => {
            out.u8(3)?;
            put_cancel_reason(out, reason, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_failure(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Failure> {
    Some(match input.u8()? {
        0 => Failure::Unprepared { preparation: get_preparation(input, sizes)? },
        1 => Failure::Run { failure: get_run_failure(input, sizes)? },
        2 => Failure::Agent { failure: get_agent_failure(input, sizes)? },
        3 => Failure::Cancelled { reason: get_cancel_reason(input, sizes)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Preparation {
    Transient,
    Missing { repository: u32, missing: Missing },
    Refused { repository: u32 },
}
pub(crate) fn put_preparation(out: &mut Encoder, value: &Preparation, sizes: &Sizes) -> Option<()> {
    match value {
        Preparation::Transient => {
            out.u8(0)?;
        }
        Preparation::Missing { repository, missing } => {
            out.u8(1)?;
            out.u32(*repository)?;
            put_missing(out, missing, sizes)?;
        }
        Preparation::Refused { repository } => {
            out.u8(2)?;
            out.u32(*repository)?;
        }
    }
    Some(())
}
pub(crate) fn get_preparation(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Preparation> {
    Some(match input.u8()? {
        0 => Preparation::Transient,
        1 => Preparation::Missing { repository: input.u32()?, missing: get_missing(input, sizes)? },
        2 => Preparation::Refused { repository: input.u32()? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Missing {
    Repository,
    Branch,
    Commit,
}
pub(crate) fn put_missing(out: &mut Encoder, value: &Missing, _sizes: &Sizes) -> Option<()> {
    match value {
        Missing::Repository => {
            out.u8(0)?;
        }
        Missing::Branch => {
            out.u8(1)?;
        }
        Missing::Commit => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_missing(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Missing> {
    Some(match input.u8()? {
        0 => Missing::Repository,
        1 => Missing::Branch,
        2 => Missing::Commit,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RunFailure {
    Model,
    Budget,
    Policy,
    Cancelled,
    Stale,
    /// The account ran out; a fresh account may succeed.
    Exhausted,
}
pub(crate) fn put_run_failure(out: &mut Encoder, value: &RunFailure, _sizes: &Sizes) -> Option<()> {
    match value {
        RunFailure::Model => {
            out.u8(0)?;
        }
        RunFailure::Budget => {
            out.u8(1)?;
        }
        RunFailure::Policy => {
            out.u8(2)?;
        }
        RunFailure::Cancelled => {
            out.u8(3)?;
        }
        RunFailure::Stale => {
            out.u8(4)?;
        }
        RunFailure::Exhausted => {
            out.u8(5)?;
        }
    }
    Some(())
}
pub(crate) fn get_run_failure(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<RunFailure> {
    Some(match input.u8()? {
        0 => RunFailure::Model,
        1 => RunFailure::Budget,
        2 => RunFailure::Policy,
        3 => RunFailure::Cancelled,
        4 => RunFailure::Stale,
        5 => RunFailure::Exhausted,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AgentFailure {
    Unstarted,
    Exited,
    Rules,
    NoProgress,
    WallTime,
}
pub(crate) fn put_agent_failure(out: &mut Encoder, value: &AgentFailure, _sizes: &Sizes) -> Option<()> {
    match value {
        AgentFailure::Unstarted => {
            out.u8(0)?;
        }
        AgentFailure::Exited => {
            out.u8(1)?;
        }
        AgentFailure::Rules => {
            out.u8(2)?;
        }
        AgentFailure::NoProgress => {
            out.u8(3)?;
        }
        AgentFailure::WallTime => {
            out.u8(4)?;
        }
    }
    Some(())
}
pub(crate) fn get_agent_failure(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<AgentFailure> {
    Some(match input.u8()? {
        0 => AgentFailure::Unstarted,
        1 => AgentFailure::Exited,
        2 => AgentFailure::Rules,
        3 => AgentFailure::NoProgress,
        4 => AgentFailure::WallTime,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CancelReason {
    Engine,
    Contact,
    Shutdown,
}
pub(crate) fn put_cancel_reason(out: &mut Encoder, value: &CancelReason, _sizes: &Sizes) -> Option<()> {
    match value {
        CancelReason::Engine => {
            out.u8(0)?;
        }
        CancelReason::Contact => {
            out.u8(1)?;
        }
        CancelReason::Shutdown => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_cancel_reason(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<CancelReason> {
    Some(match input.u8()? {
        0 => CancelReason::Engine,
        1 => CancelReason::Contact,
        2 => CancelReason::Shutdown,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LinkAnswer {
    Refused { refusal: AssignmentRefusal },
    Ended { outcome: Box<[u8]>, work: Work },
    Parked { snapshot: Option<Box<[u8]>>, work: Work },
    Failed { failure: Failure, detail: Box<[u8]>, work: Work },
}
pub(crate) fn put_link_answer(out: &mut Encoder, value: &LinkAnswer, sizes: &Sizes) -> Option<()> {
    match value {
        LinkAnswer::Refused { refusal } => {
            out.u8(0)?;
            put_assignment_refusal(out, refusal, sizes)?;
        }
        LinkAnswer::Ended { outcome, work } => {
            out.u8(1)?;
            out.bytes(outcome, sizes.outcome)?;
            put_work(out, work, sizes)?;
        }
        LinkAnswer::Parked { snapshot, work } => {
            out.u8(2)?;
            match snapshot {
                Some(value) => {
                    out.u8(1)?;
                    out.bytes(value, sizes.snapshot)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
            put_work(out, work, sizes)?;
        }
        LinkAnswer::Failed { failure, detail, work } => {
            out.u8(3)?;
            put_failure(out, failure, sizes)?;
            out.bytes(detail, sizes.detail)?;
            put_work(out, work, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_link_answer(input: &mut Reader<'_>, sizes: &Sizes) -> Option<LinkAnswer> {
    Some(match input.u8()? {
        0 => LinkAnswer::Refused { refusal: get_assignment_refusal(input, sizes)? },
        1 => LinkAnswer::Ended { outcome: p::bytes(input, sizes.outcome)?, work: get_work(input, sizes)? },
        2 => LinkAnswer::Parked {
            snapshot: match input.u8()? {
                0 => None,
                1 => Some(p::bytes(input, sizes.snapshot)?),
                _ => return None,
            },
            work: get_work(input, sizes)?,
        },
        3 => LinkAnswer::Failed {
            failure: get_failure(input, sizes)?,
            detail: p::bytes(input, sizes.detail)?,
            work: get_work(input, sizes)?,
        },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ask {
    Push { message: Box<[u8]> },
    Relay { body: Box<[u8]> },
}
pub(crate) fn put_ask(out: &mut Encoder, value: &Ask, sizes: &Sizes) -> Option<()> {
    match value {
        Ask::Push { message } => {
            out.u8(0)?;
            out.bytes(message, sizes.detail)?;
        }
        Ask::Relay { body } => {
            out.u8(1)?;
            out.bytes(body, sizes.call)?;
        }
    }
    Some(())
}
pub(crate) fn get_ask(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Ask> {
    Some(match input.u8()? {
        0 => Ask::Push { message: p::bytes(input, sizes.detail)? },
        1 => Ask::Relay { body: p::bytes(input, sizes.call)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reply {
    Relayed { answer: Box<[u8]> },
    Pushed { push: Push },
    Unavailable,
    Busy,
    Withdrawn,
    TooLarge,
}
pub(crate) fn put_reply(out: &mut Encoder, value: &Reply, sizes: &Sizes) -> Option<()> {
    match value {
        Reply::Relayed { answer } => {
            out.u8(0)?;
            out.bytes(answer, sizes.answer)?;
        }
        Reply::Pushed { push } => {
            out.u8(1)?;
            put_push(out, push, sizes)?;
        }
        Reply::Unavailable => {
            out.u8(2)?;
        }
        Reply::Busy => {
            out.u8(3)?;
        }
        Reply::Withdrawn => {
            out.u8(4)?;
        }
        Reply::TooLarge => {
            out.u8(5)?;
        }
    }
    Some(())
}
pub(crate) fn get_reply(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Reply> {
    Some(match input.u8()? {
        0 => Reply::Relayed { answer: p::bytes(input, sizes.answer)? },
        1 => Reply::Pushed { push: get_push(input, sizes)? },
        2 => Reply::Unavailable,
        3 => Reply::Busy,
        4 => Reply::Withdrawn,
        5 => Reply::TooLarge,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Finish {
    Ended { outcome: Box<[u8]> },
    Parked { snapshot: Option<Box<[u8]>> },
    Failed { failure: RunFailure },
}
pub(crate) fn put_finish(out: &mut Encoder, value: &Finish, sizes: &Sizes) -> Option<()> {
    match value {
        Finish::Ended { outcome } => {
            out.u8(0)?;
            out.bytes(outcome, sizes.outcome)?;
        }
        Finish::Parked { snapshot } => {
            out.u8(1)?;
            match snapshot {
                Some(value) => {
                    out.u8(1)?;
                    out.bytes(value, sizes.snapshot)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
        Finish::Failed { failure } => {
            out.u8(2)?;
            put_run_failure(out, failure, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_finish(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Finish> {
    Some(match input.u8()? {
        0 => Finish::Ended { outcome: p::bytes(input, sizes.outcome)? },
        1 => Finish::Parked {
            snapshot: match input.u8()? {
                0 => None,
                1 => Some(p::bytes(input, sizes.snapshot)?),
                _ => return None,
            },
        },
        2 => Finish::Failed { failure: get_run_failure(input, sizes)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Bounce {
    TooLarge,
    Full,
    Ending,
}
pub(crate) fn put_bounce(out: &mut Encoder, value: &Bounce, _sizes: &Sizes) -> Option<()> {
    match value {
        Bounce::TooLarge => {
            out.u8(0)?;
        }
        Bounce::Full => {
            out.u8(1)?;
        }
        Bounce::Ending => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_bounce(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Bounce> {
    Some(match input.u8()? {
        0 => Bounce::TooLarge,
        1 => Bounce::Full,
        2 => Bounce::Ending,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Provider {
    Anthropic,
    OpenAi,
}
pub(crate) fn put_provider(out: &mut Encoder, value: &Provider, _sizes: &Sizes) -> Option<()> {
    match value {
        Provider::Anthropic => {
            out.u8(0)?;
        }
        Provider::OpenAi => {
            out.u8(1)?;
        }
    }
    Some(())
}
pub(crate) fn get_provider(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Provider> {
    Some(match input.u8()? {
        0 => Provider::Anthropic,
        1 => Provider::OpenAi,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Address {
    V4 { bytes: [u8; 4] },
    V6 { bytes: [u8; 16] },
}
pub(crate) fn put_address(out: &mut Encoder, value: &Address, _sizes: &Sizes) -> Option<()> {
    match value {
        Address::V4 { bytes } => {
            out.u8(0)?;
            out.raw(bytes)?;
        }
        Address::V6 { bytes } => {
            out.u8(1)?;
            out.raw(bytes)?;
        }
    }
    Some(())
}
pub(crate) fn get_address(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Address> {
    Some(match input.u8()? {
        0 => Address::V4 { bytes: *input.bytes(4)?.first_chunk::<4>()? },
        1 => Address::V6 { bytes: *input.bytes(16)?.first_chunk::<16>()? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Message {
    Open {
        open: Open,
    },
    Accept {
        version: u16,
    },
    Refuse {
        refuse: Refuse,
    },
    Ping,
    Terms {
        terms: Box<[Term]>,
    },
    Hello {
        slots: u32,
        workstreams: Box<[Box<[u8]>]>,
        hosting: Box<[Hosting]>,
    },
    Answer {
        run: u64,
        attempt: u64,
        answer: LinkAnswer,
    },
    Relay {
        run: u64,
        attempt: u64,
        call: u64,
        body: Box<[u8]>,
    },
    Bounced {
        run: u64,
        attempt: u64,
        event: u64,
        bounce: Bounce,
    },
    Told {
        run: u64,
        attempt: u64,
        fact: Box<[u8]>,
    },
    Rejected {
        run: u64,
        attempt: u64,
        account: u32,
        generation: u64,
    },
    Exhausted {
        run: u64,
        attempt: u64,
        account: u32,
        retry_after: Duration,
    },
    Assign {
        run: u64,
        attempt: u64,
        workspace: Workspace,
        save: Option<Box<[u8]>>,
        charter: Box<[u8]>,
        snapshot: Option<Box<[u8]>>,
        grants: Box<[Grant]>,
    },
    Inbound {
        run: u64,
        attempt: u64,
        event: u64,
        body: Box<[u8]>,
    },
    Cancel {
        run: u64,
        attempt: u64,
    },
    Relayed {
        run: u64,
        attempt: u64,
        call: u64,
        answer: Box<[u8]>,
    },
    Acknowledge {
        run: u64,
        attempt: u64,
    },
    Grant {
        run: u64,
        attempt: u64,
        grant: Grant,
    },
    AgentCall {
        call: u64,
        ask: Ask,
    },
    Withdraw {
        call: u64,
    },
    Fact {
        fact: Box<[u8]>,
    },
    Long {
        span: Duration,
    },
    LongDone,
    Waiting {
        heard: u64,
    },
    Finish {
        finish: Finish,
    },
    AgentRejected {
        account: u32,
        generation: u64,
    },
    AgentExhausted {
        account: u32,
        retry_after: Duration,
    },
    AgentStart {
        charter: Box<[u8]>,
        snapshot: Option<Box<[u8]>>,
        repositories: Box<[AgentRepository]>,
        endpoints: Box<[EndpointDescriptor]>,
        grants: Box<[Grant]>,
    },
    AgentEvent {
        event: u64,
        body: Box<[u8]>,
    },
    AgentAnswer {
        call: u64,
        reply: Reply,
    },
    AgentCancel,
    AgentGrant {
        grant: Grant,
    },
}
impl Message {
    #[must_use]
    pub const fn kind(&self) -> u16 {
        match self {
            Message::Open { .. } => 1,
            Message::Accept { .. } => 2,
            Message::Refuse { .. } => 3,
            Message::Ping => 4,
            Message::Terms { .. } => 16,
            Message::Hello { .. } => 257,
            Message::Answer { .. } => 258,
            Message::Relay { .. } => 259,
            Message::Bounced { .. } => 260,
            Message::Told { .. } => 261,
            Message::Rejected { .. } => 262,
            Message::Exhausted { .. } => 263,
            Message::Assign { .. } => 385,
            Message::Inbound { .. } => 386,
            Message::Cancel { .. } => 387,
            Message::Relayed { .. } => 388,
            Message::Acknowledge { .. } => 389,
            Message::Grant { .. } => 390,
            Message::AgentCall { .. } => 513,
            Message::Withdraw { .. } => 514,
            Message::Fact { .. } => 515,
            Message::Long { .. } => 516,
            Message::LongDone => 517,
            Message::Waiting { .. } => 518,
            Message::Finish { .. } => 519,
            Message::AgentRejected { .. } => 520,
            Message::AgentExhausted { .. } => 521,
            Message::AgentStart { .. } => 641,
            Message::AgentEvent { .. } => 642,
            Message::AgentAnswer { .. } => 643,
            Message::AgentCancel => 644,
            Message::AgentGrant { .. } => 645,
        }
    }
}
#[expect(clippy::too_many_lines, reason = "one exhaustive table mirrors every v1 wire kind")]
pub(crate) fn put_message(out: &mut Encoder, value: &Message, sizes: &Sizes) -> Option<()> {
    match value {
        Message::Open { open } => {
            out.raw(b"tmpr")?;
            put_open(out, open, sizes)?;
        }
        Message::Accept { version } => {
            out.u16(*version)?;
        }
        Message::Refuse { refuse } => {
            put_refuse(out, refuse, sizes)?;
        }
        Message::Ping | Message::LongDone | Message::AgentCancel => {}
        Message::Terms { terms } => {
            let count_7 = u32::try_from(terms.len()).ok()?;
            if count_7 > sizes.terms {
                return None;
            }
            out.u32(count_7)?;
            for value in terms {
                put_term(out, value, sizes)?;
            }
        }
        Message::Hello { slots, workstreams, hosting } => {
            out.u32(*slots)?;
            let count_8 = u32::try_from(workstreams.len()).ok()?;
            if count_8 > sizes.workstreams {
                return None;
            }
            out.u32(count_8)?;
            for value in workstreams {
                out.bytes(value, sizes.name_bytes)?;
            }
            let count_9 = u32::try_from(hosting.len()).ok()?;
            if count_9 > sizes.slots {
                return None;
            }
            out.u32(count_9)?;
            for value in hosting {
                put_hosting(out, value, sizes)?;
            }
        }
        Message::Answer { run, attempt, answer } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            put_link_answer(out, answer, sizes)?;
        }
        Message::Relay { run, attempt, call, body } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            out.u64(*call)?;
            out.bytes(body, sizes.call)?;
        }
        Message::Bounced { run, attempt, event, bounce } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            out.u64(*event)?;
            put_bounce(out, bounce, sizes)?;
        }
        Message::Told { run, attempt, fact } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            out.bytes(fact, sizes.fact)?;
        }
        Message::Rejected { run, attempt, account, generation } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            out.u32(*account)?;
            out.u64(*generation)?;
        }
        Message::Exhausted { run, attempt, account, retry_after } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            out.u32(*account)?;
            out.u64(retry_after.as_nanos())?;
        }
        Message::Assign { run, attempt, workspace, save, charter, snapshot, grants } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            put_workspace(out, workspace, sizes)?;
            match save {
                Some(value) => {
                    out.u8(1)?;
                    out.bytes(value, sizes.name_bytes)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
            out.bytes(charter, sizes.charter)?;
            match snapshot {
                Some(value) => {
                    out.u8(1)?;
                    out.bytes(value, sizes.snapshot)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
            let count_10 = u32::try_from(grants.len()).ok()?;
            if count_10 > sizes.grants {
                return None;
            }
            out.u32(count_10)?;
            for value in grants {
                put_grant(out, value, sizes)?;
            }
        }
        Message::Inbound { run, attempt, event, body } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            out.u64(*event)?;
            out.bytes(body, sizes.inbound)?;
        }
        Message::Cancel { run, attempt } | Message::Acknowledge { run, attempt } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
        }
        Message::Relayed { run, attempt, call, answer } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            out.u64(*call)?;
            out.bytes(answer, sizes.answer)?;
        }
        Message::Grant { run, attempt, grant } => {
            out.u64(*run)?;
            out.u64(*attempt)?;
            put_grant(out, grant, sizes)?;
        }
        Message::AgentCall { call, ask } => {
            out.u64(*call)?;
            put_ask(out, ask, sizes)?;
        }
        Message::Withdraw { call } => {
            out.u64(*call)?;
        }
        Message::Fact { fact } => {
            out.bytes(fact, sizes.fact)?;
        }
        Message::Long { span } => {
            out.u64(span.as_nanos())?;
        }
        Message::Waiting { heard } => {
            out.u64(*heard)?;
        }
        Message::Finish { finish } => {
            put_finish(out, finish, sizes)?;
        }
        Message::AgentRejected { account, generation } => {
            out.u32(*account)?;
            out.u64(*generation)?;
        }
        Message::AgentExhausted { account, retry_after } => {
            out.u32(*account)?;
            out.u64(retry_after.as_nanos())?;
        }
        Message::AgentStart { charter, snapshot, repositories, endpoints, grants } => {
            out.bytes(charter, sizes.charter)?;
            match snapshot {
                Some(value) => {
                    out.u8(1)?;
                    out.bytes(value, sizes.snapshot)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
            let count_11 = u32::try_from(repositories.len()).ok()?;
            if count_11 > sizes.repositories {
                return None;
            }
            out.u32(count_11)?;
            for value in repositories {
                put_agent_repository(out, value, sizes)?;
            }
            let count_12 = u32::try_from(endpoints.len()).ok()?;
            if count_12 > sizes.endpoints {
                return None;
            }
            out.u32(count_12)?;
            for value in endpoints {
                put_endpoint_descriptor(out, value, sizes)?;
            }
            let count_13 = u32::try_from(grants.len()).ok()?;
            if count_13 > sizes.grants {
                return None;
            }
            out.u32(count_13)?;
            for value in grants {
                put_grant(out, value, sizes)?;
            }
        }
        Message::AgentEvent { event, body } => {
            out.u64(*event)?;
            out.bytes(body, sizes.inbound)?;
        }
        Message::AgentAnswer { call, reply } => {
            out.u64(*call)?;
            put_reply(out, reply, sizes)?;
        }
        Message::AgentGrant { grant } => {
            put_grant(out, grant, sizes)?;
        }
    }
    Some(())
}
#[expect(clippy::too_many_lines, reason = "one exhaustive table mirrors every v1 wire kind")]
pub(crate) fn get_message(kind: u16, input: &mut Reader<'_>, sizes: &Sizes) -> Option<Message> {
    Some(match kind {
        1 => {
            if input.bytes(4)? != b"tmpr" {
                return None;
            }
            Message::Open { open: get_open(input, sizes)? }
        }
        2 => Message::Accept { version: input.u16()? },
        3 => Message::Refuse { refuse: get_refuse(input, sizes)? },
        4 => Message::Ping,
        16 => Message::Terms {
            terms: {
                let count_14 = p::count(input, sizes.terms, 6)?;
                let mut values_14 = List::with_capacity(count_14);
                for _ in 0..count_14 {
                    let value = get_term(input, sizes)?;
                    values_14.push(value).expect("the validated count reserves capacity");
                }
                values_14.into_boxed()
            },
        },
        257 => Message::Hello {
            slots: input.u32()?,
            workstreams: {
                let count_15 = p::count(input, sizes.workstreams, 4)?;
                let mut values_15 = List::with_capacity(count_15);
                for _ in 0..count_15 {
                    let value = p::bytes(input, sizes.name_bytes)?;
                    values_15.push(value).expect("the validated count reserves capacity");
                }
                values_15.into_boxed()
            },
            hosting: {
                let count_16 = p::count(input, sizes.slots, 17)?;
                let mut values_16 = List::with_capacity(count_16);
                for _ in 0..count_16 {
                    let value = get_hosting(input, sizes)?;
                    values_16.push(value).expect("the validated count reserves capacity");
                }
                values_16.into_boxed()
            },
        },
        258 => Message::Answer { run: input.u64()?, attempt: input.u64()?, answer: get_link_answer(input, sizes)? },
        259 => Message::Relay {
            run: input.u64()?,
            attempt: input.u64()?,
            call: input.u64()?,
            body: p::bytes(input, sizes.call)?,
        },
        260 => Message::Bounced {
            run: input.u64()?,
            attempt: input.u64()?,
            event: input.u64()?,
            bounce: get_bounce(input, sizes)?,
        },
        261 => Message::Told { run: input.u64()?, attempt: input.u64()?, fact: p::bytes(input, sizes.fact)? },
        262 => Message::Rejected {
            run: input.u64()?,
            attempt: input.u64()?,
            account: input.u32()?,
            generation: input.u64()?,
        },
        263 => Message::Exhausted {
            run: input.u64()?,
            attempt: input.u64()?,
            account: input.u32()?,
            retry_after: Duration::from_nanos(input.u64()?),
        },
        385 => Message::Assign {
            run: input.u64()?,
            attempt: input.u64()?,
            workspace: get_workspace(input, sizes)?,
            save: match input.u8()? {
                0 => None,
                1 => Some(p::bytes(input, sizes.name_bytes)?),
                _ => return None,
            },
            charter: p::bytes(input, sizes.charter)?,
            snapshot: match input.u8()? {
                0 => None,
                1 => Some(p::bytes(input, sizes.snapshot)?),
                _ => return None,
            },
            grants: {
                let count_17 = p::count(input, sizes.grants, 28)?;
                let mut values_17 = List::with_capacity(count_17);
                for _ in 0..count_17 {
                    let value = get_grant(input, sizes)?;
                    values_17.push(value).expect("the validated count reserves capacity");
                }
                values_17.into_boxed()
            },
        },
        386 => Message::Inbound {
            run: input.u64()?,
            attempt: input.u64()?,
            event: input.u64()?,
            body: p::bytes(input, sizes.inbound)?,
        },
        387 => Message::Cancel { run: input.u64()?, attempt: input.u64()? },
        388 => Message::Relayed {
            run: input.u64()?,
            attempt: input.u64()?,
            call: input.u64()?,
            answer: p::bytes(input, sizes.answer)?,
        },
        389 => Message::Acknowledge { run: input.u64()?, attempt: input.u64()? },
        390 => Message::Grant { run: input.u64()?, attempt: input.u64()?, grant: get_grant(input, sizes)? },
        513 => Message::AgentCall { call: input.u64()?, ask: get_ask(input, sizes)? },
        514 => Message::Withdraw { call: input.u64()? },
        515 => Message::Fact { fact: p::bytes(input, sizes.fact)? },
        516 => Message::Long { span: Duration::from_nanos(input.u64()?) },
        517 => Message::LongDone,
        518 => Message::Waiting { heard: input.u64()? },
        519 => Message::Finish { finish: get_finish(input, sizes)? },
        520 => Message::AgentRejected { account: input.u32()?, generation: input.u64()? },
        521 => Message::AgentExhausted { account: input.u32()?, retry_after: Duration::from_nanos(input.u64()?) },
        641 => Message::AgentStart {
            charter: p::bytes(input, sizes.charter)?,
            snapshot: match input.u8()? {
                0 => None,
                1 => Some(p::bytes(input, sizes.snapshot)?),
                _ => return None,
            },
            repositories: {
                let count_18 = p::count(input, sizes.repositories, 5)?;
                let mut values_18 = List::with_capacity(count_18);
                for _ in 0..count_18 {
                    let value = get_agent_repository(input, sizes)?;
                    values_18.push(value).expect("the validated count reserves capacity");
                }
                values_18.into_boxed()
            },
            endpoints: {
                let count_19 = p::count(input, sizes.endpoints, 29)?;
                let mut values_19 = List::with_capacity(count_19);
                for _ in 0..count_19 {
                    let value = get_endpoint_descriptor(input, sizes)?;
                    values_19.push(value).expect("the validated count reserves capacity");
                }
                values_19.into_boxed()
            },
            grants: {
                let count_20 = p::count(input, sizes.grants, 28)?;
                let mut values_20 = List::with_capacity(count_20);
                for _ in 0..count_20 {
                    let value = get_grant(input, sizes)?;
                    values_20.push(value).expect("the validated count reserves capacity");
                }
                values_20.into_boxed()
            },
        },
        642 => Message::AgentEvent { event: input.u64()?, body: p::bytes(input, sizes.inbound)? },
        643 => Message::AgentAnswer { call: input.u64()?, reply: get_reply(input, sizes)? },
        644 => Message::AgentCancel,
        645 => Message::AgentGrant { grant: get_grant(input, sizes)? },
        _ => return None,
    })
}

impl RefusalReason {
    #[must_use]
    pub const fn from_code(code: u16) -> RefusalReason {
        match code {
            1 => RefusalReason::Version,
            2 => RefusalReason::Unauthorized,
            3 => RefusalReason::Limits,
            4 => RefusalReason::Busy,
            5 => RefusalReason::Replaced,
            6 => RefusalReason::Framing,
            _ => RefusalReason::Other,
        }
    }
    #[must_use]
    pub const fn code(&self) -> u16 {
        match self {
            RefusalReason::Version => 1,
            RefusalReason::Unauthorized => 2,
            RefusalReason::Limits => 3,
            RefusalReason::Busy => 4,
            RefusalReason::Replaced => 5,
            RefusalReason::Framing => 6,
            RefusalReason::Other => 0,
        }
    }
}

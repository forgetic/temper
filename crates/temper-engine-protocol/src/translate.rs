//! Engine link records, with peer-controlled names checked before events exist.
use crate::{names, payload};
use alloc::boxed::Box;
use skein_lib::bytes::copy_of;
use skein_lib::{List, Time, Token};
use temper_channel::{Sizes, wire};
use temper_legacy_engine_domain::{self as engine, Event, Request};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Name,
    Repository,
    Credential,
    Payload,
    Direction,
    InvalidRelay { run: u64, attempt: u64, call: u64 },
}

/// Protocol-owned repository spelling; the domain names only its index.
#[derive(Debug)]
pub struct Repository {
    pub name: Box<[u8]>,
    pub remote: Box<[u8]>,
    pub identity: u32,
}

/// Values beside the domain's grant names. Refresh tokens never occur here.
#[expect(missing_debug_implementations, reason = "credential values must never occur in traces")]
pub struct Value {
    pub account: u32,
    pub generation: u64,
    pub expires: Time,
    pub token: Box<[u8]>,
    pub account_id: Box<[u8]>,
}

fn grant(value: engine::accounts::Grant, values: &[Value], now: Time) -> Result<wire::Grant, Error> {
    for held in values {
        if held.account == value.account && held.generation == value.generation && held.expires > now {
            return Ok(wire::Grant {
                account: value.account,
                generation: value.generation,
                valid: value.valid.min(held.expires.saturating_since(now)),
                token: copy_of(&held.token),
                account_id: copy_of(&held.account_id),
            });
        }
    }
    Err(Error::Credential)
}

fn workspace(value: engine::Workspace, configured_repositories: &[Repository]) -> Result<wire::Workspace, Error> {
    let mut repositories = List::with_capacity(u32::try_from(value.repositories.len()).ok().ok_or(Error::Payload)?);
    for checkout in value.repositories {
        let index = usize::try_from(checkout.repository).ok().ok_or(Error::Repository)?;
        let configured = configured_repositories.get(index).ok_or(Error::Repository)?;
        let start = match checkout.start {
            engine::Start::Base { branch } => wire::Start::Base { branch },
            engine::Start::Branch { branch } => wire::Start::Branch { branch },
            engine::Start::Commit { commit } => wire::Start::Commit { commit },
            engine::Start::Saved { branch } => wire::Start::Saved { branch },
        };
        let access = match checkout.push {
            Some(push) => wire::Access::Writable { push },
            None => wire::Access::ReadOnly,
        };
        let repository = wire::Repository {
            tag: checkout.repository,
            name: copy_of(&configured.name),
            remote: copy_of(&configured.remote),
            start,
            access,
            identity: configured.identity,
        };
        repositories.push(repository).expect("capacity is the source workspace length");
    }
    Ok(wire::Workspace { key: value.key, repositories: repositories.into_boxed() })
}

/// Non-link requests and grant notices whose value has already retired
/// return no message. Assignments require every named current credential.
pub fn down(
    request: Request,
    repositories: &[Repository],
    credentials: &[Value],
    now: Time,
    sizes: &Sizes,
) -> Result<Option<(Token, wire::Message)>, Error> {
    if repositories.len() > 256 {
        return Err(Error::Repository);
    }
    let pair = match request {
        Request::Assign { channel, assignment } => {
            let engine::Assignment { item, attempt, workspace: placed, save, charter, snapshot, grants } = assignment;
            let mut values = List::with_capacity(u32::try_from(grants.len()).ok().ok_or(Error::Payload)?);
            for named in grants {
                values.push(grant(named, credentials, now)?).expect("source grant count");
            }
            let charter = temper_channel::payload::v1::encode_charter(
                &payload::charter_to(charter).ok_or(Error::Payload)?,
                sizes,
            )
            .ok_or(Error::Payload)?;
            (
                channel,
                wire::Message::Assign {
                    run: names::run(item).ok_or(Error::Name)?.raw(),
                    attempt: names::attempt(item, attempt).ok_or(Error::Name)?.raw(),
                    workspace: workspace(placed, repositories)?,
                    save,
                    charter,
                    snapshot,
                    grants: values.into_boxed(),
                },
            )
        }
        Request::Inbound { channel, item, attempt, name, event } => (
            channel,
            wire::Message::Inbound {
                run: names::run(item).ok_or(Error::Name)?.raw(),
                attempt: names::attempt(item, attempt).ok_or(Error::Name)?.raw(),
                event: name.raw(),
                body: payload::encode_inbound(event, sizes).ok_or(Error::Payload)?,
            },
        ),
        Request::Cancel { channel, item, attempt } => (
            channel,
            wire::Message::Cancel {
                run: names::run(item).ok_or(Error::Name)?.raw(),
                attempt: names::attempt(item, attempt).ok_or(Error::Name)?.raw(),
            },
        ),
        Request::Relayed { channel, item, attempt, call, served } => (
            channel,
            wire::Message::Relayed {
                run: names::run(item).ok_or(Error::Name)?.raw(),
                attempt: names::attempt(item, attempt).ok_or(Error::Name)?.raw(),
                call: call.raw(),
                answer: payload::encode_served(served, sizes).ok_or(Error::Payload)?,
            },
        ),
        Request::Acknowledge { channel, item, attempt } => (
            channel,
            wire::Message::Acknowledge {
                run: names::run(item).ok_or(Error::Name)?.raw(),
                attempt: names::attempt(item, attempt).ok_or(Error::Name)?.raw(),
            },
        ),
        Request::Grant { channel, item, attempt, grant: named } => {
            let held = match grant(named, credentials, now) {
                Ok(held) => held,
                Err(Error::Credential) => return Ok(None),
                Err(
                    Error::Name | Error::Repository | Error::Payload | Error::Direction | Error::InvalidRelay { .. },
                ) => {
                    unreachable!("grant lookup only reports a missing credential")
                }
            };
            (
                channel,
                wire::Message::Grant {
                    run: names::run(item).ok_or(Error::Name)?.raw(),
                    attempt: names::attempt(item, attempt).ok_or(Error::Name)?.raw(),
                    grant: held,
                },
            )
        }
        Request::Refuse { channel } => {
            (channel, wire::Message::Refuse { refuse: wire::Refuse { reason: 4, text: copy_of(b"busy") } })
        }
        Request::Account { .. }
        | Request::Forge { .. }
        | Request::Reply { .. }
        | Request::Deliver { .. }
        | Request::Ended { .. }
        | Request::Store { .. } => return Ok(None),
    };
    // This total validation runs before a connection retains any output.
    temper_channel::codec::frame_len(&pair.1, sizes).ok_or(Error::Payload)?;
    Ok(Some(pair))
}

fn work(value: wire::Work, repositories: u32) -> Result<engine::Work, Error> {
    let mut landed = List::with_capacity(u32::try_from(value.landed.len()).ok().ok_or(Error::Payload)?);
    for entry in value.landed {
        if entry.tag >= repositories {
            return Err(Error::Repository);
        }
        landed.push(engine::Landed { repository: entry.tag, commit: entry.commit }).expect("source landing count");
    }
    Ok(engine::Work { landed: landed.into_boxed() })
}
fn failure(value: wire::Failure) -> engine::Failure {
    match value {
        wire::Failure::Unprepared { preparation: wire::Preparation::Transient }
        | wire::Failure::Cancelled { .. }
        | wire::Failure::Run { failure: wire::RunFailure::Exhausted } => engine::Failure::Transient,
        wire::Failure::Unprepared {
            preparation: wire::Preparation::Missing { .. } | wire::Preparation::Refused { .. },
        } => engine::Failure::Permanent,
        wire::Failure::Run {
            failure:
                wire::RunFailure::Model
                | wire::RunFailure::Budget
                | wire::RunFailure::Policy
                | wire::RunFailure::Cancelled
                | wire::RunFailure::Stale,
        } => engine::Failure::Run,
        wire::Failure::Agent { .. } => engine::Failure::Agent,
    }
}
fn answer(value: wire::LinkAnswer, repositories: u32, sizes: &Sizes) -> Result<engine::Answer, Error> {
    Ok(match value {
        wire::LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy } => engine::Answer::Busy,
        wire::LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Invalid { .. } } => engine::Answer::Invalid,
        wire::LinkAnswer::Ended { outcome, work: done } => {
            let work = work(done, repositories)?;
            match payload::decode_outcome(&outcome, sizes) {
                Some(outcome) => engine::Answer::Ended { outcome, work },
                None => engine::Answer::Failed { failure: engine::Failure::Agent, work },
            }
        }
        wire::LinkAnswer::Parked { snapshot, work: done } => {
            engine::Answer::Parked { snapshot, work: work(done, repositories)? }
        }
        wire::LinkAnswer::Failed { failure: failed, detail: _, work: done } => {
            engine::Answer::Failed { failure: failure(failed), work: work(done, repositories)? }
        }
    })
}

fn phase(value: wire::HostingPhase) -> engine::fleet::Phase {
    match value {
        wire::HostingPhase::Preparing => engine::fleet::Phase::Preparing,
        wire::HostingPhase::Starting => engine::fleet::Phase::Starting,
        wire::HostingPhase::Active => engine::fleet::Phase::Active,
        wire::HostingPhase::Waiting => engine::fleet::Phase::Waiting,
        wire::HostingPhase::Ending => engine::fleet::Phase::Ending,
        wire::HostingPhase::Answered => engine::fleet::Phase::Answered,
    }
}
fn named(run: u64, attempt: u64, repositories: u32) -> Result<(engine::Item, u64), Error> {
    let pair = names::pair(Token::new(run), Token::new(attempt)).ok_or(Error::Name)?;
    if pair.0.repository >= repositories {
        return Err(Error::Repository);
    }
    Ok(pair)
}

/// Opening and pings belong to the connection, so return no domain event.
pub fn up(channel: Token, message: wire::Message, repositories: u32, sizes: &Sizes) -> Result<Option<Event>, Error> {
    if repositories > 256 {
        return Err(Error::Repository);
    }
    temper_channel::codec::frame_len(&message, sizes).ok_or(Error::Payload)?;
    let event = match message {
        wire::Message::Hello { slots, workstreams, hosting } => {
            let mut hosted = List::with_capacity(u32::try_from(hosting.len()).ok().ok_or(Error::Payload)?);
            for record in hosting {
                let (item, attempt) = named(record.run, record.attempt, repositories)?;
                hosted
                    .push(engine::Hosted { item, attempt, phase: phase(record.phase) })
                    .expect("source hosting count");
            }
            Event::Hello { channel, hello: engine::Hello { slots, workstreams, hosting: hosted.into_boxed() } }
        }
        wire::Message::Answer { run, attempt, answer: terminal } => {
            let (item, attempt) = named(run, attempt, repositories)?;
            Event::Answer { channel, item, attempt, answer: answer(terminal, repositories, sizes)? }
        }
        wire::Message::Relay { run, attempt, call, body } => {
            let (item, attempt) = named(run, attempt, repositories)?;
            let Some(body) = payload::decode_call(&body, sizes) else {
                return Err(Error::InvalidRelay {
                    run,
                    attempt: names::attempt(item, attempt).expect("the checked pair fits").raw(),
                    call,
                });
            };
            Event::Relay { channel, item, attempt, call: Token::new(call), body }
        }
        wire::Message::Bounced { run, attempt, event, bounce } => {
            let (item, attempt) = named(run, attempt, repositories)?;
            let bounce = match bounce {
                wire::Bounce::TooLarge => engine::fleet::Bounce::TooLarge,
                wire::Bounce::Full => engine::fleet::Bounce::Full,
                wire::Bounce::Ending => engine::fleet::Bounce::Ending,
            };
            Event::Bounced { channel, item, attempt, name: Token::new(event), bounce }
        }
        wire::Message::Told { run, attempt, fact } => {
            let (item, attempt) = named(run, attempt, repositories)?;
            let Some((kind, content)) = payload::decode_fact(&fact, sizes) else {
                return Ok(None);
            };
            Event::Told { channel, item, attempt, kind, content }
        }
        wire::Message::Rejected { run, attempt, account, generation } => {
            let (item, attempt) = named(run, attempt, repositories)?;
            Event::Rejected { channel, item, attempt, account, generation }
        }
        wire::Message::Exhausted { run, attempt, account, retry_after } => {
            let (item, attempt) = named(run, attempt, repositories)?;
            Event::Exhausted { channel, item, attempt, account, retry_after }
        }
        wire::Message::Unsupported { .. }
        | wire::Message::HelloV2 { .. }
        | wire::Message::AnswerV2 { .. }
        | wire::Message::AssignV2 { .. }
        | wire::Message::AgentCallV2 { .. }
        | wire::Message::FinishV2 { .. }
        | wire::Message::AgentStartV2 { .. }
        | wire::Message::Turn { .. }
        | wire::Message::AcknowledgeTurn { .. }
        | wire::Message::TurnBusy { .. }
        | wire::Message::AgentTurn { .. }
        | wire::Message::Open { .. }
        | wire::Message::Accept { .. }
        | wire::Message::Refuse { .. }
        | wire::Message::Terms { .. }
        | wire::Message::Ping => return Ok(None),
        wire::Message::Assign { .. }
        | wire::Message::Inbound { .. }
        | wire::Message::Cancel { .. }
        | wire::Message::Relayed { .. }
        | wire::Message::Acknowledge { .. }
        | wire::Message::Grant { .. }
        | wire::Message::AgentCall { .. }
        | wire::Message::Withdraw { .. }
        | wire::Message::Fact { .. }
        | wire::Message::Long { .. }
        | wire::Message::LongDone
        | wire::Message::Waiting { .. }
        | wire::Message::Finish { .. }
        | wire::Message::AgentRejected { .. }
        | wire::Message::AgentExhausted { .. }
        | wire::Message::AgentStart { .. }
        | wire::Message::AgentEvent { .. }
        | wire::Message::AgentAnswer { .. }
        | wire::Message::AgentCancel
        | wire::Message::AgentGrant { .. } => return Err(Error::Direction),
    };
    Ok(Some(event))
}

/// Malformed relay payloads are refused under their original names without
/// asking the domain. Other translation errors carry no direct reply.
#[must_use]
pub fn reply(error: Error, sizes: &Sizes) -> Option<wire::Message> {
    match error {
        Error::InvalidRelay { run, attempt, call } => Some(wire::Message::Relayed {
            run,
            attempt,
            call,
            answer: payload::encode_served(engine::Served::Unserved(engine::Unserved::Invalid), sizes)?,
        }),
        Error::Name | Error::Repository | Error::Credential | Error::Payload | Error::Direction => None,
    }
}

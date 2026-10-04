//! The worker reads only channel structure, never an agent's payload bytes.
use super::{Error, records};
use crate::credentials::Table;
use skein_lib::{List, Time, Token};
use temper_channel::{Sizes, codec, wire};
use temper_worker_domain::agent::channel;

fn llm(account: u32, endpoints: &[wire::EndpointDescriptor]) -> bool {
    for endpoint in endpoints {
        if endpoint.account == account {
            return true;
        }
    }
    false
}
pub fn down(
    value: channel::Down,
    endpoints: &[wire::EndpointDescriptor],
    credentials: &Table,
    now: Time,
    sizes: &Sizes,
) -> Result<Option<wire::Message>, Error> {
    if credentials.token_bytes() > sizes.token_bytes
        || u32::try_from(endpoints.len()).ok().ok_or(Error::Limits)? > sizes.endpoints
    {
        return Err(Error::Limits);
    }
    for endpoint in endpoints {
        for bytes in [&endpoint.host, &endpoint.path, &endpoint.effort] {
            if u32::try_from(bytes.len()).ok().ok_or(Error::Limits)? > sizes.name_bytes {
                return Err(Error::Limits);
            }
        }
    }
    let message = match value {
        channel::Down::Start { charter, snapshot, repositories, grants } => {
            let snapshot_bytes = match &snapshot {
                Some(bytes) => bytes.len(),
                None => 0,
            };
            if u32::try_from(charter.len()).ok().ok_or(Error::Limits)? > sizes.charter
                || u32::try_from(snapshot_bytes).ok().ok_or(Error::Limits)? > sizes.snapshot
                || u32::try_from(repositories.len()).ok().ok_or(Error::Limits)? > sizes.repositories
                || u32::try_from(grants.len()).ok().ok_or(Error::Limits)? > sizes.grants
                || u32::try_from(endpoints.len()).ok().ok_or(Error::Limits)? > sizes.endpoints
            {
                return Err(Error::Limits);
            }
            let mut roots = List::with_capacity(u32::try_from(repositories.len()).ok().ok_or(Error::Limits)?);
            for record in repositories {
                roots
                    .push(wire::AgentRepository { name: record.name, writable: record.writable })
                    .expect("source root count");
            }
            let mut values = List::with_capacity(u32::try_from(grants.len()).ok().ok_or(Error::Limits)?);
            for name in grants {
                if llm(name.account, endpoints)
                    && let Some(value) = credentials.grant(name.account, name.generation, now)
                {
                    values.push(value).expect("source grant count");
                }
            }
            wire::Message::AgentStart {
                charter,
                snapshot,
                repositories: roots.into_boxed(),
                endpoints: endpoints.into(),
                grants: values.into_boxed(),
            }
        }
        channel::Down::Event { name, event } => wire::Message::AgentEvent { event: name.raw(), body: event },
        channel::Down::Answer { call, reply } => {
            wire::Message::AgentAnswer { call: call.raw(), reply: records::reply_to(reply) }
        }
        channel::Down::Cancel => wire::Message::AgentCancel,
        channel::Down::Grant { grant } => {
            if !llm(grant.account, endpoints) {
                return Ok(None);
            }
            let Some(grant) = credentials.grant(grant.account, grant.generation, now) else {
                return Ok(None);
            };
            wire::Message::AgentGrant { grant }
        }
    };
    codec::frame_len(&message, sizes).ok_or(Error::Limits)?;
    Ok(Some(message))
}
pub fn up(message: wire::Message, sizes: &Sizes) -> Result<channel::Up, Error> {
    codec::frame_len(&message, sizes).ok_or(Error::Limits)?;
    Ok(match message {
        wire::Message::AgentCall { call, ask } => channel::Up::Call {
            call: Token::new(call),
            ask: match ask {
                wire::Ask::Push { message } => channel::Ask::Push { message },
                wire::Ask::Relay { body } => channel::Ask::Relay { body },
            },
        },
        wire::Message::Withdraw { call } => channel::Up::Withdraw { call: Token::new(call) },
        wire::Message::Fact { fact } => channel::Up::Fact { fact },
        wire::Message::Long { span } => channel::Up::Long { span },
        wire::Message::LongDone => channel::Up::LongDone,
        wire::Message::Waiting { heard } => channel::Up::Waiting { heard },
        wire::Message::Finish { finish } => channel::Up::Finish { finish: records::finish_from(finish) },
        wire::Message::AgentRejected { account, generation } => channel::Up::Rejected { account, generation },
        wire::Message::AgentExhausted { account, retry_after } => channel::Up::Exhausted { account, retry_after },
        wire::Message::Open { .. }
        | wire::Message::Accept { .. }
        | wire::Message::Refuse { .. }
        | wire::Message::Ping
        | wire::Message::Terms { .. }
        | wire::Message::Hello { .. }
        | wire::Message::Answer { .. }
        | wire::Message::Relay { .. }
        | wire::Message::Bounced { .. }
        | wire::Message::Told { .. }
        | wire::Message::Rejected { .. }
        | wire::Message::Exhausted { .. }
        | wire::Message::Assign { .. }
        | wire::Message::Inbound { .. }
        | wire::Message::Cancel { .. }
        | wire::Message::Relayed { .. }
        | wire::Message::Acknowledge { .. }
        | wire::Message::Grant { .. }
        | wire::Message::AgentStart { .. }
        | wire::Message::AgentEvent { .. }
        | wire::Message::AgentAnswer { .. }
        | wire::Message::AgentCancel
        | wire::Message::AgentGrant { .. } => return Err(Error::Direction),
    })
}

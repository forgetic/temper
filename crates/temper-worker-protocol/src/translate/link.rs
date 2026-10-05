//! Worker link conversions. Values move separately; opaque bytes pass through.
use super::{Error, records};
use alloc::boxed::Box;
use skein_lib::{List, Token};
use temper_channel::{Sizes, codec, wire};
use temper_worker_domain::{self as worker, host};

#[expect(missing_debug_implementations, reason = "decoded grant values must never occur in traces")]
pub struct Incoming {
    pub event: Option<worker::Event>,
    pub grants: Box<[wire::Grant]>,
}
pub fn up(message: wire::Message, sizes: &Sizes) -> Result<Incoming, Error> {
    codec::frame_len(&message, sizes).ok_or(Error::Limits)?;
    let mut values: Box<[wire::Grant]> = Box::new([]);
    let event = match message {
        wire::Message::Assign { run, attempt, workspace, save, charter, snapshot, grants } => {
            let mut repositories =
                List::with_capacity(u32::try_from(workspace.repositories.len()).ok().ok_or(Error::Limits)?);
            for record in workspace.repositories {
                repositories
                    .push(host::Repository {
                        tag: record.tag,
                        name: record.name,
                        remote: record.remote,
                        start: records::start_from(record.start),
                        access: records::access_from(record.access),
                        identity: record.identity,
                    })
                    .expect("source repository count");
            }
            let mut names = List::with_capacity(u32::try_from(grants.len()).ok().ok_or(Error::Limits)?);
            for grant in &grants {
                names
                    .push(host::Grant { account: grant.account, generation: grant.generation, valid: grant.valid })
                    .expect("source grant count");
            }
            values = grants;
            worker::Event::Assign {
                assignment: host::Assignment {
                    run: Token::new(run),
                    attempt: Token::new(attempt),
                    workspace: host::Workspace { key: workspace.key, repositories: repositories.into_boxed() },
                    save,
                    charter,
                    snapshot,
                    grants: names.into_boxed(),
                },
            }
        }
        wire::Message::Inbound { run, attempt, event, body } => worker::Event::Inbound {
            run: Token::new(run),
            attempt: Token::new(attempt),
            name: Token::new(event),
            event: body,
        },
        wire::Message::Cancel { run, attempt } => {
            worker::Event::Cancel { run: Token::new(run), attempt: Token::new(attempt) }
        }
        wire::Message::Relayed { run, attempt, call, answer } => worker::Event::Relayed {
            run: Token::new(run),
            attempt: Token::new(attempt),
            call: Token::new(call),
            answer,
        },
        wire::Message::Acknowledge { run, attempt } => {
            worker::Event::Acknowledged { run: Token::new(run), attempt: Token::new(attempt) }
        }
        wire::Message::Grant { run, attempt, grant } => {
            let name = host::Grant { account: grant.account, generation: grant.generation, valid: grant.valid };
            values = Box::new([grant]);
            worker::Event::Grant { run: Token::new(run), attempt: Token::new(attempt), grant: name }
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
        | wire::Message::Ping => return Ok(Incoming { event: None, grants: values }),
        wire::Message::Hello { .. }
        | wire::Message::Answer { .. }
        | wire::Message::Relay { .. }
        | wire::Message::Bounced { .. }
        | wire::Message::Told { .. }
        | wire::Message::Rejected { .. }
        | wire::Message::Exhausted { .. }
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
    Ok(Incoming { event: Some(event), grants: values })
}
pub fn down(request: worker::Request, sizes: &Sizes) -> Result<Option<wire::Message>, Error> {
    let message = match request {
        worker::Request::Hello { hello } => {
            if u32::try_from(hello.hosting.len()).ok().ok_or(Error::Limits)? > sizes.slots
                || u32::try_from(hello.workstreams.len()).ok().ok_or(Error::Limits)? > sizes.workstreams
            {
                return Err(Error::Limits);
            }
            let mut hosting = List::with_capacity(u32::try_from(hello.hosting.len()).ok().ok_or(Error::Limits)?);
            for record in hello.hosting {
                hosting
                    .push(wire::Hosting {
                        run: record.run.raw(),
                        attempt: record.attempt.raw(),
                        phase: records::phase_to(record.phase),
                    })
                    .expect("source hosted count");
            }
            wire::Message::Hello { slots: hello.slots, workstreams: hello.workstreams, hosting: hosting.into_boxed() }
        }
        worker::Request::Answer { run, attempt, answer } => {
            wire::Message::Answer { run: run.raw(), attempt: attempt.raw(), answer: records::answer_to(answer, sizes)? }
        }
        worker::Request::Relay { run, attempt, call, body } => {
            wire::Message::Relay { run: run.raw(), attempt: attempt.raw(), call: call.raw(), body }
        }
        worker::Request::Bounced { run, attempt, name, bounce } => wire::Message::Bounced {
            run: run.raw(),
            attempt: attempt.raw(),
            event: name.raw(),
            bounce: records::bounce_to(bounce),
        },
        worker::Request::Rejected { run, attempt, account, generation } => {
            wire::Message::Rejected { run: run.raw(), attempt: attempt.raw(), account, generation }
        }
        worker::Request::Exhausted { run, attempt, account, retry_after } => {
            wire::Message::Exhausted { run: run.raw(), attempt: attempt.raw(), account, retry_after }
        }
        worker::Request::Dial
        | worker::Request::CancelRelay { .. }
        | worker::Request::Spawn { .. }
        | worker::Request::Send { .. }
        | worker::Request::Read { .. }
        | worker::Request::Signal { .. }
        | worker::Request::Wait { .. }
        | worker::Request::Reap { .. }
        | worker::Request::Io { .. }
        | worker::Request::CancelIo { .. } => return Ok(None),
    };
    codec::frame_len(&message, sizes).ok_or(Error::Limits)?;
    Ok(Some(message))
}
pub fn told(value: worker::Told, sizes: &Sizes) -> Result<wire::Message, Error> {
    let message = wire::Message::Told { run: value.run.raw(), attempt: value.attempt.raw(), fact: value.fact };
    codec::frame_len(&message, sizes).ok_or(Error::Limits)?;
    Ok(message)
}

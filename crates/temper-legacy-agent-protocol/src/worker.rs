//! Total projection of the current agent domain onto channel v1. Unknown
//! long-lived run policy remains outside this adapter (agent-domain.md, 10).
use crate::{Error, payload, render::Text};
use alloc::boxed::Box;
use skein_lib::{Token, bytes};
use temper_channel::{Sizes, payload::v1 as document, wire};
use temper_legacy_agent_domain::{self as agent, run, session, tools};

#[derive(Debug)]
pub struct Projection {
    pub frame: wire::Finish,
    pub spent: run::Spend,
}
#[must_use]
pub const fn spent(answer: &run::Answer) -> Option<run::Spend> {
    match answer {
        run::Answer::Accepted { spent, .. } | run::Answer::Failed { spent, .. } => Some(*spent),
        run::Answer::Refused(run::Refusal::Busy | run::Refusal::Invalid(_)) => None,
    }
}
/// Channel v1 has no refusal or spend fields. Keep spent separately, and
/// refuse an unrepresentable refusal instead of silently inventing Policy.
pub fn finish(answer: run::Answer, sizes: &Sizes) -> Result<Projection, Error> {
    Ok(match answer {
        run::Answer::Accepted { outcome, spent } => {
            Projection { frame: wire::Finish::Ended { outcome: payload::outcome(&outcome, sizes)? }, spent }
        }
        run::Answer::Failed { failure, spent } => Projection {
            frame: wire::Finish::Failed {
                failure: match failure {
                    run::Failure::Model(run::Fault::Exhausted) => wire::RunFailure::Exhausted,
                    run::Failure::Model(
                        run::Fault::Provider
                        | run::Fault::ContextFull
                        | run::Fault::Refused
                        | run::Fault::Truncated
                        | run::Fault::Malformed,
                    ) => wire::RunFailure::Model,
                    run::Failure::Budget(_) => wire::RunFailure::Budget,
                    run::Failure::Policy(_) => wire::RunFailure::Policy,
                    run::Failure::Cancelled => wire::RunFailure::Cancelled,
                    run::Failure::Stale => wire::RunFailure::Stale,
                },
            },
            spent,
        },
        run::Answer::Refused(run::Refusal::Busy | run::Refusal::Invalid(_)) => return Err(Error::Unsupported),
    })
}
pub fn answer(owner: Token, reply: wire::Reply) -> Result<agent::Event, Error> {
    Ok(match reply {
        wire::Reply::Pushed { push } => agent::Event::Pushed {
            owner,
            push: match push {
                wire::Push::Done => run::Push::Done,
                wire::Push::Moved => run::Push::Moved,
                wire::Push::Nothing => run::Push::Nothing,
                wire::Push::Failed { failure } => run::Push::Failed {
                    failure: run::PushFailure {
                        repository: failure.repository,
                        reason: reason(failure.reason),
                        diagnostic: run::PushDiagnostic::new(&failure.output, failure.cut),
                    },
                },
            },
        },
        wire::Reply::Withdrawn => agent::Event::HostCancelled { owner },
        wire::Reply::Unavailable => unpushed(owner, run::PushReason::Unavailable),
        wire::Reply::Busy => unpushed(owner, run::PushReason::Busy),
        wire::Reply::TooLarge => unpushed(owner, run::PushReason::TooLarge),
        wire::Reply::Relayed { .. } => return Err(Error::Unsupported),
    })
}
fn unpushed(owner: Token, reason: run::PushReason) -> agent::Event {
    agent::Event::Pushed { owner, push: run::Push::Failed { failure: run::PushFailure::new(reason) } }
}
fn reason(value: wire::PushReason) -> run::PushReason {
    match value {
        wire::PushReason::MissingRepository => run::PushReason::MissingRepository,
        wire::PushReason::MissingBranch => run::PushReason::MissingBranch,
        wire::PushReason::MissingCommit => run::PushReason::MissingCommit,
        wire::PushReason::Refused => run::PushReason::Refused,
        wire::PushReason::Unreachable => run::PushReason::Unreachable,
        wire::PushReason::Broken => run::PushReason::Broken,
        wire::PushReason::TimedOut => run::PushReason::TimedOut,
        wire::PushReason::Cancelled => run::PushReason::Cancelled,
        wire::PushReason::Unavailable => run::PushReason::Unavailable,
        wire::PushReason::Busy => run::PushReason::Busy,
        wire::PushReason::TooLarge => run::PushReason::TooLarge,
        wire::PushReason::Nothing => run::PushReason::Nothing,
        wire::PushReason::Unknown => run::PushReason::Unknown,
    }
}
pub fn fact(value: agent::Fact, sizes: &Sizes) -> Result<wire::Message, Error> {
    let name = match value {
        agent::Fact::Run { fact } => match fact {
            run::facts::Fact::Admitted { .. } => b"run.admitted".as_slice(),
            run::facts::Fact::Prepared { .. } => b"run.prepared",
            run::facts::Fact::Opened { .. } => b"run.opened",
            run::facts::Fact::Ended { .. } => b"run.ended",
            run::facts::Fact::Called { .. } => b"run.called",
            run::facts::Fact::Returned { .. } => b"run.returned",
            run::facts::Fact::CheckStarted { .. } => b"run.check-started",
            run::facts::Fact::CheckFinished { .. } => return Ok(wire::Message::LongDone),
            run::facts::Fact::Pushed { .. } => b"run.pushed",
            run::facts::Fact::Answered { .. } => b"run.answered",
        },
        agent::Fact::Session { fact } => session_fact(fact),
    };
    let fact = document::Fact { kind: document::FactKind::Progress, content: bytes::copy_of(name) };
    Ok(wire::Message::Fact { fact: document::encode_fact(&fact, sizes).ok_or(Error::TooLarge)? })
}
fn session_fact(value: session::Fact) -> &'static [u8] {
    match value {
        session::Fact::Opened { .. } => b"session.opened",
        session::Fact::CompletionStarted { .. } => b"session.completion-started",
        session::Fact::CompletionAnswered { .. } => b"session.completion-answered",
        session::Fact::CompletionFailed { .. } => b"session.completion-failed",
        session::Fact::CompletionCancelled { .. } => b"session.completion-cancelled",
        session::Fact::CompletionRetried { .. } => b"session.completion-retried",
        session::Fact::Tools { fact, .. } => match fact {
            tools::Fact::Opened { .. } => b"tools.opened",
            tools::Fact::Refused { .. } => b"tools.refused",
            tools::Fact::Started { .. } => b"tools.started",
            tools::Fact::Answered { .. } => b"tools.answered",
            tools::Fact::Closing { .. } => b"tools.closing",
            tools::Fact::Closed { .. } => b"tools.closed",
        },
        session::Fact::DelegateStarted { .. } => b"session.delegate-started",
        session::Fact::DelegateAnswered { .. } => b"session.delegate-answered",
        session::Fact::DelegateCancelled { .. } => b"session.delegate-cancelled",
        session::Fact::Yielded { .. } => b"session.yielded",
        session::Fact::Used { .. } => b"session.used",
        session::Fact::Ended { .. } => b"session.ended",
    }
}

/// Content facts remain best effort. Their measured human-readable content is
/// valid UTF-8 even when a file or command supplied arbitrary bytes.
pub fn content(value: &agent::Content, sizes: &Sizes) -> Result<wire::Message, Error> {
    let mut measure = Text::measure();
    let kind = content_text(value, &mut measure);
    let mut write = Text::write(measure.length(sizes.fact)?);
    content_text(value, &mut write);
    let fact = document::Fact { kind, content: write.finish() };
    Ok(wire::Message::Fact { fact: document::encode_fact(&fact, sizes).ok_or(Error::TooLarge)? })
}
fn content_text(value: &agent::Content, out: &mut Text) -> document::FactKind {
    let owner = match value {
        agent::Content::Text { owner, .. }
        | agent::Content::Call { owner, .. }
        | agent::Content::Tool { owner, .. }
        | agent::Content::Usage { owner, .. } => *owner,
    };
    out.put(b"conversation ");
    out.number(owner.raw());
    out.put(b"\n");
    match value {
        agent::Content::Text { owner: _, text } => {
            out.bytes(text);
            document::FactKind::Text
        }
        agent::Content::Call { owner: _, id, name, input } => {
            out.bytes(name);
            out.put(b" ");
            out.bytes(id);
            out.put(b"\n");
            out.bytes(input);
            document::FactKind::Call
        }
        agent::Content::Tool { owner: _, done } => {
            done_text(done, out);
            document::FactKind::Tool
        }
        agent::Content::Usage { owner: _, usage } => {
            out.put(b"input ");
            out.number(usage.input_tokens);
            out.put(b"; output ");
            out.number(usage.output_tokens);
            out.put(b"; cache read ");
            out.number(usage.cache_read_tokens);
            out.put(b"; cache write ");
            out.number(usage.cache_write_tokens);
            document::FactKind::Usage
        }
    }
}
fn done_text(value: &tools::Done, out: &mut Text) {
    match value {
        tools::Done::Loaded { content, version: _ } => out.bytes(content),
        tools::Done::Scanned { entries, more } => {
            for entry in entries {
                out.bytes(entry.name.as_bytes());
                match entry.kind {
                    tools::Kind::Directory => out.put(b"/"),
                    tools::Kind::File | tools::Kind::Link | tools::Kind::Other => {}
                }
                out.put(b"\n");
            }
            out.put(b"more ");
            out.number(*more);
        }
        tools::Done::Stored { version: _ } => out.put(b"stored"),
        tools::Done::Conflict { now: _ } => out.put(b"write conflict"),
        tools::Done::Exited { exit, head, tail, dropped } => {
            out.put(b"exit ");
            match exit {
                tools::Exit::Code { code } => out.number(u64::from(*code)),
                tools::Exit::Signal { signal } => {
                    out.put(b"signal ");
                    out.number(u64::from(*signal));
                }
                tools::Exit::TimedOut => out.put(b"timed out"),
            }
            out.put(b"\n");
            out.bytes(head);
            out.put(b"\n[");
            out.number(*dropped);
            out.put(b" bytes omitted]\n");
            out.bytes(tail);
        }
        tools::Done::Found { hits, more, timed_out } => {
            for hit in hits {
                out.bytes(&hit.path);
                out.put(b":");
                out.number(u64::from(hit.line));
                out.put(b": ");
                out.bytes(&hit.text);
                out.put(b"\n");
            }
            out.put(b"more ");
            out.number(*more);
            if *timed_out {
                out.put(b"; timed out");
            }
        }
        tools::Done::Missing => out.put(b"missing"),
        tools::Done::NotFile => out.put(b"not a file"),
        tools::Done::Linked => out.put(b"symbolic link"),
        tools::Done::NotDirectory => out.put(b"not a directory"),
        tools::Done::TooLarge { size } => {
            out.put(b"too large: ");
            out.number(*size);
        }
        tools::Done::Escapes => out.put(b"outside root"),
        tools::Done::Failed { fault } => out.put(match fault {
            tools::Fault::Denied => b"permission denied",
            tools::Fault::NoSpace => b"no space",
            tools::Fault::Other => b"io failed",
        }),
        tools::Done::TimedOut => out.put(b"timed out"),
        tools::Done::Cancelled => out.put(b"cancelled"),
    }
}

/// Bytes of future snapshot policy are deliberately not parsed by this
/// projection. The current run domain has no resume operation.
pub fn snapshot(value: &Option<Box<[u8]>>) -> Result<(), Error> {
    match value {
        Some(_) => Err(Error::Unsupported),
        None => Ok(()),
    }
}

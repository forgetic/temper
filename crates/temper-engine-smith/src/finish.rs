//! Concrete Smith turn and terminal metadata become the root's opaque turn
//! and typed task ending (domain/engine.md, 7.2 and 7.4; domain/tasks.md, 5.6).
//! The boundary caller supplies encoded turn bytes and the worker's settled
//! pushed resource evidence; this crate does not encode history or inspect git.

use alloc::boxed::Box;
use smith_domain as smith;
use smith_domain_run as run;
use temper_engine_domain::engine;
use temper_engine_domain_tasks as tasks;

use crate::Problem;

/// Connector identity established by the worker's push and root's forge route.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChangeResource {
    pub connector: u16,
    pub kind: u16,
    pub resource: u64,
}

/// One root terminal body plus the Smith activation's cumulative charge.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Terminal {
    pub cumulative: u64,
    pub turns: u32,
    pub end: tasks::End,
}

/// Translate a Smith turn envelope, keeping its concrete record opaque to
/// Temper. The protocol adapter supplies the bounded encoded bytes.
#[expect(clippy::manual_map, reason = "the step subset forbids closure-based Option::map")]
pub fn turn(request: smith::Request, transcript: Box<[u8]>) -> Result<engine::Turn, Problem> {
    match request {
        smith::Request::Turn { number, read, spent, .. } => Ok(engine::Turn {
            number,
            cumulative: spent.units,
            read: match read {
                Some(name) => Some(name.raw()),
                None => None,
            },
            transcript,
        }),
        smith::Request::Waiting { .. }
        | smith::Request::HostCall { .. }
        | smith::Request::WithdrawHost { .. }
        | smith::Request::Admitted { .. }
        | smith::Request::Answer { .. }
        | smith::Request::Checking { .. }
        | smith::Request::Deliver { .. }
        | smith::Request::Complete { .. }
        | smith::Request::Rejected { .. }
        | smith::Request::Exhausted { .. }
        | smith::Request::Cancel { .. }
        | smith::Request::Io { .. }
        | smith::Request::CancelIo { .. }
        | smith::Request::Read { .. }
        | smith::Request::Probe { .. }
        | smith::Request::Check { .. }
        | smith::Request::Abort { .. } => Err(Problem::Type),
    }
}

/// Translate Smith's terminal after its turns. A successful change also
/// needs the resource the worker and forge route settled, not a guess from
/// the LLM's fields.
pub fn result(answer: run::Answer, change: Option<ChangeResource>) -> Result<Terminal, Problem> {
    match answer {
        run::Answer::Parked { spent, turns } => {
            Ok(Terminal { cumulative: spent.units, turns, end: tasks::End::Parked })
        }
        run::Answer::Refused(refusal) => match refusal {
            run::Refusal::Busy => Ok(Terminal { cumulative: 0, turns: 0, end: tasks::End::Refused }),
            run::Refusal::Invalid(_) => {
                Ok(Terminal { cumulative: 0, turns: 0, end: tasks::End::Failed(tasks::Class::Permanent) })
            }
        },
        run::Answer::Failed { failure, spent, turns } => {
            let class = match failure {
                run::Failure::Transcript(_) | run::Failure::Stale => tasks::Class::Transient,
                run::Failure::Model(_) | run::Failure::Budget(_) | run::Failure::Policy(_) => tasks::Class::Run,
                run::Failure::Cancelled => tasks::Class::Agent,
            };
            Ok(Terminal { cumulative: spent.units, turns, end: tasks::End::Failed(class) })
        }
        run::Answer::Accepted { outcome, spent, turns } => {
            let result = match outcome {
                run::outcome::Declared::Report(report) => tasks::TaskResult::Report { words: report.text },
                run::outcome::Declared::Failure(failure) => tasks::TaskResult::Failure { reason: failure.reason },
                run::outcome::Declared::Verdict(verdict) => {
                    if !verdict.items.is_empty() {
                        return Err(Problem::Type);
                    }
                    let Ok(code) = u32::try_from(crate::decimal(&verdict.name)?) else { return Err(Problem::Range) };
                    tasks::TaskResult::Verdict { code, words: verdict.text }
                }
                run::outcome::Declared::Change(change_result) => {
                    let resource = change.ok_or(Problem::Missing)?;
                    let mut words = None;
                    for field in change_result.fields {
                        match field.name.as_ref() {
                            b"body" => words = Some(field.value),
                            b"title" => {}
                            _ => return Err(Problem::Type),
                        }
                    }
                    tasks::TaskResult::Change {
                        connector: resource.connector,
                        kind: resource.kind,
                        resource: resource.resource,
                        words: words.ok_or(Problem::Missing)?,
                    }
                }
            };
            Ok(Terminal {
                cumulative: spent.units,
                turns,
                end: tasks::End::Finished { result, cancel_delegates: false },
            })
        }
    }
}

//! The workspace's commit fields and sealed Smith delivery terminals (jig's
//! domain/hosts.md, sections 7 and 9). The channel codec owns field encoding;
//! this translation keeps commit heads as receipts and operator tails as data.

use alloc::boxed::Box;
use jig_host as host;
use skein_lib::{List, Reader};
use smith_host_domain as smith;

use crate::{Domain, wire, workspace};

pub(crate) struct Fields {
    pub(crate) title: Box<[u8]>,
    pub(crate) body: Box<[u8]>,
}

pub(crate) fn fields(bytes: &[u8], limit: u32) -> Option<Fields> {
    let record = smith_channel::DeliverAsk::decode(&smith_channel::CEILINGS, &mut Reader::new(bytes)).ok()?;
    let mut title = None;
    let mut body = None;
    for field in record.into_parts().fields.into_boxed() {
        let field = field.into_parts();
        match field.name.as_ref() {
            b"title" if title.is_none() => title = Some(field.text),
            b"body" if body.is_none() => body = Some(field.text),
            _ => return None,
        }
    }
    let title = title?;
    let body = body?;
    let bytes = u32::try_from(title.len().checked_add(body.len())?).ok()?;
    if bytes > limit || title.is_empty() {
        return None;
    }
    Some(Fields { title, body })
}

pub(crate) fn reply(domain: &Domain, reply: host::Reply) -> smith::Reply {
    match reply {
        host::Reply::Relayed { answer } => smith::Reply::Host { error: false, body: answer },
        host::Reply::Delivered(delivery) => smith::Reply::Delivery(workspace::smith_delivery(domain, delivery.left)),
        host::Reply::Unavailable => smith::Reply::Unavailable,
        host::Reply::Withdrawn => smith::Reply::Withdrawn,
        host::Reply::Busy => smith::Reply::Busy,
    }
}

#[expect(clippy::manual_map, reason = "explicit option match without closures in step code")]
pub(crate) fn landings(landings: &[wire::Landing]) -> smith::Delivery {
    let mut receipts = List::with_capacity(u32::try_from(landings.len()).expect("bounded directories"));
    let mut first = None;
    let mut moved = false;
    for (directory, landing) in landings.iter().enumerate() {
        let directory = u32::try_from(directory).expect("bounded directories");
        let failure = match landing {
            wire::Landing::Landed { commit } => {
                let mut text = skein_lib::Writer::new(64);
                for byte in commit {
                    let high = *b"0123456789abcdef".get(usize::from(byte >> 4_u8)).expect("hex nibble");
                    let low = *b"0123456789abcdef".get(usize::from(byte & 15)).expect("hex nibble");
                    text.put(&[high, low]).expect("two characters per commit byte");
                }
                receipts
                    .push(smith::Receipt::new(directory, text.finish()).expect("bounded commit receipt"))
                    .expect("room for changed directories");
                None
            }
            wire::Landing::Moved => {
                moved = true;
                None
            }
            wire::Landing::Unchanged => None,
            wire::Landing::Conflicted { files } => {
                let marker = match files.first() {
                    Some(path) => Some(smith::Marker::new(directory, path.clone()).expect("validated conflict path")),
                    None => None,
                };
                Some(smith::Delivery::Refused(
                    smith::DeliveryRefusal::new(marker, Box::from(&b"resolve conflict markers before delivering"[..]))
                        .expect("bounded conflict feedback"),
                ))
            }
            wire::Landing::Refused => {
                Some(failed(directory, smith::DeliveryReason::RefusedByTarget, &smith::Diagnostic::empty()))
            }
            wire::Landing::Failed => {
                Some(failed(directory, smith::DeliveryReason::Unknown, &smith::Diagnostic::empty()))
            }
            wire::Landing::Explained { failure } => Some(failed(
                directory,
                reason(failure.reason),
                &smith::Diagnostic::new(failure.diagnostic.output(), failure.diagnostic.cut()),
            )),
        };
        if first.is_none() {
            first = failure;
        }
    }
    if moved {
        smith::Delivery::Stale
    } else if let Some(failed) = first {
        failed
    } else if receipts.is_empty() {
        smith::Delivery::Nothing
    } else {
        smith::Delivery::Delivered(
            smith::Delivered::new(receipts.into_boxed()).expect("one unique receipt per changed directory"),
        )
    }
}

fn failed(directory: u32, reason: smith::DeliveryReason, diagnostic: &smith::Diagnostic) -> smith::Delivery {
    smith::Delivery::Failed(smith::DeliveryFailure { directory, reason, diagnostic: *diagnostic })
}

fn reason(reason: wire::PushReason) -> smith::DeliveryReason {
    match reason {
        wire::PushReason::MissingRepository | wire::PushReason::MissingBranch | wire::PushReason::MissingCommit => {
            smith::DeliveryReason::Missing
        }
        wire::PushReason::Refused => smith::DeliveryReason::RefusedByTarget,
        wire::PushReason::Unreachable => smith::DeliveryReason::Unreachable,
        wire::PushReason::Broken => smith::DeliveryReason::Broken,
        wire::PushReason::TimedOut => smith::DeliveryReason::TimedOut,
        wire::PushReason::Cancelled => smith::DeliveryReason::Cancelled,
        wire::PushReason::Unavailable => smith::DeliveryReason::Unavailable,
        wire::PushReason::Busy => smith::DeliveryReason::Busy,
        wire::PushReason::TooLarge => smith::DeliveryReason::TooLarge,
        wire::PushReason::Nothing | wire::PushReason::Unknown => smith::DeliveryReason::Unknown,
    }
}

pub(crate) fn finish(result: smith::RunResult) -> host::Finish {
    match result {
        smith::RunResult::Accepted { outcome } => host::Finish::Ended { outcome },
        smith::RunResult::Parked => host::Finish::Parked,
        smith::RunResult::Failed { failure } => host::Finish::Failed { failure: run_failure(failure) },
        smith::RunResult::Refused { refusal } => host::Finish::Failed {
            failure: match refusal {
                smith::Refusal::Busy => host::RunFailure::Model,
                smith::Refusal::Invalid(_) => host::RunFailure::Policy,
            },
        },
    }
}

fn run_failure(failure: smith::RunFailure) -> host::RunFailure {
    match failure {
        smith::RunFailure::Model(smith::ModelFault::Exhausted) => host::RunFailure::Exhausted,
        smith::RunFailure::Transcript(_)
        | smith::RunFailure::Model(
            smith::ModelFault::Completion { .. }
            | smith::ModelFault::Provider
            | smith::ModelFault::ContextFull
            | smith::ModelFault::Refused
            | smith::ModelFault::Truncated
            | smith::ModelFault::Malformed,
        ) => host::RunFailure::Model,
        smith::RunFailure::Budget(_) => host::RunFailure::Budget,
        smith::RunFailure::Policy(_) => host::RunFailure::Policy,
        smith::RunFailure::Cancelled => host::RunFailure::Cancelled,
        smith::RunFailure::Stale => host::RunFailure::Stale,
    }
}

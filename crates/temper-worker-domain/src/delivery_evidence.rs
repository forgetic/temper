//! Lossless delivery evidence at the jig/Smith boundary (jig's
//! `domain/hosts.md`, sections 7 and 9). The versioned Smith channel codec
//! owns the bytes; jig stores and forwards them without interpreting them.

use alloc::boxed::Box;

use skein_lib::{List, Reader, Writer};
use smith_channel as wire;
use smith_host_domain as smith;

use jig_worker_host::{AnsweredCall, DeliveryOutcome, SettledAnswer};

/// Every resumed delivery has valid evidence for the outcome jig recorded.
pub(crate) fn valid_answered(calls: &[AnsweredCall], evidence_bytes: u64) -> bool {
    for call in calls {
        match &call.answer {
            SettledAnswer::Host { .. } => {}
            SettledAnswer::Delivery { outcome, evidence } => {
                let Ok(size) = u64::try_from(evidence.len()) else { return false };
                if size > evidence_bytes {
                    return false;
                }
                let Some(delivery) = decode(evidence) else { return false };
                let expected = match delivery {
                    smith::Delivery::Delivered(_) => DeliveryOutcome::Delivered,
                    smith::Delivery::Nothing => DeliveryOutcome::Nothing,
                    smith::Delivery::Stale => DeliveryOutcome::Stale,
                    smith::Delivery::Refused(_) => DeliveryOutcome::Refused,
                    smith::Delivery::Failed(_) => DeliveryOutcome::Failed,
                };
                if *outcome != expected {
                    return false;
                }
            }
        }
    }
    true
}

/// Encode a Smith delivery as bounded, versioned evidence for a settled call.
#[cfg_attr(not(test), expect(dead_code, reason = "the Smith agent route begins in the next increment"))]
pub(crate) fn encode(delivery: smith::Delivery) -> Option<Box<[u8]>> {
    let limits = &wire::CEILINGS;
    let value = match delivery {
        smith::Delivery::Delivered(delivered) => {
            let mut receipts = List::with_capacity(limits.delivered_receipts);
            for receipt in delivered.receipts() {
                let row = wire::Receipt::new(
                    limits,
                    wire::ReceiptParts { directory: receipt.directory(), text: Box::from(receipt.text()) },
                )
                .ok()?;
                receipts.push(row).ok()?;
            }
            wire::Delivery::Delivered(wire::Delivered::new(limits, wire::DeliveredParts { receipts }).ok()?)
        }
        smith::Delivery::Nothing => wire::Delivery::Nothing,
        smith::Delivery::Stale => wire::Delivery::Stale,
        smith::Delivery::Refused(refusal) => {
            let marker = match refusal.marker() {
                Some(marker) => Some(
                    wire::Marker::new(
                        limits,
                        wire::MarkerParts { directory: marker.directory(), path: Box::from(marker.path()) },
                    )
                    .ok()?,
                ),
                None => None,
            };
            wire::Delivery::Refused(
                wire::DeliveryRefusal::new(
                    limits,
                    wire::DeliveryRefusalParts { marker, explanation: Box::from(refusal.explanation()) },
                )
                .ok()?,
            )
        }
        smith::Delivery::Failed(failure) => wire::Delivery::Failed(
            wire::DeliveryFailure::new(
                limits,
                wire::DeliveryFailureParts {
                    directory: failure.directory,
                    reason: encode_reason(failure.reason),
                    diagnostic: Box::from(failure.diagnostic.output()),
                    dropped: failure.diagnostic.cut(),
                },
            )
            .ok()?,
        ),
    };
    let record = wire::DeliveryReply::new(limits, wire::DeliveryReplyParts { value }).ok()?;
    let mut writer = Writer::new(usize::try_from(record.measure()).ok()?);
    record.encode(&mut writer).ok()?;
    Some(writer.finish())
}

/// Decode stored evidence, checking both the channel schema and Smith's
/// sealed delivery invariants. Invalid evidence must refuse the assignment.
pub(crate) fn decode(evidence: &[u8]) -> Option<smith::Delivery> {
    let record = wire::DeliveryReply::decode(&wire::CEILINGS, &mut Reader::new(evidence)).ok()?;
    match record.into_parts().value {
        wire::Delivery::Delivered(delivered) => {
            let rows = delivered.into_parts().receipts;
            let mut receipts = List::with_capacity(rows.len());
            for row in rows.into_boxed() {
                let parts = row.into_parts();
                receipts.push(smith::Receipt::new(parts.directory, parts.text)?).ok()?;
            }
            Some(smith::Delivery::Delivered(smith::Delivered::new(receipts.into_boxed())?))
        }
        wire::Delivery::Nothing => Some(smith::Delivery::Nothing),
        wire::Delivery::Stale => Some(smith::Delivery::Stale),
        wire::Delivery::Refused(refusal) => {
            let parts = refusal.into_parts();
            let marker = match parts.marker {
                Some(marker) => {
                    let fields = marker.into_parts();
                    Some(smith::Marker::new(fields.directory, fields.path)?)
                }
                None => None,
            };
            Some(smith::Delivery::Refused(smith::DeliveryRefusal::new(marker, parts.explanation)?))
        }
        wire::Delivery::Failed(failure) => {
            let parts = failure.into_parts();
            Some(smith::Delivery::Failed(smith::DeliveryFailure {
                directory: parts.directory,
                reason: decode_reason(parts.reason),
                diagnostic: smith::Diagnostic::new(&parts.diagnostic, parts.dropped),
            }))
        }
    }
}

fn encode_reason(reason: smith::DeliveryReason) -> wire::DeliveryReason {
    match reason {
        smith::DeliveryReason::Unreachable => wire::DeliveryReason::Unreachable,
        smith::DeliveryReason::RefusedByTarget => wire::DeliveryReason::RefusedByTarget,
        smith::DeliveryReason::TimedOut => wire::DeliveryReason::TimedOut,
        smith::DeliveryReason::Broken => wire::DeliveryReason::Broken,
        smith::DeliveryReason::TooLarge => wire::DeliveryReason::TooLarge,
        smith::DeliveryReason::Missing => wire::DeliveryReason::Missing,
        smith::DeliveryReason::Busy => wire::DeliveryReason::Busy,
        smith::DeliveryReason::Unavailable => wire::DeliveryReason::Unavailable,
        smith::DeliveryReason::Cancelled => wire::DeliveryReason::Cancelled,
        smith::DeliveryReason::Unknown => wire::DeliveryReason::Unknown,
    }
}

fn decode_reason(reason: wire::DeliveryReason) -> smith::DeliveryReason {
    match reason {
        wire::DeliveryReason::Unreachable => smith::DeliveryReason::Unreachable,
        wire::DeliveryReason::RefusedByTarget => smith::DeliveryReason::RefusedByTarget,
        wire::DeliveryReason::TimedOut => smith::DeliveryReason::TimedOut,
        wire::DeliveryReason::Broken => smith::DeliveryReason::Broken,
        wire::DeliveryReason::TooLarge => smith::DeliveryReason::TooLarge,
        wire::DeliveryReason::Missing => smith::DeliveryReason::Missing,
        wire::DeliveryReason::Busy => smith::DeliveryReason::Busy,
        wire::DeliveryReason::Unavailable => smith::DeliveryReason::Unavailable,
        wire::DeliveryReason::Cancelled => smith::DeliveryReason::Cancelled,
        wire::DeliveryReason::Unknown => smith::DeliveryReason::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, valid_answered};
    use jig_worker_host::{AnsweredCall, DeliveryOutcome, SettledAnswer};
    use smith_host_domain::{Delivered, Delivery, DeliveryFailure, DeliveryReason, Diagnostic, Receipt};

    #[test]
    fn receipts_and_failed_diagnostics_survive_a_resumed_delivery() {
        let receipt = Receipt::new(2, Box::from(&b"head:abc123"[..])).expect("bounded receipt");
        let delivered = Delivery::Delivered(Delivered::new(Box::new([receipt])).expect("one receipt"));
        let bytes = encode(delivered.clone()).expect("codec accepts receipt");
        assert_eq!(decode(&bytes), Some(delivered));

        let failed = Delivery::Failed(DeliveryFailure {
            directory: 1,
            reason: DeliveryReason::TimedOut,
            diagnostic: Diagnostic::new(b"push timed out", 7),
        });
        let bytes = encode(failed.clone()).expect("codec accepts diagnostic");
        assert_eq!(decode(&bytes), Some(failed));
    }

    #[test]
    fn malformed_or_unsealed_evidence_is_invalid() {
        assert_eq!(decode(&[255, 0, 1]), None);
    }

    #[test]
    fn a_resumed_delivery_requires_bounded_evidence_matching_its_outcome() {
        let evidence = encode(Delivery::Nothing).expect("codec accepts nothing");
        let size = u64::try_from(evidence.len()).expect("encoded size fits");
        let answered = [AnsweredCall {
            name: Box::from(&b"call"[..]),
            tool: Box::from(&b"deliver"[..]),
            answer: SettledAnswer::Delivery { outcome: DeliveryOutcome::Nothing, evidence: evidence.clone() },
        }];
        assert!(valid_answered(&answered, size));
        assert!(!valid_answered(&answered, size.saturating_sub(1)));
        let wrong = [AnsweredCall {
            name: Box::from(&b"call"[..]),
            tool: Box::from(&b"deliver"[..]),
            answer: SettledAnswer::Delivery { outcome: DeliveryOutcome::Delivered, evidence },
        }];
        assert!(!valid_answered(&wrong, size));
    }
}

//! The root's ranges, expressed in jig's store vocabulary.
use super::route::Flight;
use alloc::boxed::Box;
use jig_domain_core as core;
use jig_protocol_store as store;
use temper_engine_domain::{CoreRange, Key, Range};

pub(super) fn cursor(range: &Range, key: &Key) -> Option<store::Cursor> {
    let after = match key {
        Key::Core(key) => store::keys::encode(key, u32::MAX)?,
        Key::Forge(key) => store::keys::application(&temper_protocol_store::keys::encode(key, u32::MAX)?, u32::MAX)?,
    };
    let interval = match (range, key) {
        (Range::Census(census), Key::Core(key)) => census.interval(key)?,
        // jig restores issued names first, then the counter, accounts and sources.
        (
            Range::Core(CoreRange::Accounts),
            Key::Core(core::Key::Accounts(
                jig_domain_accounts::Key::Next
                | jig_domain_accounts::Key::Account { .. }
                | jig_domain_accounts::Key::Source { .. },
            )),
        )
        | (Range::Core(CoreRange::Tasks), Key::Core(core::Key::Tasks(_))) => 1,
        (Range::Core(_) | Range::Census(_) | Range::Forge, Key::Core(_) | Key::Forge(_)) => 0,
    };
    Some(store::Cursor { interval, after })
}

pub(super) fn request(owner: skein_lib::Token, flight: &Flight) -> Option<store::Request> {
    let after = if let Some(cursor) = &flight.cursor {
        Some(cursor.clone())
    } else {
        match &flight.after {
            Some(key) => Some(cursor(&flight.range, key)?),
            None => None,
        }
    };
    let rows = flight.most;
    let bytes = u64::from(flight.bytes);
    let range = match &flight.range {
        Range::Forge => store::Restore::Application(Box::new([temper_protocol_store::interval()])),
        Range::Census(census) => store::Restore::Jig(Box::new([core::RestoreRange::Census(census.clone())])),
        Range::Core(range) => match range {
            CoreRange::Deployment => store::Restore::Jig(Box::new([core::RestoreRange::Deployment])),
            CoreRange::Settings => store::Restore::Jig(Box::new([core::RestoreRange::Settings])),
            CoreRange::Accounts => store::Restore::Jig(Box::new([core::RestoreRange::Accounts])),
            CoreRange::People => store::Restore::Jig(Box::new([core::RestoreRange::People])),
            CoreRange::Tasks => store::Restore::Jig(Box::new([core::RestoreRange::Core, core::RestoreRange::Tasks])),
            CoreRange::Calls | CoreRange::RunProofs => store::Restore::Jig(Box::new([core::RestoreRange::Core])),
            CoreRange::EndedRead { task, party } => {
                return Some(store::Request::Get {
                    owner,
                    key: store::Get::Jig(core::Key::People(jig_domain_people::Key::EndedRead {
                        task: *task,
                        party: *party,
                    })),
                });
            }
            CoreRange::TaskResult { task } => {
                return Some(store::Request::Get {
                    owner,
                    key: store::Get::Jig(core::Key::Tasks(jig_domain_tasks::Key::Ended(*task))),
                });
            }
            CoreRange::TaskProposal { task, .. } => {
                return Some(store::Request::Get {
                    owner,
                    key: store::Get::Jig(core::Key::Tasks(jig_domain_tasks::Key::Live(*task))),
                });
            }
            CoreRange::ProposalDecision { proposer, proposal } => {
                return Some(store::Request::Get {
                    owner,
                    key: store::Get::Jig(core::Key::Core(core::CoreKey::ProposalDecision {
                        proposer: *proposer,
                        proposal: *proposal,
                    })),
                });
            }
            CoreRange::EscalationDecision { task, revision } => {
                return Some(store::Request::Get {
                    owner,
                    key: store::Get::Jig(core::Key::Core(core::CoreKey::EscalationDecision {
                        task: *task,
                        revision: *revision,
                    })),
                });
            }
            CoreRange::Notes(range) => {
                return Some(store::Request::Load {
                    owner,
                    range: store::Load::Notes(range.clone()),
                    after,
                    rows,
                    bytes,
                });
            }
            CoreRange::Turns { task, .. } => {
                return Some(store::Request::Transcript {
                    owner,
                    read: store::TranscriptRead::Page { task: *task, backwards: false, after: flight.offset },
                    rows,
                    bytes,
                });
            }
            CoreRange::TaskTranscript { task, from: (attempt, turn) } => {
                return Some(store::Request::Transcript {
                    owner,
                    read: store::TranscriptRead::Window {
                        task: *task,
                        attempt: *attempt,
                        turn: *turn,
                        after: flight.offset,
                    },
                    rows,
                    bytes,
                });
            }
        },
    };
    Some(store::Request::Restore { owner, range, after, rows, bytes })
}

/// Window rows carry agent-readable text across the protocol/domain boundary.
/// Each rendering fits one tail, as jig's protocol/store.md, 8.3 prices it:
/// a longer turn keeps its text's end, as jig's core keeps it.
pub(super) fn texts(
    range: &Range,
    rows: &[temper_engine_domain::Record],
) -> Option<temper_engine_domain::TranscriptTexts> {
    if let Range::Core(CoreRange::TaskTranscript { .. }) = range {
    } else {
        return None;
    }
    let mut texts = skein_lib::List::with_capacity(u32::try_from(rows.len()).expect("bounded transcript rows"));
    for row in rows {
        let text = match row {
            temper_engine_domain::Record::Core(core::Record::Core(core::CoreRecord::Turn(turn))) => {
                let bound = u32::try_from(turn.transcript.len()).expect("bounded stored turn");
                let limits = jig_protocol_transcript::Limits::new(
                    bound,
                    jig_protocol_transcript::TurnLimits { messages: u32::MAX, blocks: u32::MAX, text: bound },
                );
                match jig_protocol_transcript::brief_text(&limits, &turn.transcript) {
                    Ok(text) => Some(text_end(&text)),
                    Err(_) => None,
                }
            }
            temper_engine_domain::Record::Core(_) | temper_engine_domain::Record::Forge { .. } => None,
        };
        texts.push(text).expect("one rendering per row");
    }
    Some(texts.into_boxed())
}

/// A rendering within one tail, its end kept after jig's cut marker on a UTF-8
/// boundary; the protocol rendered UTF-8.
fn text_end(text: &[u8]) -> Box<[u8]> {
    const MARKER: &[u8] = b"[earlier text cut]\n";
    let bound = usize::try_from(temper_engine_domain::TRANSCRIPT_TAIL_BYTES).expect("tail bound fits usize");
    if text.len() <= bound {
        return Box::from(text);
    }
    let mut start =
        text.len().checked_sub(bound.checked_sub(MARKER.len()).expect("marker fits a tail")).expect("oversized text");
    // A UTF-8 character has at most three continuation bytes.
    for _ in 0_u8..3 {
        match text.get(start) {
            Some(byte) if byte & 0xc0 == 0x80 => start = start.checked_add(1).expect("inside text"),
            Some(_) | None => break,
        }
    }
    let suffix = text.get(start..).expect("suffix within text");
    let mut writer = skein_lib::Writer::new(MARKER.len().checked_add(suffix.len()).expect("bounded text"));
    writer.put(MARKER).expect("measured marker");
    writer.put(suffix).expect("measured suffix");
    writer.finish()
}

//! Wording for W2's frame, chats and notices, using sized bytes.

use alloc::boxed::Box;
use skein_lib::{Decimal, Writer};
use temper_web_domain::{LinkState, NoticeKind, Refusal};

#[must_use]
pub fn link(state: LinkState) -> &'static [u8] {
    match state {
        LinkState::Starting => b"Connecting",
        LinkState::Live => b"Live",
        LinkState::Behind => b"Catching up",
        LinkState::Offline { .. } => b"Offline",
    }
}

#[must_use]
pub fn notice(kind: NoticeKind) -> &'static [u8] {
    match kind {
        NoticeKind::RequestFull => b"Too many requests in flight",
        NoticeKind::ReadFull => b"Too many reads in flight",
        NoticeKind::WordsTooLong => b"Your words are too long",
        NoticeKind::Refused(refusal) => match refusal {
            Refusal::Role => b"Your role cannot do that",
            Refusal::Limit => b"The limit was reached",
            Refusal::Ended => b"This task has ended",
            Refusal::Unknown => b"The task was not found",
            Refusal::KeyConflict => b"The request key was already used",
        },
        NoticeKind::WentOffline => b"Connection lost; your requests will be retried",
        NoticeKind::CameBack => b"Connection restored",
    }
}

#[must_use]
pub fn numbered(prefix: &[u8], number: u64) -> Box<[u8]> {
    let digits = Decimal::of(number);
    let len = prefix.len().checked_add(digits.as_bytes().len()).expect("a label fits in usize");
    let mut writer = Writer::new(len);
    writer.put(prefix).expect("prefix fits the measured label");
    writer.put(digits.as_bytes()).expect("digits fit the measured label");
    writer.finish()
}

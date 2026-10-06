//! Wording for W2's frame, chats and notices, using sized bytes.

use alloc::boxed::Box;
use skein_lib::{Decimal, Wall, Writer};
use temper_web_domain::{Choice, EndKind, HoldReason, LinkState, NoticeKind, Offset, Refusal, TaskPhase, Why};

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
        NoticeKind::StreamFull => b"Too many live watches",
        NoticeKind::ObjectFull => b"This page cannot show every task object",
        NoticeKind::StaleObject => b"This card is no longer available",
        NoticeKind::Decided => b"The decision was recorded",
        NoticeKind::WordsTooLong => b"Your words are too long",
        NoticeKind::Refused(reason) => refusal(reason),
        NoticeKind::WentOffline => b"Connection lost; your requests will be retried",
        NoticeKind::CameBack => b"Connection restored",
    }
}

#[must_use]
pub fn refusal(reason: Refusal) -> &'static [u8] {
    match reason {
        Refusal::Role => b"Your role cannot do that",
        Refusal::Limit => b"The limit was reached",
        Refusal::Ended => b"This task has ended",
        Refusal::Unknown => b"The task was not found",
        Refusal::KeyConflict => b"The request key was already used",
        Refusal::Authority { .. } => b"You do not have authority for that decision",
        Refusal::Moved { .. } => b"The task changed before your decision",
        Refusal::Standing => b"Someone else is waiting to decide this",
        Refusal::NoFurther => b"This cannot be passed further",
        Refusal::NeedsAmend => b"Amend the task before releasing it",
    }
}

#[must_use]
pub fn phase(phase: TaskPhase) -> &'static [u8] {
    match phase {
        TaskPhase::Starting => b"Starting",
        TaskPhase::Running => b"Writing",
        TaskPhase::Parked => b"Parked; your words resume it",
        TaskPhase::Held(reason) => hold(reason),
        TaskPhase::Ended(kind) => match kind {
            EndKind::Done => b"Done",
            EndKind::Failed => b"Failed",
            EndKind::Cancelled => b"Cancelled",
        },
    }
}

#[must_use]
pub fn hold(reason: HoldReason) -> &'static [u8] {
    match reason {
        HoldReason::Tries => b"Held: out of tries",
        HoldReason::Budget => b"Held: out of budget",
        HoldReason::Stopped => b"Held: stopped by a person",
    }
}

#[must_use]
pub fn release_does(reason: HoldReason) -> &'static [u8] {
    match reason {
        HoldReason::Tries => b"Releasing it starts a new run and resets its tries.",
        HoldReason::Budget => b"Amend its budget before releasing it, or it will hold again.",
        HoldReason::Stopped => b"Releasing it starts the task again from saved work.",
    }
}

#[must_use]
pub fn leaving(why: &Why, offset: Offset) -> Box<[u8]> {
    match why {
        Why::Decided { by, choice, at } => {
            let verb: &[u8] = match choice {
                Choice::Released => b" released at ",
                Choice::Rejected => b" left this held at ",
                Choice::Passed => b" passed this up at ",
            };
            let clock = clock(*at, offset);
            let head = by.name.len().checked_add(verb.len()).expect("decision wording fits usize");
            let len = head.checked_add(clock.len()).expect("decision wording fits usize");
            let mut writer = Writer::new(len);
            writer.put(&by.name).expect("name fits measured wording");
            writer.put(verb).expect("verb fits measured wording");
            writer.put(&clock).expect("clock fits measured wording");
            writer.finish()
        }
        Why::Withdrawn => Box::from(b"Withdrawn".as_slice()),
        Why::Ended => Box::from(b"Task ended".as_slice()),
    }
}

#[must_use]
pub fn clock(at: Wall, offset: Offset) -> Box<[u8]> {
    let local = i64::try_from(at.as_secs()).unwrap_or(i64::MAX).saturating_add(i64::from(offset.0)).rem_euclid(86_400);
    let hour = u64::try_from(local.div_euclid(3_600)).expect("hour is nonnegative");
    let minute = u64::try_from(local.rem_euclid(3_600).div_euclid(60)).expect("minute is nonnegative");
    let mut writer = Writer::new(5);
    two_digits(&mut writer, hour);
    writer.put(b":").expect("separator fits clock");
    two_digits(&mut writer, minute);
    writer.finish()
}

fn two_digits(writer: &mut Writer, number: u64) {
    let digits = Decimal::of(number);
    if digits.as_bytes().len() == 1 {
        writer.put(b"0").expect("zero fits clock");
    }
    writer.put(digits.as_bytes()).expect("digits fit clock");
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

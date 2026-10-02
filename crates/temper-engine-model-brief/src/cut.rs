//! Cutting a brief to its budgets (engine-model.md, section 9): each section
//! to its kind's, and all of them together to the brief's.
//!
//! A section's content is its parts one after another, in the order its
//! source gives them. When they do not fit, what goes first is the kind's:
//!
//! ```text
//! kind          order        what goes first
//! item          first        the farthest ancestors, then the tail of what is left
//! comments      last         the oldest comments, then the head of the oldest kept
//! dependencies  even, start  each outcome's excess over an even share, from its tail
//! ci            even, end    each failed check's excess over an even share, from the
//!                            head of its output: a failure shows at the end
//! reviews       even, start  each review comment's excess over an even share, from its tail
//! pull          first        the tail
//! attempts      last         the oldest attempts, then the head of the oldest kept
//! plan          first        the tail
//! notes         lines        whole lines, from the last; the last part, which says
//!                            how many entries did not fit, stays
//! template      first        the tail
//! ```
//!
//! A section is read as it is cut: a read of a kind cut first or last, or by
//! lines, brings at most the kind's budget, as one run from the end it keeps
//! (or whole lines); a read of an even kind brings what a read may, each part
//! cut to an even share of it, so that every part reaches the brief.
//!
//! Every cut is told where it is, by a line saying how many bytes it left
//! out, `[N bytes cut]`, its count in decimal digits; cuts next to each
//! other (a part's tail, and the next part left out whole) are one line. A
//! list of dependencies cut at the entrance ends its section with a line
//! saying how many items it left out, `[N items cut]`. The lines count in
//! the budget. A cut never splits a UTF-8 sequence: it moves to the
//! sequence's edge, and leaves out its bytes too.
//!
//! How much to keep is one measure: the bytes kept from the start of the
//! content (first), from its end (last), of each part (even), or the whole
//! lines kept (lines). The largest measure whose text fits is kept; with
//! none kept, a section is its cut lines, which every budget holds (the
//! limits see to it). The text's length grows with the measure, except
//! where a part becomes whole and its cut line goes: past the end of the
//! content, or, for an even share, at each part's length. So the measure is
//! found by bisection within each stretch between those points, from the
//! highest stretch down. The text is measured before it is written, so each
//! section is written once, into a box of its length.
//!
//! The brief's budget holds the sections together. When they would not fit
//! it at their own budgets, each is held to the same bound, the largest
//! under which the sections, each at the least of its uncut length, its
//! budget and the bound, fit: short sections stay whole, and long ones share
//! what is left evenly. A missing section takes nothing.

use alloc::boxed::Box;

use temper_lib::{Decimal, List, Token, Writer};

use crate::boundary::{Body, Fit, Keep, Kind, Part, Request, Section, Source};
use crate::brief::{Content, Slot};
use crate::limits::{Limits, budget};

/// The longest cut line, and items line: a newline before it, and the most
/// digits a count has.
pub(crate) const CUT_LINE: usize = b"\n[".len() + 20 + b" bytes cut]\n".len();
pub(crate) const ITEMS_LINE: usize = b"\n[".len() + 10 + b" items cut]\n".len();

/// The least a section's budget may be: room for both lines, so that a
/// section cut to nothing still says what it left out.
pub(crate) const FLOOR: usize = CUT_LINE + ITEMS_LINE;

/// What a section loses first, when it does not fit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Order {
    /// Its first bytes are kept: later parts go first, then the tail of the
    /// last one kept.
    First,
    /// Its last bytes are kept: earlier parts go first, then the head of the
    /// first one kept.
    Last,
    /// Each part keeps an even share, from this end.
    Even(Keep),
    /// Whole parts are kept from the start, and the last part with them.
    Lines,
}

fn order(kind: Kind) -> Order {
    match kind {
        Kind::Item | Kind::Pull | Kind::Plan | Kind::Template => Order::First,
        Kind::Comments | Kind::Attempts => Order::Last,
        Kind::Dependencies | Kind::Reviews => Order::Even(Keep::Start),
        Kind::Ci => Order::Even(Keep::End),
        Kind::Notes => Order::Lines,
    }
}

/// Which end of a part `order` keeps.
fn end(order: Order) -> Keep {
    match order {
        Order::First | Order::Lines => Keep::Start,
        Order::Last => Keep::End,
        Order::Even(keep) => keep,
    }
}

/// The read of `source`, as its section is cut: within the kind's budget,
/// or, for an even share, what a read may bring.
pub(crate) fn read(owner: Token, source: Source, limits: &Limits) -> Request {
    let kind = source.kind();
    let how = order(kind);
    let fit = match how {
        Order::First | Order::Last => Fit::Run,
        Order::Even(_) => Fit::Each,
        Order::Lines => Fit::Lines,
    };
    Request::Read { owner, source, keep: end(how), fit, parts: limits.parts, bytes: most(kind, limits) }
}

/// The most bytes a read for a section of `kind` may bring.
pub(crate) fn most(kind: Kind, limits: &Limits) -> u32 {
    match order(kind) {
        Order::First | Order::Last | Order::Lines => limits.read_bytes.min(budget(&limits.budgets, kind)),
        Order::Even(_) => limits.read_bytes,
    }
}

/// A brief rendered: its sections, how many of them were cut, and how many
/// are missing.
#[derive(Debug)]
pub(crate) struct Rendered {
    pub(crate) sections: Box<[Section]>,
    pub(crate) cut: u32,
    pub(crate) missing: u32,
}

/// The sections of `slots`, in their order, each within its kind's budget
/// and all within the brief's.
pub(crate) fn brief(slots: &[Slot], limits: &Limits) -> Rendered {
    let total = size(limits.brief_bytes);
    let bound = if asked(slots, limits, usize::MAX) <= total { usize::MAX } else { share(slots, limits, total) };
    let mut sections = List::with_capacity(count(slots.len()));
    let mut cut = 0_u32;
    let mut missing = 0_u32;
    for slot in slots {
        let body = match &slot.content {
            Content::Got(parts) => {
                let room = size(budget(&limits.budgets, slot.kind)).min(bound);
                let (bytes, whole) = section(slot.kind, parts, slot.items, room);
                if !whole {
                    cut = cut.saturating_add(1);
                }
                Body::Text(bytes)
            }
            Content::Missing(unread) => {
                missing = missing.saturating_add(1);
                Body::Missing(*unread)
            }
            Content::Asked(_) => unreachable!("a brief is rendered once every section is read or missing"),
        };
        sections.push(Section { kind: slot.kind, body }).expect("room for a section per slot");
    }
    Rendered { sections: sections.into_boxed(), cut, missing }
}

/// The bound every section of `slots` is held to so that all fit `total`:
/// the largest under which they do.
fn share(slots: &[Slot], limits: &Limits, total: usize) -> usize {
    // Every budget holds a section's lines, and the brief's those of each
    // section: the least bound fits.
    let mut low = FLOOR;
    let mut high = FLOOR;
    for slot in slots {
        high = high.max(size(budget(&limits.budgets, slot.kind)));
    }
    for _ in 0..usize::BITS {
        if low >= high {
            break;
        }
        let mid = low.saturating_add(high.saturating_sub(low).div_ceil(2));
        if asked(slots, limits, mid) <= total {
            low = mid;
        } else {
            high = mid.saturating_sub(1);
        }
    }
    low
}

/// How long the sections of `slots` are together, each at the least of its
/// uncut length, its budget and `bound`.
fn asked(slots: &[Slot], limits: &Limits, bound: usize) -> usize {
    let mut sum: usize = 0;
    for slot in slots {
        let span = match &slot.content {
            Content::Got(parts) => {
                uncut(slot.kind, parts, slot.items).min(size(budget(&limits.budgets, slot.kind))).min(bound)
            }
            Content::Asked(_) | Content::Missing(_) => 0,
        };
        sum = sum.saturating_add(span);
    }
    sum
}

/// How long a section of `kind` holding `parts` is when the brief cuts no
/// bytes: its parts, a line for each cut their source made, and a line for
/// the `items` cut at the entrance.
pub(crate) fn uncut(kind: Kind, parts: &[Part], items: u32) -> usize {
    let how = order(kind);
    measure(how, whole(how, parts), parts, items)
}

/// A section of `kind` holding `parts`, less `items` items cut at the
/// entrance, within `room` bytes; and whether it is whole: nothing cut,
/// by the brief, its source or the entrance.
pub(crate) fn section(kind: Kind, parts: &[Part], items: u32, room: usize) -> (Box<[u8]>, bool) {
    let how = order(kind);
    let all = whole(how, parts);
    let kept = fit(how, parts, items, room);
    let mut text = Text::writing(measure(how, kept, parts, items));
    put(&mut text, how, kept, parts, items);
    let mut left = 0_u64;
    for part in parts {
        left = left.saturating_add(part.left);
    }
    (text.finish(), kept == all && left == 0 && items == 0)
}

/// The measure that keeps all of `parts` under `order`.
fn whole(order: Order, parts: &[Part]) -> u64 {
    let mut all = 0_u64;
    for part in parts {
        let span = length(&part.bytes);
        all = match order {
            Order::First | Order::Last => all.saturating_add(span),
            Order::Even(_) => all.max(span),
            Order::Lines => all.saturating_add(1),
        };
    }
    all
}

/// The largest measure whose text fits `room`.
fn fit(order: Order, parts: &[Part], items: u32, room: usize) -> u64 {
    let all = whole(order, parts);
    if measure(order, all, parts, items) <= room {
        return all;
    }
    assert!(measure(order, 0, parts, items) <= room, "a budget holds a section's lines");
    // The highest stretch whose lowest measure fits holds the answer.
    let mut top = all.saturating_sub(1);
    for _ in 0..=parts.len() {
        let bottom = floor(order, parts, top);
        if measure(order, bottom, parts, items) <= room {
            return bisect(order, parts, items, room, bottom, top);
        }
        top = bottom.saturating_sub(1);
    }
    unreachable!("the lowest stretch starts with nothing kept, which fits")
}

/// Where the stretch holding the measure `top` starts: the highest point at
/// or below it where a part becomes whole and loses its cut line (for an
/// even share; there is none below the content's end for the others), or
/// nothing kept.
fn floor(order: Order, parts: &[Part], top: u64) -> u64 {
    let mut highest = 0;
    match order {
        Order::First | Order::Last | Order::Lines => {}
        Order::Even(_) => {
            for part in parts {
                let span = length(&part.bytes);
                if part.left == 0 && span <= top {
                    highest = highest.max(span);
                }
            }
        }
    }
    highest
}

/// The largest measure from `low` to `high` whose text fits `room`, where
/// the text grows with the measure and `low`'s fits.
fn bisect(order: Order, parts: &[Part], items: u32, room: usize, low: u64, high: u64) -> u64 {
    let mut low = low;
    let mut high = high;
    for _ in 0..u64::BITS {
        if low >= high {
            break;
        }
        let mid = low.saturating_add(high.saturating_sub(low).div_ceil(2));
        if measure(order, mid, parts, items) <= room {
            low = mid;
        } else {
            high = mid.saturating_sub(1);
        }
    }
    low
}

/// How long the text of `parts` is at the measure `kept` under `order`.
fn measure(order: Order, kept: u64, parts: &[Part], items: u32) -> usize {
    let mut text = Text::measuring();
    put(&mut text, order, kept, parts, items);
    text.len
}

/// Puts `parts` into `text`, keeping of each what the measure `kept` keeps
/// under `order`, with a cut line wherever bytes were left out, and the
/// line for `items` cut at the entrance last.
fn put(text: &mut Text, order: Order, kept: u64, parts: &[Part], items: u32) {
    let keep = end(order);
    let mut total = 0_u64;
    for part in parts {
        total = total.saturating_add(length(&part.bytes));
    }
    let trailer = parts.len().saturating_sub(1);
    // The content before the part at hand, and what was cut since the last
    // bytes kept.
    let mut before = 0_u64;
    let mut pending = 0_u64;
    for (index, part) in parts.iter().enumerate() {
        let span = length(&part.bytes);
        let after = total.saturating_sub(before).saturating_sub(span);
        let wanted = match order {
            Order::First => kept.saturating_sub(before),
            Order::Last => kept.saturating_sub(after),
            Order::Even(_) => kept,
            Order::Lines => lines(kept, index, trailer, span),
        };
        let bytes = edge(&part.bytes, keep, wanted.min(span));
        let lost = part.left.saturating_add(span.saturating_sub(length(bytes)));
        match keep {
            Keep::Start => {
                pending = keep_bytes(text, pending, bytes);
                pending = pending.saturating_add(lost);
            }
            Keep::End => {
                pending = pending.saturating_add(lost);
                pending = keep_bytes(text, pending, bytes);
            }
        }
        before = before.saturating_add(span);
    }
    if pending > 0 {
        cut_line(text, pending, b" bytes cut]\n");
    }
    if items > 0 {
        cut_line(text, u64::from(items), b" items cut]\n");
    }
}

/// The bytes of the part at `index`, `span` long, that `kept` whole lines
/// keep, the last part being at `trailer`: none for none kept; else the last
/// part, and the first `kept - 1` others.
fn lines(kept: u64, index: usize, trailer: usize, span: u64) -> u64 {
    let place = u64::try_from(index).expect("an index fits a u64");
    if kept > 0 && (index == trailer || place < kept.saturating_sub(1)) { span } else { 0 }
}

/// Puts `bytes` into `text` after the line for the `pending` bytes cut
/// before them, if there are any: what is pending after.
fn keep_bytes(text: &mut Text, pending: u64, bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return pending;
    }
    if pending > 0 {
        cut_line(text, pending, b" bytes cut]\n");
    }
    text.put(bytes);
    0
}

/// The line that tells `count` bytes or items were cut, as `unit` says: on a
/// line of its own, after what the section holds so far.
fn cut_line(text: &mut Text, count: u64, unit: &[u8]) {
    if text.len > 0 {
        text.put(b"\n");
    }
    text.put(b"[");
    text.put(Decimal::of(count).as_bytes());
    text.put(unit);
}

/// The `wanted` bytes at the end `keep` of `bytes`, fewer if that would
/// split a UTF-8 sequence.
fn edge(bytes: &[u8], keep: Keep, wanted: u64) -> &[u8] {
    let wanted = usize::try_from(wanted).expect("no more than a part holds");
    // A sequence has at most three continuation bytes after its first.
    match keep {
        Keep::Start => {
            let mut stop = wanted;
            for _ in 0..3 {
                if continues(bytes.get(stop)) {
                    stop = stop.saturating_sub(1);
                }
            }
            bytes.get(..stop).expect("within the part")
        }
        Keep::End => {
            let mut start = bytes.len().checked_sub(wanted).expect("no more than a part holds");
            for _ in 0..3 {
                if continues(bytes.get(start)) {
                    start = start.saturating_add(1);
                }
            }
            bytes.get(start..).expect("within the part")
        }
    }
}

/// Whether `byte` continues a UTF-8 sequence rather than starting one.
fn continues(byte: Option<&u8>) -> bool {
    match byte {
        Some(byte) => byte & 0b1100_0000 == 0b1000_0000,
        None => false,
    }
}

fn length(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("a length fits a u64")
}

fn size(bytes: u32) -> usize {
    usize::try_from(bytes).expect("a u32 fits a usize")
}

fn count(len: usize) -> u32 {
    u32::try_from(len).expect("no more sections than the limits")
}

/// A section's text, measured, then written at that length.
#[derive(Debug)]
struct Text {
    /// The bytes put so far.
    len: usize,
    /// Where they go, once the text has been measured.
    writer: Option<Writer>,
}

impl Text {
    fn measuring() -> Text {
        Text { len: 0, writer: None }
    }

    fn writing(len: usize) -> Text {
        Text { len: 0, writer: Some(Writer::new(len)) }
    }

    fn put(&mut self, bytes: &[u8]) {
        self.len = self.len.saturating_add(bytes.len());
        if let Some(writer) = &mut self.writer {
            writer.put(bytes).expect("a text is written as it was measured");
        }
    }

    fn finish(self) -> Box<[u8]> {
        self.writer.expect("a text is finished once written").finish()
    }
}

#[cfg(test)]
pub(crate) mod probe {
    //! What the step tests check of the measure: that the one kept is the
    //! largest that fits.

    use super::{Part, fit, measure, order, whole};
    use crate::boundary::Kind;

    /// The measure kept for `parts` of `kind` within `budget`, the whole
    /// one, and how long the text is at any measure.
    pub(crate) fn kept(kind: Kind, parts: &[Part], budget: usize) -> (u64, u64) {
        let how = order(kind);
        (fit(how, parts, 0, budget), whole(how, parts))
    }

    pub(crate) fn length(kind: Kind, parts: &[Part], kept: u64) -> usize {
        measure(order(kind), kept, parts, 0)
    }
}

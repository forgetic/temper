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
//! notes         first        the last lines of the index
//! template      first        the tail
//! ```
//!
//! A source with more than a read may bring cuts its parts at the same end,
//! and says how much it left out of each.
//!
//! Every cut is told where it is, by a line saying how many bytes it left
//! out, `[N bytes cut]`, its count in decimal digits; cuts next to each
//! other (a part's tail, and the next part left out whole) are one line. The
//! lines count in the budget. A cut never splits a UTF-8 sequence: it moves
//! to the sequence's edge, and leaves out its bytes too.
//!
//! How much to keep is one measure, found by bisection: the bytes kept from
//! the start of the content (first), from its end (last), or of each part
//! (even). The largest measure whose text fits is kept; with none kept, a
//! section is a single cut line, which every budget holds (the limits see to
//! it). The text is measured before it is written, so each section is
//! written once, into a box of its length.
//!
//! The brief's budget holds the sections together. When they would not fit
//! it at their own budgets, each is held to the same bound, the largest
//! under which the sections, each at the least of its uncut length, its
//! budget and the bound, fit: short sections stay whole, and long ones share
//! what is left evenly. A missing section takes nothing.

use alloc::boxed::Box;

use temper_lib::{Decimal, List, Writer};

use crate::boundary::{Body, Keep, Kind, Part, Section};
use crate::brief::{Content, Slot};
use crate::limits::{Limits, budget};

/// The longest cut line: a newline before it, and the most digits a count
/// has.
pub(crate) const CUT_LINE: usize = b"\n[".len() + 20 + b" bytes cut]\n".len();

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
}

fn order(kind: Kind) -> Order {
    match kind {
        Kind::Item | Kind::Pull | Kind::Plan | Kind::Notes | Kind::Template => Order::First,
        Kind::Comments | Kind::Attempts => Order::Last,
        Kind::Dependencies | Kind::Reviews => Order::Even(Keep::Start),
        Kind::Ci => Order::Even(Keep::End),
    }
}

/// Which end of a part a section of `kind` keeps: what its source keeps too,
/// when it has more than a read may bring.
pub(crate) fn keep(kind: Kind) -> Keep {
    end(order(kind))
}

/// Which end of a part `order` keeps.
fn end(order: Order) -> Keep {
    match order {
        Order::First => Keep::Start,
        Order::Last => Keep::End,
        Order::Even(keep) => keep,
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
/// and all within the brief's. A slot still asked for is missing.
pub(crate) fn brief(slots: &[Slot], limits: &Limits) -> Rendered {
    let total = size(limits.brief_bytes);
    let bound = if asked(slots, limits, usize::MAX) <= total { usize::MAX } else { share(slots, limits, total) };
    let mut sections = List::with_capacity(count(slots.len()));
    let (mut cut, mut missing) = (0_u32, 0_u32);
    for slot in slots {
        let body = match &slot.content {
            Content::Got(parts) => {
                let (bytes, whole) = section(slot.kind, parts, size(budget(&limits.budgets, slot.kind)).min(bound));
                if !whole {
                    cut = cut.saturating_add(1);
                }
                Body::Text(bytes)
            }
            Content::Asked | Content::Missing => {
                missing = missing.saturating_add(1);
                Body::Missing
            }
        };
        sections.push(Section { kind: slot.kind, body }).expect("room for a section per slot");
    }
    Rendered { sections: sections.into_boxed(), cut, missing }
}

/// The bound every section of `slots` is held to so that all fit `total`:
/// the largest under which they do.
fn share(slots: &[Slot], limits: &Limits, total: usize) -> usize {
    // Every section holds a cut line, and the brief one in each: the least
    // bound fits.
    let (mut low, mut high) = (CUT_LINE, CUT_LINE);
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
    let mut asked: usize = 0;
    for slot in slots {
        let length = match &slot.content {
            Content::Got(parts) => uncut(slot.kind, parts).min(size(budget(&limits.budgets, slot.kind))).min(bound),
            Content::Asked | Content::Missing => 0,
        };
        asked = asked.saturating_add(length);
    }
    asked
}

/// How long a section of `kind` holding `parts` is when the brief cuts
/// nothing: its parts, and a line for each cut their source made.
fn uncut(kind: Kind, parts: &[Part]) -> usize {
    let order = order(kind);
    measure(order, whole(order, parts), parts)
}

/// A section of `kind` holding `parts`, within `budget` bytes, and whether
/// it is whole: nothing cut, by the brief or by its source.
pub(crate) fn section(kind: Kind, parts: &[Part], budget: usize) -> (Box<[u8]>, bool) {
    let order = order(kind);
    let whole = whole(order, parts);
    let kept = fit(order, parts, budget, whole);
    let mut text = Text::writing(measure(order, kept, parts));
    put(&mut text, order, kept, parts);
    let mut left = 0_u64;
    for part in parts {
        left = left.saturating_add(part.left);
    }
    (text.finish(), kept == whole && left == 0)
}

/// The measure that keeps every byte of `parts` under `order`.
fn whole(order: Order, parts: &[Part]) -> u64 {
    let mut whole = 0_u64;
    for part in parts {
        let length = length(&part.bytes);
        whole = match order {
            Order::First | Order::Last => whole.saturating_add(length),
            Order::Even(_) => whole.max(length),
        };
    }
    whole
}

/// The largest measure up to `whole` whose text fits `budget`.
fn fit(order: Order, parts: &[Part], budget: usize, whole: u64) -> u64 {
    if measure(order, whole, parts) <= budget {
        return whole;
    }
    assert!(measure(order, 0, parts) <= budget, "a budget holds a cut line");
    let (mut low, mut high) = (0, whole);
    for _ in 0..u64::BITS {
        if low >= high {
            break;
        }
        let mid = low.saturating_add(high.saturating_sub(low).div_ceil(2));
        if measure(order, mid, parts) <= budget {
            low = mid;
        } else {
            high = mid.saturating_sub(1);
        }
    }
    low
}

/// How long the text of `parts` is at the measure `kept` under `order`.
fn measure(order: Order, kept: u64, parts: &[Part]) -> usize {
    let mut text = Text::measuring();
    put(&mut text, order, kept, parts);
    text.len
}

/// Puts `parts` into `text`, keeping of each what the measure `kept` keeps
/// under `order`, and a cut line wherever bytes were left out.
fn put(text: &mut Text, order: Order, kept: u64, parts: &[Part]) {
    let end = end(order);
    let mut total = 0_u64;
    for part in parts {
        total = total.saturating_add(length(&part.bytes));
    }
    // The content before the part at hand, and what was cut since the last
    // bytes kept.
    let mut before = 0_u64;
    let mut pending = 0_u64;
    for part in parts {
        let length = length(&part.bytes);
        let after = total.saturating_sub(before).saturating_sub(length);
        let wanted = match order {
            Order::First => kept.saturating_sub(before),
            Order::Last => kept.saturating_sub(after),
            Order::Even(_) => kept,
        };
        let bytes = edge(&part.bytes, end, wanted.min(length));
        let lost = part.left.saturating_add(length.saturating_sub(self::length(bytes)));
        match end {
            Keep::Start => {
                pending = keep_bytes(text, pending, bytes);
                pending = pending.saturating_add(lost);
            }
            Keep::End => {
                pending = pending.saturating_add(lost);
                pending = keep_bytes(text, pending, bytes);
            }
        }
        before = before.saturating_add(length);
    }
    if pending > 0 {
        cut_line(text, pending);
    }
}

/// Puts `bytes` into `text` after the line for the `pending` bytes cut
/// before them, if there are any: what is pending after.
fn keep_bytes(text: &mut Text, pending: u64, bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return pending;
    }
    if pending > 0 {
        cut_line(text, pending);
    }
    text.put(bytes);
    0
}

/// The line that tells `count` bytes were cut: on a line of its own, after
/// what the section holds so far.
fn cut_line(text: &mut Text, count: u64) {
    if text.len > 0 {
        text.put(b"\n");
    }
    text.put(b"[");
    text.put(Decimal::of(count).as_bytes());
    text.put(b" bytes cut]\n");
}

/// The `wanted` bytes at `end` of `bytes`, fewer if that would split a UTF-8
/// sequence.
fn edge(bytes: &[u8], end: Keep, wanted: u64) -> &[u8] {
    let wanted = usize::try_from(wanted).expect("no more than a part holds");
    // A sequence has at most three continuation bytes after its first.
    match end {
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

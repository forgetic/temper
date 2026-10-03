//! The window a read answers with: which lines of a file, within the limit on
//! what a read answers with. A line is the bytes up to and including a `\n`,
//! or the bytes after the last one; finding them is counting bytes, not
//! parsing. Numbering the lines for the LLM is the protocol layer's rendering.

use alloc::boxed::Box;

use temper_lib::bytes::{self, copy_of};

use crate::call::Outcome;

/// The lines a read asks for: those after the first `skip`, at most `lines`
/// of them if given.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Span {
    pub(crate) skip: u32,
    pub(crate) lines: Option<u32>,
}

/// The read of `span` of `content`, at most `max` bytes of it. It is whole
/// lines, but for a first line longer than `max`, which is cut there.
pub(crate) fn window(content: Box<[u8]>, span: Span, max: u32) -> Outcome {
    let Span { skip, lines } = span;
    let total = count(&content);
    let (skipped, start) = line_start(&content, skip, total);
    let rest = content.get(start..).expect("a line starts within the content");
    let max = usize::try_from(max).unwrap_or(usize::MAX);
    // Whole lines, while they fit, and no more than there are.
    let mut end: usize = 0;
    let mut taken: u32 = 0;
    for _ in 0..lines.unwrap_or(total).min(total) {
        if end >= rest.len() {
            break;
        }
        let line_end = next_line(rest, end);
        if line_end > max {
            break;
        }
        end = line_end;
        taken = taken.saturating_add(1);
    }
    // A first line too long to answer with whole.
    let cut = taken == 0 && end < rest.len() && lines != Some(0);
    if cut {
        end = max;
        taken = 1;
    }
    let content = if start == 0 && end == content.len() {
        content
    } else {
        copy_of(rest.get(..end).expect("the window is within the content"))
    };
    Outcome::Read { content, skipped, lines: taken, total, cut }
}

/// How many lines `content` has.
fn count(content: &[u8]) -> u32 {
    let ended = bytes::count(content, b"\n", u32::MAX);
    match content.last() {
        Some(b'\n') | None => ended,
        Some(_) => ended.saturating_add(1),
    }
}

/// How many lines come before the `skip`th, at most the `total` of
/// `content`, and where it starts: the end of `content` past its last line.
fn line_start(content: &[u8], skip: u32, total: u32) -> (u32, usize) {
    let skipped = skip.min(total);
    let mut start: usize = 0;
    for _ in 0..skipped {
        start = next_line(content, start);
    }
    (skipped, start)
}

/// Where the line that starts at `start` of `content` ends: past its `\n`, or
/// at the end of `content`.
fn next_line(content: &[u8], start: usize) -> usize {
    match bytes::find_from(content, b"\n", start) {
        Some(at) => at.saturating_add(1),
        None => content.len(),
    }
}

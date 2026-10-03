//! Edits: a snippet of a file replaced with another. Whether the snippet is
//! there, once or more, is policy, so it is decided here, over the file io
//! loaded, with lib's linear byte search; the new file is spliced together at
//! its final length.

use alloc::boxed::Box;

use temper_lib::bytes::{count, find_from};
use temper_lib::{List, Writer};

use crate::call::Outcome;
use crate::limits::Limits;

/// What an edit asks for: `old` replaced with `new`, at its one occurrence, or
/// at every one if `all`.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct Edit {
    pub(crate) old: Box<[u8]>,
    pub(crate) new: Box<[u8]>,
    pub(crate) all: bool,
}

/// `content` with `edit` made, and how many occurrences it replaced; or the
/// outcome that refuses it: `NoMatch`, `Ambiguous` with the lines of the
/// first matches, or `TooLarge` if the file would outgrow what the tools
/// store.
pub(crate) fn apply(content: &[u8], edit: &Edit, limits: &Limits) -> Result<(Box<[u8]>, u32), Outcome> {
    let matches = count(content, &edit.old, u32::MAX);
    if matches == 0 {
        return Err(Outcome::NoMatch);
    }
    if matches > 1 && !edit.all {
        return Err(Outcome::Ambiguous { count: matches, lines: lines(content, &edit.old, limits.match_lines) });
    }
    let size = size(content.len(), matches, edit.old.len(), edit.new.len()).unwrap_or(u64::MAX);
    if size > u64::from(limits.file_bytes) {
        return Err(Outcome::TooLarge { size });
    }
    let mut writer = Writer::new(usize::try_from(size).expect("no larger than the file limit"));
    let mut from: usize = 0;
    for _ in 0..matches {
        let at = find_from(content, &edit.old, from).expect("counted above");
        writer.put(content.get(from..at).expect("a match is past the last")).expect("sized above");
        writer.put(&edit.new).expect("sized above");
        from = at.checked_add(edit.old.len()).expect("a match is within the content");
    }
    writer.put(content.get(from..).expect("the last match is within the content")).expect("sized above");
    Ok((writer.finish(), matches))
}

/// The size of a file of `len` bytes with `matches` occurrences of `old`
/// bytes replaced by `new` bytes, or `None` past a `u64`.
fn size(len: usize, matches: u32, old: usize, new: usize) -> Option<u64> {
    let matches = u64::from(matches);
    let removed = u64::try_from(old).ok()?.checked_mul(matches)?;
    let added = u64::try_from(new).ok()?.checked_mul(matches)?;
    u64::try_from(len).ok()?.checked_sub(removed)?.checked_add(added)
}

/// The numbers of the lines, counting from 1, where the first `max`
/// occurrences of `old` in `content` start.
fn lines(content: &[u8], old: &[u8], max: u32) -> Box<[u32]> {
    let mut lines = List::with_capacity(max);
    let mut line: u32 = 1;
    let mut from: usize = 0;
    let mut counted: usize = 0;
    for _ in 0..max {
        let Some(at) = find_from(content, old, from) else {
            break;
        };
        let before = content.get(counted..at).expect("a match is past the last");
        line = line.saturating_add(count(before, b"\n", u32::MAX));
        counted = at;
        lines.push(line).expect("room for `max` lines");
        from = at.saturating_add(old.len());
    }
    lines.into_boxed()
}

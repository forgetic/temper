//! Resolve every edit against the unchanged original byte sequence.

use crate::workspace_edits::ExactEdit;

pub(super) fn apply(original: &[u8], edits: &[ExactEdit]) -> Result<Vec<u8>, String> {
    let source = std::str::from_utf8(original).map_err(|error| error.to_string())?;
    let mut ranges = Vec::new();
    for edit in edits {
        let start = source
            .find(&edit.old_text)
            .ok_or("oldText does not match original contents")?;
        // Advance by one scalar, not by the whole match, so overlapping repeated
        // matches such as "aa" in "aaa" are also rejected as ambiguous.
        let next = start
            + edit
                .old_text
                .chars()
                .next()
                .ok_or("oldText must be nonempty")?
                .len_utf8();
        if source[next..].contains(&edit.old_text) {
            return Err("oldText matches original contents more than once".into());
        }
        ranges.push((start, start + edit.old_text.len(), edit.new_text.as_bytes()));
    }
    ranges.sort_unstable_by_key(|range| range.0);
    let mut cursor = 0;
    let mut output = Vec::new();
    for (start, end, replacement) in ranges {
        if start < cursor {
            return Err("edits overlap in original contents".into());
        }
        output.extend_from_slice(&original[cursor..start]);
        output.extend_from_slice(replacement);
        cursor = end;
    }
    output.extend_from_slice(&original[cursor..]);
    Ok(output)
}

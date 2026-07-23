use super::{METADATA_BEGIN, METADATA_END, MetadataBlockSpan, MetadataError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MarkdownFence {
    marker: u8,
    length: usize,
}

pub(super) fn metadata_block_spans(body: &str) -> Result<Vec<MetadataBlockSpan>, MetadataError> {
    let mut blocks = Vec::new();
    let mut cursor = 0;
    let mut active_fence = None;

    while cursor < body.len() {
        if let Some(fence) = active_fence {
            let (line, after_line) = line_at(body, cursor);
            if is_closing_fence(line, fence) {
                active_fence = None;
            }
            cursor = after_line;
            continue;
        }

        if cursor == 0 || body.as_bytes()[cursor - 1] == b'\n' {
            let (line, after_line) = line_at(body, cursor);
            if let Some(fence) = opening_fence(line) {
                active_fence = Some(fence);
                cursor = after_line;
                continue;
            }
        }

        if body[cursor..].starts_with(METADATA_BEGIN) && metadata_begin_has_boundary(body, cursor) {
            let after_begin = cursor + METADATA_BEGIN.len();
            let Some(relative_end) = body[after_begin..].find(METADATA_END) else {
                return Err(MetadataError::Unterminated);
            };
            let end = after_begin + relative_end + METADATA_END.len();
            blocks.push(MetadataBlockSpan { start: cursor, end });
            cursor = end;
            continue;
        }

        if body.as_bytes()[cursor] == b'`' {
            let length = backtick_run_length(body, cursor);
            let after_open = cursor + length;
            if !is_escaped(body, cursor) {
                if let Some(after_close) = matching_backtick_close(body, after_open, length) {
                    cursor = after_close;
                    continue;
                }
            }
            cursor = after_open;
            continue;
        }

        cursor += body[cursor..]
            .chars()
            .next()
            .expect("cursor is before end of body")
            .len_utf8();
    }

    Ok(blocks)
}

fn line_at(body: &str, start: usize) -> (&str, usize) {
    let after_line = body[start..]
        .find('\n')
        .map_or(body.len(), |relative| start + relative + 1);
    let line_with_ending = &body[start..after_line];
    let line = line_with_ending
        .strip_suffix('\n')
        .unwrap_or(line_with_ending);
    (line.strip_suffix('\r').unwrap_or(line), after_line)
}

fn opening_fence(line: &str) -> Option<MarkdownFence> {
    let bytes = line.as_bytes();
    let mut start = 0;
    while start < bytes.len() && bytes[start] == b' ' && start < 4 {
        start += 1;
    }
    if start > 3 {
        return None;
    }

    let marker = *bytes.get(start)?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = bytes[start..]
        .iter()
        .take_while(|byte| **byte == marker)
        .count();
    if length < 3 {
        return None;
    }
    if marker == b'`' && bytes[start + length..].contains(&b'`') {
        return None;
    }
    Some(MarkdownFence { marker, length })
}

fn is_closing_fence(line: &str, fence: MarkdownFence) -> bool {
    let bytes = line.as_bytes();
    let mut start = 0;
    while start < bytes.len() && bytes[start] == b' ' && start < 4 {
        start += 1;
    }
    if start > 3 || bytes.get(start) != Some(&fence.marker) {
        return false;
    }

    let length = bytes[start..]
        .iter()
        .take_while(|byte| **byte == fence.marker)
        .count();
    length >= fence.length
        && bytes[start + length..]
            .iter()
            .all(|byte| *byte == b' ' || *byte == b'\t')
}

fn metadata_begin_has_boundary(text: &str, start: usize) -> bool {
    text[start + METADATA_BEGIN.len()..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace)
}

fn backtick_run_length(text: &str, start: usize) -> usize {
    text.as_bytes()[start..]
        .iter()
        .take_while(|byte| **byte == b'`')
        .count()
}

fn is_escaped(text: &str, start: usize) -> bool {
    text.as_bytes()[..start]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}

fn matching_backtick_close(text: &str, mut cursor: usize, length: usize) -> Option<usize> {
    while let Some(relative) = text[cursor..].find('`') {
        let start = cursor + relative;
        let candidate_length = backtick_run_length(text, start);
        if candidate_length == length {
            return Some(start + candidate_length);
        }
        cursor = start + candidate_length;
    }
    None
}

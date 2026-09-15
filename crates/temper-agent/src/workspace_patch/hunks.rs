//! Recount hunk metadata without changing source/context rows.

use temper_agent_core::TargetAdmissionStatus;

type Result<T> = std::result::Result<T, TargetAdmissionStatus>;

struct Header<'a> {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    context: &'a str,
}

pub(super) fn canonical_hunk(
    lines: &[&str],
    start: usize,
    section_at: impl Fn(usize) -> bool,
) -> Result<(String, usize)> {
    let header = parse_header(lines[start]).ok_or(TargetAdmissionStatus::MalformedTarget)?;
    let first = start + 1;
    let boundary =
        |index| index == lines.len() || section_at(index) || lines[index].starts_with("@@ ");
    // Prefer each valid hunk's declared extent, including context that happens
    // to look like another file header. A later malformed hunk cannot change it.
    let end = strict_end(lines, first, &header, &boundary)
        .or_else(|| repaired_end(lines, first, &header, &section_at))
        .ok_or(TargetAdmissionStatus::MalformedTarget)?;
    let (old, new) =
        count_rows(&lines[first..end]).ok_or(TargetAdmissionStatus::MalformedTarget)?;
    if old + new == 0 {
        return Err(TargetAdmissionStatus::MalformedTarget);
    }
    let mut output = format!(
        "@@ -{},{} +{},{} @@{}\n",
        header.old_start, old, header.new_start, new, header.context
    );
    for line in &lines[first..end] {
        output.push_str(line);
        output.push('\n');
    }
    Ok((output, end))
}

fn strict_end(
    lines: &[&str],
    first: usize,
    header: &Header<'_>,
    boundary: &impl Fn(usize) -> bool,
) -> Option<usize> {
    let (mut old, mut new) = (0, 0);
    let mut cursor = first;
    while cursor < lines.len() && (old < header.old_count || new < header.new_count) {
        let (left, right) = row_counts(lines[cursor])?;
        old += left;
        new += right;
        if old > header.old_count || new > header.new_count {
            return None;
        }
        cursor += 1;
    }
    if lines.get(cursor) == Some(&"\\ No newline at end of file") {
        cursor += 1;
    }
    (old == header.old_count && new == header.new_count && boundary(cursor)).then_some(cursor)
}

fn repaired_end(
    lines: &[&str],
    first: usize,
    header: &Header<'_>,
    section_at: &impl Fn(usize) -> bool,
) -> Option<usize> {
    let mut old = 0;
    for cursor in first..lines.len() {
        let line = lines[cursor];
        if section_at(cursor) {
            // An indented header can be context. Do not reinterpret it while
            // the declared old-side context still needs source rows.
            if line.starts_with(' ') && old < header.old_count {
                return None;
            }
            return Some(cursor);
        }
        if line.starts_with("@@ ") {
            return Some(cursor);
        }
        if line.starts_with("--- ")
            && lines
                .get(cursor + 1)
                .is_some_and(|line| line.starts_with("+++ "))
            && lines
                .get(cursor + 2)
                .is_some_and(|line| line.starts_with("@@ "))
        {
            // Traditional patch framing must not smuggle an undeclared file.
            return None;
        }
        old += row_counts(line)?.0;
    }
    Some(lines.len())
}

fn count_rows(lines: &[&str]) -> Option<(usize, usize)> {
    let (mut old, mut new) = (0, 0);
    let mut previous_data = false;
    for line in lines {
        let (left, right) = row_counts(line)?;
        if left + right == 0 && !previous_data {
            return None;
        }
        previous_data = left + right != 0;
        old += left;
        new += right;
    }
    Some((old, new))
}

fn row_counts(line: &str) -> Option<(usize, usize)> {
    match line.as_bytes().first()? {
        b' ' => Some((1, 1)),
        b'-' => Some((1, 0)),
        b'+' => Some((0, 1)),
        b'\\' if line == "\\ No newline at end of file" => Some((0, 0)),
        _ => None,
    }
}

fn parse_header(line: &str) -> Option<Header<'_>> {
    let (old, remainder) = line.strip_prefix("@@ -")?.split_once(" +")?;
    let (new, context) = remainder.split_once(" @@")?;
    let (old_start, old_count) = range(old)?;
    let (new_start, new_count) = range(new)?;
    Some(Header {
        old_start,
        old_count,
        new_start,
        new_count,
        context,
    })
}

fn range(value: &str) -> Option<(usize, usize)> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    if start.is_empty()
        || count.is_empty()
        || !start.bytes().all(|byte| byte.is_ascii_digit())
        || !count.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((start.parse().ok()?, count.parse().ok()?))
}

//! Canonical text-patch framing shared by target admission and Git execution.

use temper_agent_core::TargetAdmissionStatus;

use super::hunks;

type Result<T> = std::result::Result<T, TargetAdmissionStatus>;

pub(super) fn canonicalize(patch: &str) -> Result<String> {
    if patch.is_empty() || patch.len() > super::MAX_PATCH_BYTES || patch.contains('\0') {
        return Err(TargetAdmissionStatus::MalformedTarget);
    }
    let lines = patch.split_terminator('\n').collect::<Vec<_>>();
    let mut cursor = 0;
    let mut output = String::with_capacity(patch.len());
    while cursor < lines.len() {
        if !section_at(&lines, cursor) {
            return Err(TargetAdmissionStatus::MalformedTarget);
        }
        let header = lines[cursor].strip_prefix(' ').unwrap_or(lines[cursor]);
        push_line(&mut output, header);
        cursor += 1;
        let mut creation_mode = None;
        let mut deletion_mode = None;
        while let Some(line) = lines.get(cursor).filter(|line| metadata(line)) {
            if let Some(mode) = line.strip_prefix("new file mode ") {
                if creation_mode.replace(mode).is_some() {
                    return Err(TargetAdmissionStatus::CompetingTargets);
                }
            } else if let Some(mode) = line.strip_prefix("deleted file mode ") {
                if deletion_mode.replace(mode).is_some() {
                    return Err(TargetAdmissionStatus::CompetingTargets);
                }
            } else if !line.starts_with("index ") {
                push_line(&mut output, line);
            }
            cursor += 1;
        }
        let old = lines[cursor];
        let new = lines[cursor + 1];
        if creation_mode.is_some() && old != "--- /dev/null"
            || deletion_mode.is_some() && new != "+++ /dev/null"
        {
            return Err(TargetAdmissionStatus::CompetingTargets);
        }
        if old == "--- /dev/null" {
            push_line(
                &mut output,
                &format!("new file mode {}", creation_mode.unwrap_or("100644")),
            );
        } else if new == "+++ /dev/null" {
            push_line(
                &mut output,
                &format!("deleted file mode {}", deletion_mode.unwrap_or("100644")),
            );
        }
        push_line(&mut output, old);
        push_line(&mut output, new);
        cursor += 2;
        let first_hunk = cursor;
        while cursor < lines.len() && !section_at(&lines, cursor) {
            let (hunk, next) =
                hunks::canonical_hunk(&lines, cursor, |index| section_at(&lines, index))?;
            output.push_str(&hunk);
            cursor = next;
        }
        if cursor == first_hunk {
            return Err(TargetAdmissionStatus::MalformedTarget);
        }
    }
    Ok(output)
}

fn section_at(lines: &[&str], index: usize) -> bool {
    let Some(line) = lines.get(index) else {
        return false;
    };
    let header = line.strip_prefix(' ').unwrap_or(line);
    let fields = header.split_ascii_whitespace().collect::<Vec<_>>();
    if !header.starts_with("diff --git ") || fields.len() != 4 || fields[..2] != ["diff", "--git"] {
        return false;
    }
    let mut cursor = index + 1;
    while lines.get(cursor).is_some_and(|line| metadata(line)) {
        cursor += 1;
    }
    let Some(old) = lines.get(cursor).and_then(|line| line.strip_prefix("--- ")) else {
        return false;
    };
    let Some(new) = lines
        .get(cursor + 1)
        .and_then(|line| line.strip_prefix("+++ "))
    else {
        return false;
    };
    (old == fields[2] || old == "/dev/null")
        && (new == fields[3] || new == "/dev/null")
        && !(old == "/dev/null" && new == "/dev/null")
}

fn metadata(line: &str) -> bool {
    if let Some(index) = line.strip_prefix("index ") {
        let fields = index.split_ascii_whitespace().collect::<Vec<_>>();
        return matches!(fields.len(), 1 | 2)
            && fields[0].split_once("..").is_some_and(|(old, new)| {
                !old.is_empty()
                    && !new.is_empty()
                    && old.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && new.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            && fields.get(1).is_none_or(|mode| regular_mode(mode));
    }
    [
        "new file mode ",
        "deleted file mode ",
        "old mode ",
        "new mode ",
    ]
    .iter()
    .any(|prefix| line.strip_prefix(prefix).is_some_and(regular_mode))
}

fn regular_mode(mode: &str) -> bool {
    matches!(mode, "100644" | "100755")
}

fn push_line(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
}

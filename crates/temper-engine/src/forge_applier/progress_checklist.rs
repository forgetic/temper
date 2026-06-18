// SPDX-License-Identifier: MPL-2.0

//! Implementation-plan checklist body updates for PR-targeted progress.

use temper_workflow::METADATA_BEGIN;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum ChecklistTick {
    Changed(String),
    AlreadyDone,
    NoChecklist,
    NoMatch,
}

pub(super) fn tick_implementation_plan_phase(body: &str, phase: &str) -> ChecklistTick {
    let target = normalize_phase(phase);
    if target.is_empty() {
        return ChecklistTick::NoMatch;
    }

    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();
    let Some(start) = lines
        .iter()
        .position(|line| line.trim() == "Implementation plan:")
    else {
        return ChecklistTick::NoChecklist;
    };

    let mut saw_checklist = false;
    for index in start + 1..lines.len() {
        let trimmed = lines[index].trim();
        if trimmed.starts_with(METADATA_BEGIN) {
            break;
        }
        if trimmed.is_empty() {
            continue;
        }
        let Some(item) = parse_checklist_item(&lines[index]) else {
            if saw_checklist {
                break;
            }
            return ChecklistTick::NoChecklist;
        };
        saw_checklist = true;
        if normalize_phase(item.label) != target {
            continue;
        }
        if item.checked {
            return ChecklistTick::AlreadyDone;
        }
        lines[index] = format!("{}- [x] {}", item.indent, item.label);
        return ChecklistTick::Changed(join_lines_preserving_final_newline(&lines, body));
    }

    if saw_checklist {
        ChecklistTick::NoMatch
    } else {
        ChecklistTick::NoChecklist
    }
}

pub(super) struct ChecklistItem<'a> {
    pub(super) indent: &'a str,
    pub(super) checked: bool,
    pub(super) label: &'a str,
}

pub(super) fn parse_checklist_item(line: &str) -> Option<ChecklistItem<'_>> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let rest = &line[indent_len..];
    let (checked, label) = if let Some(label) = rest.strip_prefix("- [ ] ") {
        (false, label)
    } else if let Some(label) = rest.strip_prefix("- [x] ") {
        (true, label)
    } else if let Some(label) = rest.strip_prefix("- [X] ") {
        (true, label)
    } else {
        return None;
    };
    Some(ChecklistItem {
        indent,
        checked,
        label,
    })
}

fn normalize_phase(phase: &str) -> String {
    phase.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn join_lines_preserving_final_newline(lines: &[String], original: &str) -> String {
    let mut body = lines.join("\n");
    if original.ends_with('\n') {
        body.push('\n');
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_only_matching_phase_and_preserves_metadata() {
        let body = concat!(
            "Summary: done\n\n",
            "Implementation plan:\n\n",
            "- [ ] Write failing test\n",
            "- [ ] Implement fix\n\n",
            "<!-- temper:workflow\n{}\n-->"
        );

        let ChecklistTick::Changed(updated) = tick_implementation_plan_phase(body, "Implement fix")
        else {
            panic!("expected changed checklist")
        };

        assert!(updated.contains("- [ ] Write failing test\n- [x] Implement fix"));
        assert!(updated.contains("<!-- temper:workflow\n{}\n-->"));
        assert_eq!(
            tick_implementation_plan_phase(&updated, "Implement fix"),
            ChecklistTick::AlreadyDone
        );
    }

    #[test]
    fn ignores_plain_bodies() {
        assert_eq!(
            tick_implementation_plan_phase("Summary: small edit", "small edit"),
            ChecklistTick::NoChecklist
        );
    }
}

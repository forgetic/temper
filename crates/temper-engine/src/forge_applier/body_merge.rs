// SPDX-License-Identifier: MPL-2.0

//! Pull-request body merging for plan-first implementation PRs.

use std::collections::BTreeSet;

use temper_workflow::{
    METADATA_BEGIN, METADATA_END, MetadataError, WorkflowMetadata, parse_metadata_block,
    render_metadata_block,
};

use crate::forge_applier::progress_checklist::parse_checklist_item;

pub(super) fn merge_implementation_pr_body(
    current: &str,
    desired: &str,
) -> Result<Option<String>, MetadataError> {
    let desired_parts = DesiredBodyParts::parse(desired)?;
    let split = split_metadata_block(current)?;
    let metadata = parse_metadata_block(current)?.unwrap_or(desired_parts.metadata);
    let checked = checked_phase_labels(current);

    let mut prose = split.before.trim_end().to_string();
    if prose.trim().is_empty() {
        prose = desired_parts.prose.trim_end().to_string();
    }
    prose = upsert_summary(&prose, desired_parts.summary_line);
    if let Some(plan) = desired_parts.plan_section {
        let plan = apply_checked_phases(plan, &checked);
        prose = upsert_plan_section(&prose, &plan);
    }

    let updated = join_body(&prose, &metadata, split.after);
    if updated == current {
        Ok(None)
    } else {
        Ok(Some(updated))
    }
}

struct DesiredBodyParts<'a> {
    prose: &'a str,
    summary_line: &'a str,
    plan_section: Option<&'a str>,
    metadata: WorkflowMetadata,
}

impl<'a> DesiredBodyParts<'a> {
    fn parse(body: &'a str) -> Result<Self, MetadataError> {
        let split = split_metadata_block(body)?;
        let metadata = parse_metadata_block(body)?.unwrap_or_default();
        let summary_line = split
            .before
            .lines()
            .find(|line| is_summary_line(line))
            .unwrap_or("Summary: (none)");
        Ok(Self {
            prose: split.before,
            summary_line,
            plan_section: plan_section(split.before),
            metadata,
        })
    }
}

struct BodySplit<'a> {
    before: &'a str,
    after: &'a str,
}

fn split_metadata_block(body: &str) -> Result<BodySplit<'_>, MetadataError> {
    let Some(start) = body.find(METADATA_BEGIN) else {
        return Ok(BodySplit {
            before: body,
            after: "",
        });
    };
    let after_begin = start + METADATA_BEGIN.len();
    let Some(relative_end) = body[after_begin..].find(METADATA_END) else {
        return Err(MetadataError::Unterminated);
    };
    let end = after_begin + relative_end + METADATA_END.len();
    Ok(BodySplit {
        before: &body[..start],
        after: &body[end..],
    })
}

fn upsert_summary(prose: &str, summary_line: &str) -> String {
    let mut lines = lines(prose);
    if let Some(index) = lines.iter().position(|line| is_summary_line(line)) {
        lines[index] = summary_line.to_string();
        return trim_joined(lines);
    }

    let index = lines
        .iter()
        .position(|line| line.trim() == "Implementation plan:")
        .unwrap_or(lines.len());
    insert_section(&mut lines, index, &[summary_line.to_string()]);
    trim_joined(lines)
}

fn upsert_plan_section(prose: &str, plan: &str) -> String {
    let mut prose_lines = lines(prose);
    let mut plan_lines = lines(plan);
    if let Some((start, end)) = plan_section_span(&prose_lines) {
        if end < prose_lines.len() && !prose_lines[end].trim().is_empty() {
            plan_lines.push(String::new());
        }
        prose_lines.splice(start..end, plan_lines);
    } else {
        let index = prose_lines.len();
        insert_section(&mut prose_lines, index, &plan_lines);
    }
    trim_joined(prose_lines)
}

fn insert_section(lines: &mut Vec<String>, index: usize, section: &[String]) {
    let mut replacement = Vec::new();
    if index > 0
        && !lines[..index]
            .last()
            .is_none_or(|line| line.trim().is_empty())
    {
        replacement.push(String::new());
    }
    replacement.extend_from_slice(section);
    if index < lines.len() && !lines[index].trim().is_empty() {
        replacement.push(String::new());
    }
    lines.splice(index..index, replacement);
}

fn plan_section(body: &str) -> Option<&str> {
    let lines = body.lines().collect::<Vec<_>>();
    let (start, end) = plan_section_span_str(&lines)?;
    let start_byte = byte_offset_of_line(body, start);
    let end_byte = if end >= lines.len() {
        body.len()
    } else {
        byte_offset_of_line(body, end)
    };
    Some(body[start_byte..end_byte].trim_end())
}

fn plan_section_span(lines: &[String]) -> Option<(usize, usize)> {
    let borrowed = lines.iter().map(String::as_str).collect::<Vec<_>>();
    plan_section_span_str(&borrowed)
}

fn plan_section_span_str(lines: &[&str]) -> Option<(usize, usize)> {
    let start = lines
        .iter()
        .position(|line| line.trim() == "Implementation plan:")?;
    let mut index = start + 1;
    let mut saw_item = false;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if trimmed.is_empty() {
            index += 1;
            continue;
        }
        if parse_checklist_item(lines[index]).is_some() {
            saw_item = true;
            index += 1;
            continue;
        }
        break;
    }
    if saw_item {
        Some((start, index))
    } else {
        Some((start, start + 1))
    }
}

fn byte_offset_of_line(body: &str, target: usize) -> usize {
    if target == 0 {
        return 0;
    }
    let mut line = 0;
    for (index, byte) in body.bytes().enumerate() {
        if byte == b'\n' {
            line += 1;
            if line == target {
                return index + 1;
            }
        }
    }
    body.len()
}

fn checked_phase_labels(body: &str) -> BTreeSet<String> {
    let mut labels = BTreeSet::new();
    let Some(section) = plan_section(body) else {
        return labels;
    };
    for line in section.lines() {
        if let Some(item) = parse_checklist_item(line)
            && item.checked
        {
            labels.insert(normalize_phase(item.label));
        }
    }
    labels
}

fn apply_checked_phases(plan: &str, checked: &BTreeSet<String>) -> String {
    plan.lines()
        .map(|line| {
            let Some(item) = parse_checklist_item(line) else {
                return line.to_string();
            };
            if checked.contains(&normalize_phase(item.label)) {
                format!("{}- [x] {}", item.indent, item.label)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn lines(body: &str) -> Vec<String> {
    body.lines().map(str::to_string).collect()
}

fn trim_joined(lines: Vec<String>) -> String {
    lines.join("\n").trim_end().to_string()
}

fn join_body(prose: &str, metadata: &WorkflowMetadata, after: &str) -> String {
    let block = render_metadata_block(metadata);
    if prose.trim().is_empty() {
        format!("{block}{after}")
    } else {
        format!("{}\n\n{block}{after}", prose.trim_end())
    }
}

fn is_summary_line(line: &str) -> bool {
    line.trim_start().starts_with("Summary:")
}

fn normalize_phase(phase: &str) -> String {
    phase.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_workflow::{ArtifactKindId, WorkflowMetadata, render_metadata_block};

    fn metadata() -> WorkflowMetadata {
        WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            correlation_key: Some("pr-for-code-7".to_string()),
            ..WorkflowMetadata::default()
        }
    }

    #[test]
    fn preserves_checked_items_when_summary_changes() {
        let current = format!(
            "Intro.\n\nSummary: planned\n\nImplementation plan:\n\n- [ ] Test\n- [x] Build\n\n{}",
            render_metadata_block(&metadata())
        );
        let desired = format!(
            "Intro.\n\nSummary: final\n\nImplementation plan:\n\n- [ ] Test\n- [ ] Build\n\n{}",
            render_metadata_block(&metadata())
        );

        let updated = merge_implementation_pr_body(&current, &desired)
            .expect("merge succeeds")
            .expect("body changes");

        assert!(updated.contains("Summary: final"));
        assert!(updated.contains("- [ ] Test\n- [x] Build"));
        assert_eq!(parse_metadata_block(&updated).unwrap(), Some(metadata()));
    }

    #[test]
    fn keeps_human_notes_around_managed_sections() {
        let current = format!(
            "Intro.\n\nHuman note.\n\nSummary: planned\n\nImplementation plan:\n\n- [x] Test\n- [ ] Build\n\nMore notes.\n\n{}",
            render_metadata_block(&metadata())
        );
        let desired = format!(
            "Intro.\n\nSummary: final\n\nImplementation plan:\n\n- [ ] Test\n- [ ] Build\n\n{}",
            render_metadata_block(&metadata())
        );

        let updated = merge_implementation_pr_body(&current, &desired)
            .expect("merge succeeds")
            .expect("body changes");

        assert!(updated.contains("Human note."));
        assert!(updated.contains("More notes."));
        assert!(updated.contains("- [x] Test\n- [ ] Build"));
    }
}

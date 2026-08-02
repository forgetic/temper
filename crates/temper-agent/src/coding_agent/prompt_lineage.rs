//! Deterministic, decision-dense projection for unusually large lineage bodies.

use temper_protocol_agent::ArtifactSnapshot;

const CURATION_THRESHOLD_BYTES: usize = 6_000;
const CURATED_TOTAL_BYTES: usize = 4_500;
const MIN_ARTIFACT_BYTES: usize = 700;

#[derive(Clone, Copy)]
struct Section<'a> {
    text: &'a str,
    priority: u8,
    order: usize,
}

/// Returns curated bodies only when mandatory lineage is large enough to make
/// repeated planning prose materially expensive. Small bundles remain byte-for-byte intact.
pub(super) fn curate_lineage(lineage: &[ArtifactSnapshot]) -> Option<Vec<String>> {
    let original_bytes = lineage
        .iter()
        .map(|snapshot| snapshot.body.len())
        .sum::<usize>();
    if original_bytes <= CURATION_THRESHOLD_BYTES {
        return None;
    }

    let artifact_budget = (CURATED_TOTAL_BYTES / lineage.len().max(1)).max(MIN_ARTIFACT_BYTES);
    Some(
        lineage
            .iter()
            .map(|snapshot| curate_body(&snapshot.body, artifact_budget))
            .collect(),
    )
}

fn curate_body(body: &str, budget: usize) -> String {
    if body.len() <= budget {
        return body.to_string();
    }
    let sections = markdown_sections(body);
    let mut ranked = sections.clone();
    ranked.sort_by_key(|section| (std::cmp::Reverse(section.priority), section.order));

    let mut selected = Vec::new();
    let mut used = 0;
    for section in ranked {
        if section.priority == 0 || section.text.len() > budget.saturating_sub(used) {
            continue;
        }
        selected.push(section);
        used += section.text.len();
    }
    if selected.is_empty() {
        return bounded_prefix(body, budget);
    }
    selected.sort_by_key(|section| section.order);
    selected
        .into_iter()
        .map(|section| section.text.trim_end())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn markdown_sections(body: &str) -> Vec<Section<'_>> {
    let mut starts = body
        .match_indices('\n')
        .filter_map(|(index, _)| {
            let start = index + 1;
            body[start..].starts_with('#').then_some(start)
        })
        .collect::<Vec<_>>();
    if body.starts_with('#') {
        starts.insert(0, 0);
    }
    starts.sort_unstable();
    starts.dedup();

    let mut sections = Vec::new();
    if starts.first().copied().unwrap_or(body.len()) > 0 {
        let end = starts.first().copied().unwrap_or(body.len());
        if !body[..end].trim().is_empty() {
            sections.push(Section {
                text: &body[..end],
                priority: 1,
                order: 0,
            });
        }
    }
    for (position, start) in starts.iter().copied().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(body.len());
        let text = &body[start..end];
        sections.push(Section {
            text,
            priority: heading_priority(text.lines().next().unwrap_or_default()),
            order: position + 1,
        });
    }
    sections
}

fn heading_priority(heading: &str) -> u8 {
    let normalized = heading.trim_start_matches('#').trim().to_ascii_lowercase();
    if [
        "constraint",
        "architecture",
        "risk",
        "decision",
        "compatibility",
        "non-goal",
        "evidence",
    ]
    .iter()
    .any(|term| normalized.contains(term))
    {
        3
    } else if [
        "requirement",
        "public api",
        "parsing",
        "evaluation",
        "reporting",
        "design",
    ]
    .iter()
    .any(|term| normalized.contains(term))
    {
        2
    } else if [
        "objective",
        "problem",
        "desired behavior",
        "acceptance",
        "work breakdown",
        "validation strategy",
        "test mapping",
    ]
    .iter()
    .any(|term| normalized.contains(term))
    {
        0
    } else {
        1
    }
}

fn bounded_prefix(body: &str, budget: usize) -> String {
    let mut end = budget.min(body.len());
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[remaining lineage body omitted]",
        body[..end].trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_lineage_remains_authored_and_large_lineage_keeps_decisions() {
        let small = snapshots("## Objective\nsmall");
        assert!(curate_lineage(&small).is_none());

        let repeated = "x".repeat(1_400);
        let body = format!(
            "## Objective\n{repeated}\n## Constraints\nKEEP_CONSTRAINT\n## Acceptance\n{repeated}\n## Architecture\nKEEP_ARCHITECTURE\n## Test mapping\n{repeated}\n## Non-goals\nKEEP_NON_GOALS"
        );
        let curated = curate_lineage(&snapshots(&body)).expect("large lineage is curated");
        assert!(curated.iter().all(|body| body.contains("KEEP_CONSTRAINT")));
        assert!(
            curated
                .iter()
                .all(|body| body.contains("KEEP_ARCHITECTURE"))
        );
        assert!(curated.iter().all(|body| body.contains("KEEP_NON_GOALS")));
        assert!(curated.iter().all(|body| !body.contains("## Acceptance")));
        assert!(curated.iter().map(String::len).sum::<usize>() < body.len() * 2);
    }

    fn snapshots(body: &str) -> Vec<ArtifactSnapshot> {
        (1..=2)
            .map(|number| {
                serde_json::from_value(serde_json::json!({
                    "artifact": {
                        "repository": {"id":"repo-1", "path":"acme/service"},
                        "artifact_type":"issue",
                        "number":number
                    },
                    "title":format!("Ancestor {number}"),
                    "body":body,
                    "state":"open"
                }))
                .unwrap()
            })
            .collect()
    }
}

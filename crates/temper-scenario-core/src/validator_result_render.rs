// SPDX-License-Identifier: MPL-2.0

use std::fmt::Write as _;

use crate::{
    FollowUpIssueIntent, RelatedPullRequest, ScenarioPromotionIntent, ValidationAssertion,
    ValidatorResult,
};

impl ValidatorResult {
    /// Render a deterministic Markdown report from the structured result.
    pub fn render_markdown(&self) -> String {
        let mut output = String::new();
        let _ = writeln!(output, "# Validation report");
        let _ = writeln!(output);
        let _ = writeln!(
            output,
            "- Target: {} {} in {}",
            self.target.kind, self.target.reference, self.target.repo
        );
        if let Some(fingerprint) = self.target.state_fingerprint.as_deref() {
            let _ = writeln!(output, "- State fingerprint: `{fingerprint}`");
        }
        if let Some(trigger_reason) = self.target.trigger_reason.as_deref() {
            let _ = writeln!(output, "- Trigger: {trigger_reason}");
        }
        let _ = writeln!(output, "- Verdict: {}", self.verdict);
        for (label, value) in [
            ("Feature", self.feature.as_deref()),
            ("Plan", self.plan.as_deref()),
            ("Mapped scenario", self.scenario_name.as_deref()),
            ("Scenario path", self.scenario_path.as_deref()),
            ("Source branch", self.source_branch.as_deref()),
            ("Exact head SHA", self.exact_head_sha.as_deref()),
            (
                "Resolved content digest",
                self.resolved_content_digest.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                let _ = writeln!(output, "- {label}: `{value}`");
            }
        }
        if let Some(binary) = &self.standalone_binary {
            let _ = writeln!(
                output,
                "- Standalone binary: `{}` sha256={} size_bytes={}",
                binary.path, binary.sha256, binary.size_bytes
            );
        }
        if let Some(duration_ms) = self.duration_ms {
            let _ = writeln!(output, "- Duration: {duration_ms}ms");
        }
        for path in &self.retained_paths {
            let _ = writeln!(output, "- Retained artifact: `{path}`");
        }
        let _ = writeln!(output);

        let _ = writeln!(output, "## Related PRs");
        let _ = writeln!(output);
        if self.related_prs.is_empty() {
            let _ = writeln!(output, "- None recorded.");
        } else {
            for pull_request in &self.related_prs {
                write_related_pr(&mut output, pull_request);
            }
        }
        let _ = writeln!(output);

        let _ = writeln!(output, "## Validated claims");
        let _ = writeln!(output);
        write_assertions(&mut output, &self.validated_claims);
        let _ = writeln!(output);

        let _ = writeln!(output, "## Acceptance criteria");
        let _ = writeln!(output);
        write_assertions(&mut output, &self.acceptance_criteria);
        let _ = writeln!(output);

        let _ = writeln!(output, "## Evidence");
        let _ = writeln!(output);
        if self.evidence.is_empty() {
            let _ = writeln!(output, "- None recorded.");
        } else {
            for (index, entry) in self.evidence.iter().enumerate() {
                let _ = writeln!(
                    output,
                    "{}. **{}** `{}` — {}",
                    index + 1,
                    entry.kind,
                    entry.id,
                    entry.summary
                );
                if let Some(details) = entry.details.as_deref() {
                    write_bullet(&mut output, details, "   ");
                }
                if let Some(uri) = entry.uri.as_deref() {
                    write_bullet(&mut output, &format!("uri: {uri}"), "   ");
                }
                if let Some(path) = entry.artifact_path.as_deref() {
                    write_bullet(&mut output, &format!("artifact: `{path}`"), "   ");
                }
            }
        }
        let _ = writeln!(output);

        let _ = writeln!(output, "## Limitations");
        let _ = writeln!(output);
        if self.limitations.is_empty() {
            let _ = writeln!(output, "- None recorded.");
        } else {
            for limitation in &self.limitations {
                write_bullet(&mut output, limitation, "");
            }
        }
        let _ = writeln!(output);

        let _ = writeln!(output, "## Follow-up intent");
        let _ = writeln!(output);
        if let Some(intent) = self.follow_up_intent.as_deref() {
            write_bullet(&mut output, intent, "");
        }
        match &self.follow_up_issue {
            Some(intent) => write_follow_up(&mut output, intent),
            None if self.follow_up_intent.is_none() => {
                let _ = writeln!(output, "- None recorded.");
            }
            None => {}
        }
        let _ = writeln!(output);

        let _ = writeln!(output, "## Scenario promotion intent");
        let _ = writeln!(output);
        match &self.scenario_promotion {
            Some(intent) => write_scenario_promotion(&mut output, intent),
            None => {
                let _ = writeln!(output, "- None recorded.");
            }
        }

        output
    }
}

fn write_related_pr(output: &mut String, pull_request: &RelatedPullRequest) {
    let _ = write!(output, "- #{}", pull_request.pr_number);
    if let Some(source_issue) = pull_request.source_issue {
        let _ = write!(output, " (source issue #{source_issue})");
    }
    if let Some(sha) = pull_request.merged_main_sha.as_deref() {
        let _ = write!(output, ", merged `{sha}`");
    }
    if let Some(sha) = pull_request.observed_main_sha.as_deref() {
        let _ = write!(output, ", observed `{sha}`");
    }
    let _ = writeln!(output);
}

fn write_assertions(output: &mut String, assertions: &[ValidationAssertion]) {
    if assertions.is_empty() {
        let _ = writeln!(output, "- None recorded.");
        return;
    }

    for assertion in assertions {
        if assertion.required {
            let _ = writeln!(output, "- [{}] {}", assertion.status, assertion.description);
        } else {
            let _ = writeln!(
                output,
                "- [{}, optional] {}",
                assertion.status, assertion.description
            );
        }
        for evidence_ref in &assertion.evidence_refs {
            write_bullet(output, &format!("evidence: `{evidence_ref}`"), "  ");
        }
    }
}

fn write_follow_up(output: &mut String, intent: &FollowUpIssueIntent) {
    let _ = writeln!(output, "- Title: {}", intent.title);
    if intent.labels.is_empty() {
        let _ = writeln!(output, "- Labels: none");
    } else {
        let _ = writeln!(output, "- Labels: {}", intent.labels.join(", "));
    }
    if !intent.relation_hints.is_empty() {
        let _ = writeln!(
            output,
            "- Relation hints: {}",
            intent.relation_hints.join(", ")
        );
    }
    let _ = writeln!(output, "- Body:");
    write_blockquote(output, &intent.body);
}

fn write_scenario_promotion(output: &mut String, intent: &ScenarioPromotionIntent) {
    let _ = writeln!(output, "- Scenario: {}", intent.scenario_name);
    let _ = writeln!(output, "- Intent: {}", intent.intent);
    let _ = writeln!(
        output,
        "- Proposed effects: issue={}, pr={}",
        intent.propose_issue, intent.propose_pr
    );
    if !intent.source_evidence.is_empty() {
        let _ = writeln!(output, "- Source evidence:");
        for evidence in &intent.source_evidence {
            write_bullet(output, evidence, "  ");
        }
    }
    if let Some(notes) = intent.fixture_notes.as_deref() {
        let _ = writeln!(output, "- Fixture notes:");
        write_blockquote(output, notes);
    }
}

fn write_bullet(output: &mut String, text: &str, indent: &str) {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        let _ = writeln!(output, "{indent}-");
        return;
    };
    let _ = writeln!(output, "{indent}- {first}");
    for line in lines {
        let _ = writeln!(output, "{indent}  {line}");
    }
}

fn write_blockquote(output: &mut String, text: &str) {
    if text.is_empty() {
        let _ = writeln!(output, "> ");
        return;
    }
    for line in text.lines() {
        let _ = writeln!(output, "> {line}");
    }
}

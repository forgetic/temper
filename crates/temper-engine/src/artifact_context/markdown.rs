// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use temper_protocol_worker::ArtifactType;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct MarkdownReference {
    pub path: Option<String>,
    pub artifact_type: ArtifactTypeKey,
    pub number: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum ArtifactTypeKey {
    Issue,
    PullRequest,
}

impl From<ArtifactTypeKey> for ArtifactType {
    fn from(value: ArtifactTypeKey) -> Self {
        match value {
            ArtifactTypeKey::Issue => Self::Issue,
            ArtifactTypeKey::PullRequest => Self::PullRequest,
        }
    }
}

/// Extracts forge references while deliberately ignoring code and Temper's
/// machine-authored HTML metadata comments.
pub(super) fn references(body: &str, forge_url: &str) -> Vec<MarkdownReference> {
    let visible = visible_markdown(body);
    let mut found = BTreeSet::new();
    for token in visible.split_whitespace() {
        let token = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '(' | ')' | '[' | ']' | '<' | '>' | ',' | '.' | ';' | ':' | '"' | '\''
            )
        });
        if let Some(reference) =
            url_reference(token, forge_url).or_else(|| shorthand_reference(token))
        {
            found.insert(reference);
        }
    }
    found.into_iter().collect()
}

fn shorthand_reference(token: &str) -> Option<MarkdownReference> {
    let (prefix, raw_number) = token.rsplit_once('#')?;
    let number = raw_number.parse().ok()?;
    if prefix.is_empty() {
        return Some(MarkdownReference {
            path: None,
            artifact_type: ArtifactTypeKey::Issue,
            number,
        });
    }
    let (owner, name) = prefix.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some(MarkdownReference {
        path: Some(format!("{owner}/{name}")),
        artifact_type: ArtifactTypeKey::Issue,
        number,
    })
}

fn url_reference(token: &str, forge_url: &str) -> Option<MarkdownReference> {
    if forge_url.is_empty() {
        return None;
    }
    let suffix = token.strip_prefix(forge_url)?.trim_start_matches('/');
    let parts: Vec<&str> = suffix.split('/').collect();
    if parts.len() < 4 {
        return None;
    }
    let artifact_type = match parts[2] {
        "issues" => ArtifactTypeKey::Issue,
        "pulls" | "pull" => ArtifactTypeKey::PullRequest,
        _ => return None,
    };
    let number = parts[3]
        .trim_end_matches(|ch: char| !ch.is_ascii_digit())
        .parse()
        .ok()?;
    Some(MarkdownReference {
        path: Some(format!("{}/{}", parts[0], parts[1])),
        artifact_type,
        number,
    })
}

fn visible_markdown(body: &str) -> String {
    let mut output = String::with_capacity(body.len());
    let mut fenced = false;
    let mut comment = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if !comment && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            fenced = !fenced;
            output.push('\n');
            continue;
        }
        if fenced {
            output.push('\n');
            continue;
        }
        let mut inline = false;
        let chars: Vec<char> = line.chars().collect();
        let mut index = 0;
        while index < chars.len() {
            if !inline && chars[index..].starts_with(&['<', '!', '-', '-']) {
                comment = true;
            }
            if comment && chars[index..].starts_with(&['-', '-', '>']) {
                comment = false;
                index += 3;
                continue;
            }
            if !comment && chars[index] == '`' {
                inline = !inline;
                output.push(' ');
            } else if !comment && !inline {
                output.push(chars[index]);
            } else {
                output.push(' ');
            }
            index += 1;
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_code_and_metadata() {
        let body = "See #1 and ai/other#2. `#3`\n```\n#4\n```\n<!-- temper:workflow\n{\"parents\":[5]}\n-->";
        let refs = references(body, "https://forge.example");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].number, 1);
        assert_eq!(refs[1].path.as_deref(), Some("ai/other"));
    }

    #[test]
    fn accepts_only_urls_at_configured_forge() {
        let refs = references(
            "https://forge.example/ai/temper/pulls/7 https://other/ai/temper/issues/8",
            "https://forge.example",
        );
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].artifact_type, ArtifactTypeKey::PullRequest);
    }
}

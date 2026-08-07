// SPDX-License-Identifier: MPL-2.0

//! Conservative parsing of the shell command-list subset used for discovery.

#[derive(Clone, Copy)]
pub(super) enum ShellDiscoveryError {
    MissingCommand,
    MissingArguments,
    QuotingOrEscaping,
    UnsupportedSyntax,
}

impl ShellDiscoveryError {
    pub(super) fn availability_message(self) -> &'static str {
        match self {
            Self::MissingArguments => {
                "shell discovery classification is unavailable because a discovery command is missing arguments"
            }
            Self::QuotingOrEscaping => {
                "shell discovery classification is unavailable because quoting or escaping makes command-list boundaries ambiguous"
            }
            Self::MissingCommand | Self::UnsupportedSyntax => {
                "shell discovery classification is unavailable because the command uses unsupported or ambiguous shell syntax"
            }
        }
    }
}

/// Parses the deliberately small, reviewable subset of a shell command list
/// used for discovery classification. It accepts unquoted words separated into
/// commands by `&&`, `||`, `;`, or newlines. Shell quoting, escaping,
/// expansions, pipelines, redirects, grouping, globbing, comments, and single
/// `&` are rejected rather than approximated.
pub(super) fn classify_shell_discovery(
    command: &str,
    prefixes: &[Vec<String>],
) -> Result<u64, ShellDiscoveryError> {
    let segments = parse_common_command_list(command)?;
    let mut discovery_segments = 0_u64;
    for segment in segments {
        match classify_shell_segment(&segment, prefixes) {
            ShellSegmentClassification::Discovery => {
                discovery_segments = discovery_segments.saturating_add(1);
            }
            ShellSegmentClassification::NonDiscovery => {}
            ShellSegmentClassification::MissingArguments => {
                return Err(ShellDiscoveryError::MissingArguments);
            }
        }
    }
    Ok(discovery_segments)
}

#[derive(Clone, Copy)]
enum ShellSegmentClassification {
    Discovery,
    NonDiscovery,
    MissingArguments,
}

fn classify_shell_segment(
    segment: &[String],
    prefixes: &[Vec<String>],
) -> ShellSegmentClassification {
    let mut missing_arguments = false;
    for prefix in prefixes {
        if segment.starts_with(prefix) {
            if segment.len() > prefix.len() {
                return ShellSegmentClassification::Discovery;
            }
            missing_arguments = true;
        }
    }
    if missing_arguments {
        ShellSegmentClassification::MissingArguments
    } else {
        ShellSegmentClassification::NonDiscovery
    }
}

fn parse_common_command_list(command: &str) -> Result<Vec<Vec<String>>, ShellDiscoveryError> {
    let mut characters = command.chars().peekable();
    let mut segments = Vec::new();
    let mut segment = Vec::new();
    let mut word = String::new();
    let mut requires_segment = false;

    while let Some(character) = characters.next() {
        match character {
            '\n' => {
                push_shell_word(&mut word, &mut segment, &mut requires_segment);
                if !segment.is_empty() {
                    segments.push(std::mem::take(&mut segment));
                } else if requires_segment {
                    return Err(ShellDiscoveryError::MissingCommand);
                }
            }
            character if character.is_whitespace() => {
                push_shell_word(&mut word, &mut segment, &mut requires_segment);
            }
            ';' => {
                push_shell_word(&mut word, &mut segment, &mut requires_segment);
                if segment.is_empty() {
                    return Err(ShellDiscoveryError::MissingCommand);
                }
                segments.push(std::mem::take(&mut segment));
                requires_segment = false;
            }
            '&' => {
                if characters.next() != Some('&') {
                    return Err(ShellDiscoveryError::UnsupportedSyntax);
                }
                push_shell_word(&mut word, &mut segment, &mut requires_segment);
                if segment.is_empty() {
                    return Err(ShellDiscoveryError::MissingCommand);
                }
                segments.push(std::mem::take(&mut segment));
                requires_segment = true;
            }
            '|' => {
                if characters.next() != Some('|') {
                    return Err(ShellDiscoveryError::UnsupportedSyntax);
                }
                push_shell_word(&mut word, &mut segment, &mut requires_segment);
                if segment.is_empty() {
                    return Err(ShellDiscoveryError::MissingCommand);
                }
                segments.push(std::mem::take(&mut segment));
                requires_segment = true;
            }
            '\'' | '"' | '\\' => return Err(ShellDiscoveryError::QuotingOrEscaping),
            '$' | '`' | '(' | ')' | '<' | '>' | '*' | '?' | '[' | ']' | '{' | '}' | '#' | '!' => {
                return Err(ShellDiscoveryError::UnsupportedSyntax);
            }
            character => word.push(character),
        }
    }

    push_shell_word(&mut word, &mut segment, &mut requires_segment);
    if !segment.is_empty() {
        segments.push(segment);
    } else if requires_segment || segments.is_empty() {
        return Err(ShellDiscoveryError::MissingCommand);
    }
    Ok(segments)
}

fn push_shell_word(word: &mut String, segment: &mut Vec<String>, requires_segment: &mut bool) {
    if !word.is_empty() {
        segment.push(std::mem::take(word));
        *requires_segment = false;
    }
}

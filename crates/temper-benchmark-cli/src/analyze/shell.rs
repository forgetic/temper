// SPDX-License-Identifier: MPL-2.0

//! Conservative parsing of the shell command-list subset used for discovery.

use serde_json::Value;

/// Shell evidence retains its producer-declared boundaries. In particular,
/// argv is never flattened into text that could turn an argument into syntax.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CapturedShellCommand {
    Structured(String),
    Argv(Vec<String>),
    LegacyBacktick(String),
}

impl CapturedShellCommand {
    pub(super) fn matches_validation_prefixes(&self, prefixes: &[String]) -> bool {
        match self {
            Self::Structured(command) | Self::LegacyBacktick(command) => prefixes
                .iter()
                .any(|prefix| command.trim_start().starts_with(prefix)),
            Self::Argv(argv) => prefixes.iter().any(|prefix| {
                let prefix = prefix.split_ascii_whitespace().collect::<Vec<_>>();
                !prefix.is_empty()
                    && argv.len() >= prefix.len()
                    && argv
                        .iter()
                        .zip(prefix)
                        .all(|(argument, prefix)| argument == prefix)
            }),
        }
    }
}

/// Extracts only representations that can be distinguished without guessing.
/// Structured command strings and argv remain typed; legacy human previews are
/// accepted only when they retain both backticks and no truncation/redaction
/// marker.
pub(super) fn captured_shell_command(text: &str) -> Option<CapturedShellCommand> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return structured_shell_command(value);
    }

    let trimmed = text.trim();
    let command = trimmed
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))?;
    if command.trim().is_empty() || command.ends_with('…') || contains_redaction_marker(command) {
        return None;
    }
    Some(CapturedShellCommand::LegacyBacktick(command.to_string()))
}

fn structured_shell_command(value: Value) -> Option<CapturedShellCommand> {
    match value {
        Value::String(command) => structured_command(command),
        Value::Array(argv) => structured_argv(&argv),
        Value::Object(object) => match (object.get("command"), object.get("argv")) {
            (Some(Value::String(command)), None) => structured_command(command.clone()),
            (None, Some(Value::Array(argv))) => structured_argv(argv),
            _ => None,
        },
        _ => None,
    }
}

fn structured_command(command: String) -> Option<CapturedShellCommand> {
    (!command.trim().is_empty() && !contains_redaction_marker(&command))
        .then_some(CapturedShellCommand::Structured(command))
}

fn structured_argv(values: &[Value]) -> Option<CapturedShellCommand> {
    let argv = values
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?;
    (!argv.is_empty()
        && argv
            .iter()
            .all(|argument| !contains_redaction_marker(argument)))
    .then(|| CapturedShellCommand::Argv(argv.into_iter().map(str::to_string).collect()))
}

fn contains_redaction_marker(value: &str) -> bool {
    value.contains("[REDACTED]") || value.contains("<redacted>")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ShellDiscoveryError {
    MissingCommand,
    MissingArguments,
    IncompleteQuoteOrEscape,
    UnsupportedSyntax,
}

impl ShellDiscoveryError {
    pub(super) fn availability_message(self) -> &'static str {
        match self {
            Self::MissingArguments => {
                "shell discovery classification is unavailable because a discovery command is missing arguments"
            }
            Self::IncompleteQuoteOrEscape => {
                "shell discovery classification is unavailable because the command has an incomplete quote or escape"
            }
            Self::MissingCommand | Self::UnsupportedSyntax => {
                "shell discovery classification is unavailable because the command uses unsupported or ambiguous shell syntax"
            }
        }
    }
}

/// Parses the deliberately small, reviewable subset of a shell command list
/// used for discovery classification. Structured argv is classified as one
/// exact command. Command strings support sound single/double quoting,
/// escaping, and lists separated by `&&`, `||`, `;`, or newlines. Expansions,
/// pipelines, redirects, grouping, globbing, comments, assignments, and single
/// `&` are rejected rather than approximated.
pub(super) fn classify_shell_discovery(
    command: &CapturedShellCommand,
    prefixes: &[Vec<String>],
) -> Result<u64, ShellDiscoveryError> {
    let segments = match command {
        CapturedShellCommand::Structured(command)
        | CapturedShellCommand::LegacyBacktick(command) => parse_common_command_list(command)?,
        CapturedShellCommand::Argv(argv) => {
            if argv.first().is_none_or(String::is_empty) {
                return Err(ShellDiscoveryError::MissingCommand);
            }
            vec![argv.clone()]
        }
    };
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
    let longest_prefix = prefixes
        .iter()
        .filter(|prefix| segment.starts_with(prefix))
        .max_by_key(|prefix| prefix.len());
    let Some(prefix) = longest_prefix else {
        return ShellSegmentClassification::NonDiscovery;
    };
    if segment.len() > prefix.len() {
        ShellSegmentClassification::Discovery
    } else {
        ShellSegmentClassification::MissingArguments
    }
}

fn parse_common_command_list(command: &str) -> Result<Vec<Vec<String>>, ShellDiscoveryError> {
    let mut characters = command.chars().peekable();
    let mut segments = Vec::new();
    let mut segment = Vec::new();
    let mut word = String::new();
    let mut word_started = false;
    let mut requires_segment = false;
    let mut quote = Quote::Unquoted;

    while let Some(character) = characters.next() {
        match quote {
            Quote::Single => match character {
                '\'' => quote = Quote::Unquoted,
                '\0' => return Err(ShellDiscoveryError::UnsupportedSyntax),
                character => word.push(character),
            },
            Quote::Double => match character {
                '"' => quote = Quote::Unquoted,
                '$' | '`' => return Err(ShellDiscoveryError::UnsupportedSyntax),
                '\\' => {
                    let escaped = characters
                        .next()
                        .ok_or(ShellDiscoveryError::IncompleteQuoteOrEscape)?;
                    match escaped {
                        '\n' | '\0' => return Err(ShellDiscoveryError::UnsupportedSyntax),
                        '$' | '`' | '"' | '\\' => word.push(escaped),
                        character => {
                            word.push('\\');
                            word.push(character);
                        }
                    }
                }
                '\0' => return Err(ShellDiscoveryError::UnsupportedSyntax),
                character => word.push(character),
            },
            Quote::Unquoted => match character {
                ' ' | '\t' => push_shell_word(
                    &mut word,
                    &mut word_started,
                    &mut segment,
                    &mut requires_segment,
                ),
                '\n' => {
                    push_shell_word(
                        &mut word,
                        &mut word_started,
                        &mut segment,
                        &mut requires_segment,
                    );
                    if !segment.is_empty() {
                        finish_shell_segment(&mut segment, &mut segments)?;
                    } else if requires_segment {
                        return Err(ShellDiscoveryError::MissingCommand);
                    }
                }
                ';' => {
                    push_shell_word(
                        &mut word,
                        &mut word_started,
                        &mut segment,
                        &mut requires_segment,
                    );
                    finish_shell_segment(&mut segment, &mut segments)?;
                    requires_segment = false;
                }
                '&' | '|' => {
                    if characters.peek() != Some(&character) {
                        return Err(ShellDiscoveryError::UnsupportedSyntax);
                    }
                    characters.next();
                    push_shell_word(
                        &mut word,
                        &mut word_started,
                        &mut segment,
                        &mut requires_segment,
                    );
                    finish_shell_segment(&mut segment, &mut segments)?;
                    requires_segment = true;
                }
                '\'' => {
                    quote = Quote::Single;
                    word_started = true;
                }
                '"' => {
                    quote = Quote::Double;
                    word_started = true;
                }
                '\\' => {
                    let escaped = characters
                        .next()
                        .ok_or(ShellDiscoveryError::IncompleteQuoteOrEscape)?;
                    if matches!(escaped, '\n' | '\0') {
                        return Err(ShellDiscoveryError::UnsupportedSyntax);
                    }
                    word_started = true;
                    word.push(escaped);
                }
                '$' | '`' | '(' | ')' | '<' | '>' | '*' | '?' | '[' | ']' | '{' | '}' | '#'
                | '!' | '~' | '\0' => return Err(ShellDiscoveryError::UnsupportedSyntax),
                character => {
                    word_started = true;
                    word.push(character);
                }
            },
        }
    }

    if quote != Quote::Unquoted {
        return Err(ShellDiscoveryError::IncompleteQuoteOrEscape);
    }
    push_shell_word(
        &mut word,
        &mut word_started,
        &mut segment,
        &mut requires_segment,
    );
    if !segment.is_empty() {
        finish_shell_segment(&mut segment, &mut segments)?;
    } else if requires_segment || segments.is_empty() {
        return Err(ShellDiscoveryError::MissingCommand);
    }
    Ok(segments)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Quote {
    Unquoted,
    Single,
    Double,
}

fn push_shell_word(
    word: &mut String,
    word_started: &mut bool,
    segment: &mut Vec<String>,
    requires_segment: &mut bool,
) {
    if *word_started {
        segment.push(std::mem::take(word));
        *word_started = false;
        *requires_segment = false;
    }
}

fn finish_shell_segment(
    segment: &mut Vec<String>,
    segments: &mut Vec<Vec<String>>,
) -> Result<(), ShellDiscoveryError> {
    if segment.is_empty() {
        return Err(ShellDiscoveryError::MissingCommand);
    }
    if is_reserved_word(&segment[0]) || is_assignment_word(&segment[0]) {
        return Err(ShellDiscoveryError::UnsupportedSyntax);
    }
    segments.push(std::mem::take(segment));
    Ok(())
}

fn is_assignment_word(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn is_reserved_word(word: &str) -> bool {
    matches!(
        word,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "while"
            | "until"
            | "for"
            | "select"
            | "in"
            | "function"
            | "time"
            | "coproc"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes() -> Vec<Vec<String>> {
        vec![vec!["git".into(), "grep".into()], vec!["rg".into()]]
    }

    fn script(command: &str) -> CapturedShellCommand {
        CapturedShellCommand::Structured(command.to_string())
    }

    #[test]
    fn quoted_and_escaped_words_preserve_list_boundaries() {
        let command = script(
            "git grep 'a && b' && rg \"c; d || e\"; git status\nrg two\\ words && rg escaped\\;meta",
        );
        assert_eq!(classify_shell_discovery(&command, &prefixes()), Ok(4));
    }

    #[test]
    fn argv_is_one_exact_command_even_when_an_argument_looks_like_shell() {
        let command = CapturedShellCommand::Argv(vec![
            "git".into(),
            "grep".into(),
            "Widget && rg Alias".into(),
        ]);
        assert_eq!(classify_shell_discovery(&command, &prefixes()), Ok(1));
    }

    #[test]
    fn unsupported_or_incomplete_shell_forms_are_rejected() {
        for command in [
            "git grep $(whoami)",
            "git grep $HOME",
            "git grep Widget | rg Alias",
            "(git grep Widget)",
            "git grep Widget > result",
            "git grep Widget &",
            "git grep 'Widget",
            "git grep Widget\\",
            "git grep *",
            "if git grep Widget; then true; fi",
            "PATTERN=Widget git grep Widget",
        ] {
            assert!(
                classify_shell_discovery(&script(command), &prefixes()).is_err(),
                "{command}"
            );
        }
    }

    #[test]
    fn extraction_keeps_representations_typed_and_rejects_lossy_previews() {
        assert_eq!(
            captured_shell_command(r#"{"command":"git grep Widget"}"#),
            Some(script("git grep Widget"))
        );
        assert_eq!(
            captured_shell_command(r#"{"argv":["git","grep","two words"]}"#),
            Some(CapturedShellCommand::Argv(vec![
                "git".into(),
                "grep".into(),
                "two words".into(),
            ]))
        );
        assert_eq!(
            captured_shell_command("`git grep Widget`"),
            Some(CapturedShellCommand::LegacyBacktick(
                "git grep Widget".into()
            ))
        );
        for unavailable in [
            "git grep Widget",
            "`git grep Widget…`",
            "`git grep <redacted>`",
            r#"{"command":"git grep [REDACTED]"}"#,
            r#"{"command":"git grep Widget","argv":["rg","Alias"]}"#,
        ] {
            assert_eq!(captured_shell_command(unavailable), None, "{unavailable}");
        }
    }
}

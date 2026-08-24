//! Closed admission for shell discovery and validation commands.

use serde_json::{Map, Value};
use temper_agent_core::{InvocationTargetAdmission, TargetAdmissionStatus};

const MAX_CLASSIFIED_COMMAND_BYTES: usize = 16 * 1024;

pub(super) fn classify_bash(arguments: &Map<String, Value>) -> InvocationTargetAdmission {
    let Some(command) = arguments.get("command").and_then(Value::as_str) else {
        return InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::MalformedTarget);
    };
    if source_neutral_script(command, false) {
        InvocationTargetAdmission::SourceNeutralProcess
    } else {
        InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::UnsupportedTool)
    }
}

fn source_neutral_script(command: &str, substitution: bool) -> bool {
    if command.is_empty()
        || command.len() > MAX_CLASSIFIED_COMMAND_BYTES
        || !command.is_ascii()
        || command
            .bytes()
            .any(|byte| matches!(byte, b'\n' | b'\r' | b';' | b'|' | b'<' | b'>' | b'`'))
    {
        return false;
    }
    let Some(command) = remove_safe_substitutions(command) else {
        return false;
    };
    command.split(" && ").all(|segment| {
        !segment.is_empty()
            && !segment
                .chars()
                .any(|character| matches!(character, '&' | '$' | '(' | ')' | '#'))
            && balanced_quotes(segment)
            && source_neutral_segment(segment.trim(), substitution)
    })
}

fn remove_safe_substitutions(command: &str) -> Option<String> {
    let mut remaining = command;
    let mut classified = String::with_capacity(command.len());
    while let Some(start) = remaining.find("$(") {
        classified.push_str(&remaining[..start]);
        let nested = &remaining[start + 2..];
        let end = nested.find(')')?;
        let inner = &nested[..end];
        if inner
            .chars()
            .any(|character| matches!(character, '(' | ')'))
            || !source_neutral_script(inner, true)
        {
            return None;
        }
        classified.push_str("SUBSTITUTION");
        remaining = &nested[end + 1..];
    }
    classified.push_str(remaining);
    Some(classified)
}

fn balanced_quotes(segment: &str) -> bool {
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for byte in segment.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && !single {
            escaped = true;
        } else if byte == b'\'' && !double {
            single = !single;
        } else if byte == b'"' && !single {
            double = !double;
        }
    }
    !escaped && !single && !double
}

fn source_neutral_segment(segment: &str, substitution: bool) -> bool {
    let words = segment.split_ascii_whitespace().collect::<Vec<_>>();
    match words.as_slice() {
        ["cd", path] => safe_word(path),
        ["cargo", "fmt", "--check"] => true,
        ["cargo", "test", arguments @ ..] => arguments.iter().all(|word| safe_word(word)),
        ["git", "diff", arguments @ ..] => arguments.iter().all(|word| safe_git_diff_word(word)),
        ["git", "status", arguments @ ..] => arguments.iter().all(|word| safe_word(word)),
        ["git", "rev-parse", arguments @ ..] => {
            !arguments.is_empty() && arguments.iter().all(|word| safe_word(word))
        }
        ["rg", arguments @ ..] => {
            !arguments.is_empty()
                && arguments
                    .iter()
                    .all(|word| !word.starts_with('-') && safe_word(word))
        }
        ["test", arguments @ ..] => {
            !arguments.is_empty() && arguments.iter().all(|word| safe_test_word(word))
        }
        ["printf", arguments @ ..] if substitution => {
            !arguments.is_empty() && arguments.iter().all(|word| safe_test_word(word))
        }
        ["cat", path] if substitution => safe_word(path),
        _ => false,
    }
}

fn safe_git_diff_word(word: &str) -> bool {
    !matches!(word, "--ext-diff" | "--textconv") && safe_word(word)
}

fn safe_test_word(word: &str) -> bool {
    word.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b' ' | b'\t' | b'!' | b'-' | b'_' | b'.' | b'/' | b'=' | b'\'' | b'"' | b'\\'
            )
    })
}

fn safe_word(word: &str) -> bool {
    word.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':' | b'=' | b'"' | b'\'')
    })
}

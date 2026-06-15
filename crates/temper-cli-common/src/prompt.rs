// SPDX-License-Identifier: MPL-2.0

//! The interactive-I/O seam: [`Prompter`] and its two implementations.
//!
//! Every interactive subcommand (today, `temper init`) talks to the operator
//! only through a `&mut dyn Prompter`. Production builds a [`TerminalPrompter`]
//! (questions + notes to stderr, answers from stdin, secrets read with no echo
//! via `rpassword`); tests build a [`ScriptedPrompter`] with a queue of canned
//! answers, so the flow's logic is exercised without ever touching a tty.

use std::collections::VecDeque;
use std::io::{self, BufRead, Write};

/// The interactive-I/O capability a CLI flow uses to talk to the operator.
///
/// Prompts and notes go to a side channel (stderr in production) so a flow's
/// real output (paths written, the final summary) stays uncluttered. Answers
/// come back trimmed of surrounding whitespace; an empty answer means "take the
/// default" where a default is offered.
pub trait Prompter {
    /// Asks `q`, optionally showing `default`, and returns the operator's answer
    /// (the default substituted when the answer is empty and a default exists).
    fn ask(&mut self, q: &str, default: Option<&str>) -> io::Result<String>;

    /// Asks `q` and reads a secret with no terminal echo.
    fn ask_secret(&mut self, q: &str) -> io::Result<String>;

    /// Asks a yes/no `q` with the given `default`, returning the answer.
    fn confirm(&mut self, q: &str, default: bool) -> io::Result<bool>;

    /// Emits an informational `line` (not a question).
    fn note(&mut self, line: &str);
}

/// A [`Prompter`] backed by the real terminal: questions + notes to a writer
/// (stderr in production), answers from a reader (stdin), and no-echo secrets
/// via `rpassword`.
///
/// The output sink and the plain-text reader are injectable so the type is
/// constructible in tests, but the secret read always goes through `rpassword`
/// (a tty), so tests use [`ScriptedPrompter`] instead of this type.
pub struct TerminalPrompter<R: BufRead, W: Write> {
    reader: R,
    writer: W,
}

impl TerminalPrompter<io::BufReader<io::Stdin>, io::Stderr> {
    /// Builds the production prompter: stdin for answers, stderr for prompts.
    pub fn stdio() -> Self {
        Self {
            reader: io::BufReader::new(io::stdin()),
            writer: io::stderr(),
        }
    }
}

impl<R: BufRead, W: Write> TerminalPrompter<R, W> {
    /// Builds a prompter over an explicit reader + writer (the secret read still
    /// goes through `rpassword`).
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    fn read_line(&mut self) -> io::Result<String> {
        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        Ok(line.trim().to_string())
    }
}

impl<R: BufRead, W: Write> Prompter for TerminalPrompter<R, W> {
    fn ask(&mut self, q: &str, default: Option<&str>) -> io::Result<String> {
        match default {
            Some(default) => write!(self.writer, "{q} [{default}]: ")?,
            None => write!(self.writer, "{q}: ")?,
        }
        self.writer.flush()?;
        let answer = self.read_line()?;
        if answer.is_empty()
            && let Some(default) = default
        {
            return Ok(default.to_string());
        }
        Ok(answer)
    }

    fn ask_secret(&mut self, q: &str) -> io::Result<String> {
        write!(self.writer, "{q}: ")?;
        self.writer.flush()?;
        // rpassword reads from the tty with echo disabled; it prints no newline,
        // so emit one ourselves to keep the prompt sequence readable.
        let secret = rpassword::read_password()?;
        writeln!(self.writer)?;
        Ok(secret.trim().to_string())
    }

    fn confirm(&mut self, q: &str, default: bool) -> io::Result<bool> {
        let hint = if default { "Y/n" } else { "y/N" };
        write!(self.writer, "{q} [{hint}]: ")?;
        self.writer.flush()?;
        let answer = self.read_line()?.to_lowercase();
        Ok(match answer.as_str() {
            "" => default,
            "y" | "yes" => true,
            _ => false,
        })
    }

    fn note(&mut self, line: &str) {
        let _ = writeln!(self.writer, "{line}");
    }
}

/// A [`Prompter`] for tests: answers come from a queue, notes are recorded.
///
/// It never touches a tty, so a flow built on [`Prompter`] can be exercised
/// end-to-end (collect → build → write) under `cargo test`. Feed it the answers
/// in prompt order; an empty string exercises the "take the default" path.
#[derive(Debug, Default)]
pub struct ScriptedPrompter {
    /// Canned answers, consumed front-to-back, one per `ask`/`ask_secret`.
    pub answers: VecDeque<String>,
    /// Canned yes/no answers for `confirm`, consumed front-to-back. When empty,
    /// `confirm` falls back to the supplied default.
    pub confirmations: VecDeque<bool>,
    /// Every line passed to `note`, in order (asserted in tests).
    pub notes: Vec<String>,
}

impl ScriptedPrompter {
    /// Builds a scripted prompter from a list of answers (in prompt order).
    pub fn new(answers: impl IntoIterator<Item = String>) -> Self {
        Self {
            answers: answers.into_iter().collect(),
            confirmations: VecDeque::new(),
            notes: Vec::new(),
        }
    }

    fn pop(&mut self, q: &str, default: Option<&str>) -> io::Result<String> {
        let raw = self.answers.pop_front().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("scripted prompter ran out of answers at: {q}"),
            )
        })?;
        let answer = raw.trim().to_string();
        if answer.is_empty()
            && let Some(default) = default
        {
            return Ok(default.to_string());
        }
        Ok(answer)
    }
}

impl Prompter for ScriptedPrompter {
    fn ask(&mut self, q: &str, default: Option<&str>) -> io::Result<String> {
        self.pop(q, default)
    }

    fn ask_secret(&mut self, q: &str) -> io::Result<String> {
        self.pop(q, None)
    }

    fn confirm(&mut self, _q: &str, default: bool) -> io::Result<bool> {
        Ok(self.confirmations.pop_front().unwrap_or(default))
    }

    fn note(&mut self, line: &str) {
        self.notes.push(line.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_takes_default_on_empty() {
        let mut p = ScriptedPrompter::new(["".to_string(), "typed".to_string()]);
        assert_eq!(p.ask("q1", Some("dfl")).unwrap(), "dfl");
        assert_eq!(p.ask("q2", Some("dfl")).unwrap(), "typed");
    }

    #[test]
    fn scripted_records_notes_and_confirms() {
        let mut p = ScriptedPrompter::new([]);
        p.note("hello");
        assert_eq!(p.notes, vec!["hello".to_string()]);
        // No scripted confirmation → falls back to the default.
        assert!(p.confirm("ok?", true).unwrap());
        assert!(!p.confirm("ok?", false).unwrap());
    }

    #[test]
    fn scripted_secret_pops_an_answer() {
        let mut p = ScriptedPrompter::new(["s3cr3t".to_string()]);
        assert_eq!(p.ask_secret("key").unwrap(), "s3cr3t");
    }

    #[test]
    fn terminal_prompter_reads_a_line_and_default() {
        let input = b"answer\n\n";
        let mut out: Vec<u8> = Vec::new();
        let mut p = TerminalPrompter::new(&input[..], &mut out);
        assert_eq!(p.ask("q", Some("dfl")).unwrap(), "answer");
        assert_eq!(p.ask("q", Some("dfl")).unwrap(), "dfl");
    }
}

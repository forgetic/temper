//! Measured provider-neutral results. Arbitrary file/process bytes remain
//! visible: invalid UTF-8 bytes are spelled `\\xNN` (llm.md, 5.1).
use crate::Error;
use alloc::boxed::Box;
use skein_lib::{Decimal, Writer};
use temper_legacy_agent_domain::{llm, run, tools};

pub(crate) struct Text {
    length: Option<usize>,
    writer: Option<Writer>,
}
impl Text {
    pub(crate) fn measure() -> Text {
        Text { length: Some(0), writer: None }
    }
    pub(crate) fn write(length: usize) -> Text {
        Text { length: Some(0), writer: Some(Writer::new(length)) }
    }
    pub(crate) fn put(&mut self, text: &[u8]) {
        self.length = match self.length {
            Some(n) => n.checked_add(text.len()),
            None => None,
        };
        if let Some(writer) = &mut self.writer {
            writer.put(text).expect("the measurement made room for these same bytes");
        }
    }
    pub(crate) fn number(&mut self, n: u64) {
        self.put(Decimal::of(n).as_bytes());
    }
    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        let mut at = 0;
        for _ in 0..bytes.len() {
            if at >= bytes.len() {
                break;
            }
            let n = utf8(bytes.get(at..).unwrap_or_default());
            if n == 0 {
                let byte = *bytes.get(at).expect("within the bytes");
                self.put(&[b'\\', b'x', hex(byte >> 4), hex(byte & 15)]);
                at = at.saturating_add(1);
            } else {
                self.put(bytes.get(at..at.saturating_add(n)).expect("a checked sequence"));
                at = at.saturating_add(n);
            }
        }
    }
    pub(crate) fn length(&self, max: u32) -> Result<usize, Error> {
        match self.length {
            Some(n) if n <= usize::try_from(max).expect("u32 fits usize") => Ok(n),
            Some(_) | None => Err(Error::TooLarge),
        }
    }
    pub(crate) fn finish(self) -> Box<[u8]> {
        self.writer.expect("the writing pass").finish()
    }
}

fn hex(n: u8) -> u8 {
    if n < 10 { b'0'.saturating_add(n) } else { b'A'.saturating_add(n.saturating_sub(10)) }
}

/// Length of the valid UTF-8 scalar at the start, zero for an invalid byte.
pub(crate) fn utf8(bytes: &[u8]) -> usize {
    let Some(&first) = bytes.first() else {
        return 0;
    };
    if first < 128 {
        return 1;
    }
    let n = match first {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return 0,
    };
    let Some(sequence) = bytes.get(..n) else {
        return 0;
    };
    for byte in sequence.get(1..).unwrap_or_default() {
        if !(0x80..=0xBF).contains(byte) {
            return 0;
        }
    }
    let second = *sequence.get(1).expect("a multibyte sequence");
    if (first == 0xE0 && second < 0xA0)
        || (first == 0xED && second >= 0xA0)
        || (first == 0xF0 && second < 0x90)
        || (first == 0xF4 && second >= 0x90)
    {
        return 0;
    }
    n
}

pub fn result(result: &llm::Returned, max: u32) -> Result<(Box<[u8]>, bool), Error> {
    let mut measure = Text::measure();
    let error = returned(result, &mut measure);
    let mut write = Text::write(measure.length(max)?);
    returned(result, &mut write);
    Ok((write.finish(), error))
}
pub(crate) fn length(result: &llm::Returned, max: u32) -> Result<usize, Error> {
    let mut measure = Text::measure();
    returned(result, &mut measure);
    measure.length(max)
}

fn returned(result: &llm::Returned, out: &mut Text) -> bool {
    match result {
        llm::Returned::Owned { outcome } => owned(outcome, out),
        llm::Returned::Served { returned, error } => {
            served(returned, out);
            *error
        }
        llm::Returned::Invalid { problem } => {
            out.put(b"malformed call: ");
            problem_text(problem, out);
            true
        }
        llm::Returned::NotRun => {
            out.put(b"not run: the answer stopped first");
            true
        }
    }
}

fn owned(value: &tools::Outcome, out: &mut Text) -> bool {
    use tools::Outcome;
    match value {
        Outcome::Read { content, skipped, lines, total, cut } => {
            read_text(content, *skipped, *lines, *total, *cut, out);
            false
        }
        Outcome::Listed { entries, more } => {
            for entry in entries {
                out.bytes(entry.name.as_bytes());
                match entry.kind {
                    tools::Kind::Directory => out.put(b"/"),
                    tools::Kind::File | tools::Kind::Link | tools::Kind::Other => {}
                }
                out.put(b"\n");
            }
            more_text(*more, b"entries", out);
            false
        }
        Outcome::Found { hits, more, timed_out } => {
            for hit in hits {
                out.bytes(&hit.path);
                out.put(b":");
                out.number(u64::from(hit.line));
                out.put(b": ");
                out.bytes(&hit.text);
                out.put(b"\n");
            }
            more_text(*more, b"hits", out);
            if *timed_out {
                out.put(b"search timed out\n");
            }
            *timed_out
        }
        Outcome::Written { created } => {
            out.put(if *created { b"file created" } else { b"file written" });
            false
        }
        Outcome::Edited { replaced } => {
            out.put(b"replaced ");
            out.number(u64::from(*replaced));
            out.put(b" occurrences");
            false
        }
        Outcome::Exited { exit, head, tail, dropped } => {
            let failed = exit_text(*exit, out);
            out.put(b"\n");
            out.bytes(head);
            if *dropped > 0 {
                out.put(b"\n[");
                out.number(*dropped);
                out.put(b" bytes omitted]\n");
            }
            out.bytes(tail);
            failed
        }
        Outcome::TooLarge { size } => {
            out.put(b"too large: ");
            out.number(*size);
            out.put(b" bytes");
            true
        }
        Outcome::Ambiguous { count, lines } => {
            out.put(b"ambiguous edit: ");
            out.number(u64::from(*count));
            out.put(b" matches; first lines");
            for line in lines {
                out.put(b" ");
                out.number(u64::from(*line));
            }
            true
        }
        Outcome::Failed { fault } => {
            out.put(b"io failed: ");
            out.put(match fault {
                tools::Fault::Denied => b"permission denied",
                tools::Fault::NoSpace => b"no space",
                tools::Fault::Other => b"other",
            });
            true
        }
        Outcome::NotGranted => fail(b"tool family not granted", out),
        Outcome::Outside => fail(b"path outside checkout", out),
        Outcome::ReadOnly => fail(b"repository is read only", out),
        Outcome::TooLong => fail(b"path too long", out),
        Outcome::NotFound => fail(b"path not found", out),
        Outcome::NotFile => fail(b"not a regular file", out),
        Outcome::Linked => fail(b"write crosses a symbolic link", out),
        Outcome::Protected => fail(b"git directory protected", out),
        Outcome::NotDirectory => fail(b"not a directory", out),
        Outcome::NotRead => fail(b"read the file before changing it", out),
        Outcome::Stale => fail(b"file changed; read it again", out),
        Outcome::NoMatch => fail(b"snippet not found", out),
        Outcome::Unchanged => fail(b"edit changes nothing", out),
        Outcome::TimedOut => fail(b"timed out", out),
        Outcome::Cancelled => fail(b"cancelled", out),
        Outcome::Busy => fail(b"tools busy", out),
        Outcome::NulByte => fail(b"process argument contains NUL", out),
    }
}
fn read_text(content: &[u8], skipped: u32, lines: u32, total: u32, cut: bool, out: &mut Text) {
    let mut start: usize = 0;
    let mut line = u64::from(skipped).saturating_add(1);
    for (at, &byte) in content.iter().enumerate() {
        if byte == b'\n' {
            out.number(line);
            out.put(b": ");
            out.bytes(content.get(start..at).expect("the line is within the content"));
            out.put(b"\n");
            start = at.saturating_add(1);
            line = line.saturating_add(1);
        }
    }
    if start < content.len() {
        out.number(line);
        out.put(b": ");
        out.bytes(content.get(start..).expect("the final line is within the content"));
        out.put(b"\n");
    }
    if lines == 0 {
        out.put(b"shown 0 lines of ");
    } else {
        out.put(b"shown lines ");
        out.number(u64::from(skipped).saturating_add(1));
        out.put(b"-");
        out.number(u64::from(skipped).saturating_add(u64::from(lines)));
        out.put(b" of ");
    }
    out.number(u64::from(total));
    if cut {
        out.put(b" (last line cut)");
    }
    out.put(b"\n");
}
fn fail(text: &[u8], out: &mut Text) -> bool {
    out.put(text);
    true
}
fn more_text(n: u64, what: &[u8], out: &mut Text) {
    if n > 0 {
        out.put(b"[");
        out.number(n);
        out.put(b" more ");
        out.put(what);
        out.put(b"]\n");
    }
}
fn exit_text(exit: tools::Exit, out: &mut Text) -> bool {
    match exit {
        tools::Exit::Code { code } => {
            out.put(b"exit ");
            out.number(u64::from(code));
            code != 0
        }
        tools::Exit::Signal { signal } => {
            out.put(b"killed by signal ");
            out.number(u64::from(signal));
            true
        }
        tools::Exit::TimedOut => {
            out.put(b"command timed out");
            true
        }
    }
}
fn problem_text(value: &llm::Problem, out: &mut Text) {
    match value {
        llm::Problem::UnknownTool => out.put(b"unknown tool"),
        llm::Problem::NotAnObject => out.put(b"input is not an object"),
        llm::Problem::TooLarge => out.put(b"input too large"),
        llm::Problem::Missing { field } => {
            out.put(b"missing ");
            out.bytes(field);
        }
        llm::Problem::WrongType { field } => {
            out.put(b"wrong type for ");
            out.bytes(field);
        }
        llm::Problem::BadValue { field } => {
            out.put(b"bad value for ");
            out.bytes(field);
        }
    }
}
fn served(value: &run::Returned, out: &mut Text) {
    match value {
        run::Returned::Accepted => out.put(b"outcome accepted"),
        run::Returned::Rejected { problems } => {
            out.put(b"outcome rejected\n");
            for problem in &problems.listed {
                outcome_problem(problem, out);
                out.put(b"\n");
            }
            more_text(u64::from(problems.more), b"problems", out);
        }
        run::Returned::ChecksFailed { repository, ran } => {
            out.put(b"checks failed in ");
            out.bytes(repository);
            out.put(b"\n");
            ran_text(ran, out);
        }
        run::Returned::Moved => out.put(b"branch moved; change not pushed"),
        run::Returned::Unpushed { failure } => {
            out.put(b"push failed");
            if let Some(repository) = failure.repository {
                out.put(b" in repository ");
                out.number(u64::from(repository));
            }
            out.put(b": ");
            out.put(match failure.reason {
                run::PushReason::MissingRepository => b"missing repository",
                run::PushReason::MissingBranch => b"missing branch",
                run::PushReason::MissingCommit => b"missing commit",
                run::PushReason::Refused => b"refused",
                run::PushReason::Unreachable => b"unreachable",
                run::PushReason::Broken => b"broken",
                run::PushReason::TimedOut => b"timed out",
                run::PushReason::Cancelled => b"cancelled",
                run::PushReason::Unavailable => b"unavailable",
                run::PushReason::Busy => b"busy",
                run::PushReason::TooLarge => b"too large",
                run::PushReason::Nothing => b"nothing to push",
                run::PushReason::Unknown => b"unknown",
            });
            out.put(b"\n");
            if failure.diagnostic.cut() > 0 {
                out.put(b"[");
                out.number(failure.diagnostic.cut());
                out.put(b" diagnostic bytes omitted]\n");
            }
            out.bytes(failure.diagnostic.output());
        }
        run::Returned::Cancelled => out.put(b"cancelled"),
        run::Returned::TimedOut => out.put(b"timed out"),
        run::Returned::Busy => out.put(b"run busy"),
        run::Returned::Answered { text, cut, stop: _ } => {
            out.bytes(text);
            more_text(*cut, b"answer bytes omitted", out);
        }
        run::Returned::Unanswered { end: _ } => out.put(b"sub-agent ended without an answer"),
        run::Returned::Refused { refusal: _ } => out.put(b"sub-agent refused"),
    }
}
fn ran_text(value: &run::Ran, out: &mut Text) {
    match value.exit {
        run::Exit::Code { code } => {
            out.put(b"exit ");
            out.number(u64::from(code));
        }
        run::Exit::Signalled => out.put(b"killed by signal"),
        run::Exit::TimedOut => out.put(b"timed out"),
        run::Exit::Unstarted => out.put(b"process did not start"),
    }
    out.put(b"\n");
    out.bytes(&value.output);
    more_text(value.cut, b"diagnostic bytes omitted", out);
}
fn outcome_problem(value: &run::outcome::Problem, out: &mut Text) {
    use run::outcome::Problem;
    match value {
        Problem::TooLarge { max } => {
            out.put(b"outcome exceeds ");
            out.number(*max);
            out.put(b" bytes");
        }
        Problem::ChangeNotAllowed => out.put(b"change not allowed"),
        Problem::VerdictNotAllowed => out.put(b"verdict not allowed"),
        Problem::UnknownVerdict => out.put(b"unknown verdict"),
        Problem::EmptyTitle => out.put(b"empty title"),
        Problem::EmptyBody => out.put(b"empty body"),
        Problem::TooFewChildren { min } => {
            out.put(b"at least ");
            out.number(u64::from(*min));
            out.put(b" children required");
        }
        Problem::TooManyChildren { max } => {
            out.put(b"at most ");
            out.number(u64::from(*max));
            out.put(b" children allowed");
        }
        Problem::KindNotAllowed { child } => {
            child_text(*child, out);
            out.put(b"kind not allowed");
        }
        Problem::MissingField { child, field } => {
            child_text(*child, out);
            out.put(b"missing ");
            out.bytes(field);
        }
        Problem::EmptyField { child, field } => {
            child_text(*child, out);
            out.put(b"empty ");
            out.bytes(field);
        }
        Problem::RepeatedField { child, field } => {
            child_text(*child, out);
            out.put(b"repeated ");
            out.bytes(field);
        }
    }
}
fn child_text(child: u32, out: &mut Text) {
    out.put(b"child ");
    out.number(u64::from(child));
    out.put(b": ");
}

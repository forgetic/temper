use std::collections::VecDeque;
use std::io::{self, Read};

use temper_protocol_agent::{ArtifactType, WorkspaceContext};

const STDERR_READ_BUFFER_BYTES: usize = 4 * 1024;
pub(super) const STDERR_LINE_BYTES: usize = 16 * 1024;
pub(super) const STDERR_TAIL_BYTES: usize = 2_000;
pub(super) const STDERR_TRUNCATION_MARKER: &str = " [stderr line truncated]";

/// Assignment identity attached by the worker to every child diagnostic.
///
/// All values come from the runner arguments or the worker-built context. Child
/// text is deliberately opaque and can never supply or override these fields.
pub(super) struct DiagnosticIdentity {
    job_id: String,
    correlation_key: String,
    role: String,
    repository: String,
    artifact: String,
}

impl DiagnosticIdentity {
    pub(super) fn from_context(job_id: &str, context: &WorkspaceContext) -> Self {
        let (repository, artifact) = context.artifact_context.as_ref().map_or_else(
            || {
                (
                    context
                        .primary()
                        .map(|repo| repo.id.clone())
                        .unwrap_or_else(|| "<unknown>".to_string()),
                    context.work_item.target.clone(),
                )
            },
            |bundle| {
                let artifact = &bundle.primary.artifact;
                let artifact_ref = match artifact.artifact_type {
                    ArtifactType::Issue => {
                        format!("{}#{}", artifact.repository.path, artifact.number)
                    }
                    ArtifactType::PullRequest => {
                        format!("{} PR#{}", artifact.repository.path, artifact.number)
                    }
                };
                (artifact.repository.path.clone(), artifact_ref)
            },
        );
        Self {
            job_id: job_id.to_string(),
            correlation_key: context.correlation_key.clone(),
            role: context.work_item.role.clone(),
            repository,
            artifact,
        }
    }
}

struct StderrLine {
    text: String,
    truncated: bool,
}

struct StderrReadReport {
    tail: String,
    read_error: Option<io::Error>,
}

/// Drains a child stderr pipe without ever retaining a whole line or stream.
pub(super) fn stream(reader: impl Read, identity: &DiagnosticIdentity) -> String {
    let report = read_stderr(reader, |line| emit_line(identity, &line));
    if let Some(error) = report.read_error {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "agent.stderr.read_failed",
            job_id = identity.job_id.as_str(),
            correlation_key = identity.correlation_key.as_str(),
            role = identity.role.as_str(),
            repository = identity.repository.as_str(),
            repo = identity.repository.as_str(),
            artifact = identity.artifact.as_str(),
            artifact.ref = identity.artifact.as_str(),
            %error,
            "worker: agent stderr reader failed; streamed diagnostics may be incomplete"
        );
    }
    report.tail
}

pub(super) fn emit_reader_unavailable(identity: &DiagnosticIdentity) {
    tracing::warn!(
        target: "temper::worker",
        service = "worker",
        event = "agent.stderr.reader_unavailable",
        job_id = identity.job_id.as_str(),
        correlation_key = identity.correlation_key.as_str(),
        role = identity.role.as_str(),
        repository = identity.repository.as_str(),
        repo = identity.repository.as_str(),
        artifact = identity.artifact.as_str(),
        artifact.ref = identity.artifact.as_str(),
        "worker: agent stderr pipe was unavailable"
    );
}

fn emit_line(identity: &DiagnosticIdentity, line: &StderrLine) {
    tracing::debug!(
        target: "temper::worker",
        service = "worker",
        event = "agent.stderr",
        job_id = identity.job_id.as_str(),
        correlation_key = identity.correlation_key.as_str(),
        role = identity.role.as_str(),
        repository = identity.repository.as_str(),
        repo = identity.repository.as_str(),
        artifact = identity.artifact.as_str(),
        artifact.ref = identity.artifact.as_str(),
        stream = "stderr",
        truncated = line.truncated,
        "{}",
        line.text
    );
}

fn read_stderr(mut reader: impl Read, mut on_line: impl FnMut(StderrLine)) -> StderrReadReport {
    let mut buffer = [0_u8; STDERR_READ_BUFFER_BYTES];
    let mut line = Vec::with_capacity(STDERR_LINE_BYTES);
    let mut line_truncated = false;
    let mut tail = RollingTail::new(STDERR_TAIL_BYTES);
    let read_error = loop {
        match reader.read(&mut buffer) {
            Ok(0) => break None,
            Ok(read) => {
                tail.push(&buffer[..read]);
                for &byte in &buffer[..read] {
                    if byte == b'\n' {
                        on_line(finish_line(&mut line, line_truncated));
                        line_truncated = false;
                    } else if line.len() < STDERR_LINE_BYTES {
                        line.push(byte);
                    } else {
                        line_truncated = true;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => break Some(error),
        }
    };
    if !line.is_empty() || line_truncated {
        on_line(finish_line(&mut line, line_truncated));
    }
    StderrReadReport {
        tail: tail.finish(),
        read_error,
    }
}

fn finish_line(bytes: &mut Vec<u8>, truncated: bool) -> StderrLine {
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    bytes.clear();
    if truncated {
        text.push_str(STDERR_TRUNCATION_MARKER);
    }
    StderrLine { text, truncated }
}

struct RollingTail {
    bytes: VecDeque<u8>,
    capacity: usize,
}

impl RollingTail {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.capacity {
            self.bytes.clear();
            self.bytes
                .extend(bytes[bytes.len() - self.capacity..].iter().copied());
            return;
        }
        let overflow = self
            .bytes
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(self.capacity);
        self.bytes.drain(..overflow);
        self.bytes.extend(bytes.iter().copied());
    }

    fn finish(self) -> String {
        let bytes = self.bytes.into_iter().collect::<Vec<_>>();
        stderr_tail(&bytes, self.capacity)
    }
}

/// Last `max_len` lossy UTF-8 bytes, cut only on a character boundary.
pub(super) fn stderr_tail(stderr: &[u8], max_len: usize) -> String {
    let text = String::from_utf8_lossy(stderr).into_owned();
    if text.len() <= max_len {
        return text;
    }
    let mut start = text.len() - max_len;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Error, ErrorKind, Read};

    use super::*;

    #[test]
    fn lines_are_lossy_bounded_and_explicitly_truncated() {
        let mut input = vec![b'x'; STDERR_LINE_BYTES + 500];
        input.extend_from_slice(b"\ninvalid:\xff\r\nfinal");
        let mut lines = Vec::new();
        let report = read_stderr(Cursor::new(input), |line| lines.push(line));

        assert!(report.read_error.is_none());
        assert_eq!(lines.len(), 3);
        assert!(lines[0].truncated);
        assert_eq!(
            lines[0].text.len(),
            STDERR_LINE_BYTES + STDERR_TRUNCATION_MARKER.len()
        );
        assert!(lines[0].text.ends_with(STDERR_TRUNCATION_MARKER));
        assert_eq!(lines[1].text, "invalid:\u{fffd}");
        assert_eq!(lines[2].text, "final");
    }

    #[test]
    fn long_stream_retains_only_the_rolling_tail() {
        let input = "diagnostic line\n".repeat(20_000);
        let mut count = 0;
        let report = read_stderr(Cursor::new(input.as_bytes()), |_| count += 1);

        assert_eq!(count, 20_000);
        assert!(report.tail.len() <= STDERR_TAIL_BYTES);
        assert!(report.tail.ends_with("diagnostic line\n"));
    }

    #[test]
    fn eof_emits_an_unterminated_final_line() {
        let mut lines = Vec::new();
        let report = read_stderr(Cursor::new(b"closed reader"), |line| lines.push(line.text));

        assert!(report.read_error.is_none());
        assert_eq!(lines, ["closed reader"]);
    }

    #[test]
    fn reader_error_preserves_diagnostics_and_is_non_fatal() {
        let mut lines = Vec::new();
        let report = read_stderr(ErrorAfterData::new(b"before close"), |line| {
            lines.push(line.text)
        });

        assert_eq!(lines, ["before close"]);
        assert_eq!(report.tail, "before close");
        assert_eq!(
            report.read_error.as_ref().map(io::Error::kind),
            Some(ErrorKind::BrokenPipe)
        );
    }

    struct ErrorAfterData {
        data: Cursor<Vec<u8>>,
        returned_error: bool,
    }

    impl ErrorAfterData {
        fn new(data: &[u8]) -> Self {
            Self {
                data: Cursor::new(data.to_vec()),
                returned_error: false,
            }
        }
    }

    impl Read for ErrorAfterData {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = self.data.read(buffer)?;
            if read > 0 {
                return Ok(read);
            }
            if self.returned_error {
                Ok(0)
            } else {
                self.returned_error = true;
                Err(Error::new(ErrorKind::BrokenPipe, "reader closed"))
            }
        }
    }
}

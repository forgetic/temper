use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::{ContainmentSignal, ProcessIdentity, SignalAttempt, SignalBatch};

const MAX_PROTOCOL_LINE: usize = 16 * 1024;

pub(super) struct SupervisorClient {
    reader: BufReader<std::os::unix::net::UnixStream>,
    writer: std::os::unix::net::UnixStream,
    terminal: Option<ProtocolFrame>,
}

impl SupervisorClient {
    pub(super) fn new(
        reader: std::os::unix::net::UnixStream,
        writer: std::os::unix::net::UnixStream,
    ) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            terminal: None,
        }
    }

    pub(super) fn terminal(&self) -> Option<&ProtocolFrame> {
        self.terminal.as_ref()
    }

    pub(super) fn request(&mut self, command: u8) -> io::Result<ProtocolFrame> {
        if let Some(terminal) = &self.terminal {
            return Ok(terminal.clone());
        }
        let write_result = self.writer.write_all(&[command]);
        if let Err(error) = &write_result {
            if !matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::NotConnected
            ) {
                return Err(io::Error::new(error.kind(), error.to_string()));
            }
        }
        let frame = self.read_frame()?;
        if matches!(frame, ProtocolFrame::Final { .. }) {
            self.terminal = Some(frame.clone());
        }
        Ok(frame)
    }

    pub(super) fn read_frame(&mut self) -> io::Result<ProtocolFrame> {
        let header = read_protocol_line(&mut self.reader)?;
        let fields: Vec<&str> = header.split('\t').collect();
        match fields.as_slice() {
            ["R", pid] => Ok(ProtocolFrame::Ready {
                payload_pid: parse_field(pid, "payload pid")?,
            }),
            ["M", omitted, count] => {
                let omitted = parse_field(omitted, "omitted member count")?;
                let count: usize = parse_field(count, "member count")?;
                let mut members = Vec::with_capacity(count.min(crate::MAX_SURVIVOR_IDENTITIES));
                for _ in 0..count {
                    members.push(read_process_line(&mut self.reader)?);
                }
                Ok(ProtocolFrame::Members { members, omitted })
            }
            ["A", omitted, count] => {
                let omitted = parse_field(omitted, "omitted attempt count")?;
                let count: usize = parse_field(count, "attempt count")?;
                let mut attempts = Vec::with_capacity(count.min(crate::MAX_SIGNAL_ATTEMPTS));
                for _ in 0..count {
                    attempts.push(read_attempt_line(&mut self.reader)?);
                }
                Ok(ProtocolFrame::Attempts { attempts, omitted })
            }
            ["E", inspections] => Ok(ProtocolFrame::Empty {
                inspections: parse_field(inspections, "inspection count")?,
            }),
            [
                "F",
                inspections,
                payload_status,
                term_attempted,
                term_omitted,
                term_count,
                kill_attempted,
                kill_omitted,
                kill_count,
            ] => Ok(ProtocolFrame::Final {
                inspections: parse_field(inspections, "inspection count")?,
                payload_status: parse_field(payload_status, "payload status")?,
                automatic_term: read_optional_signal_batch(
                    &mut self.reader,
                    ContainmentSignal::Term,
                    term_attempted,
                    term_omitted,
                    term_count,
                )?,
                automatic_kill: read_optional_signal_batch(
                    &mut self.reader,
                    ContainmentSignal::Kill,
                    kill_attempted,
                    kill_omitted,
                    kill_count,
                )?,
            }),
            ["!", message] => Ok(ProtocolFrame::Error(decode_text(message)?)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid Linux supervisor protocol header: {header:?}"),
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum ProtocolFrame {
    Ready {
        payload_pid: u32,
    },
    Members {
        members: Vec<ProcessIdentity>,
        omitted: usize,
    },
    Attempts {
        attempts: Vec<SignalAttempt>,
        omitted: usize,
    },
    Empty {
        inspections: u64,
    },
    Final {
        inspections: u64,
        payload_status: i32,
        automatic_term: Option<SignalBatch>,
        automatic_kill: Option<SignalBatch>,
    },
    Error(String),
}

fn read_protocol_line(reader: &mut impl BufRead) -> io::Result<String> {
    let mut line = String::new();
    let read = reader.read_line(&mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Linux supervisor control channel closed without cleanup proof",
        ));
    }
    if line.len() > MAX_PROTOCOL_LINE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Linux supervisor protocol line exceeds its bound",
        ));
    }
    while matches!(line.as_bytes().last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Ok(line)
}

pub(super) fn read_process_line(reader: &mut impl BufRead) -> io::Result<ProcessIdentity> {
    let line = read_protocol_line(reader)?;
    parse_process_fields(&line)
}

fn parse_process_fields(line: &str) -> io::Result<ProcessIdentity> {
    let fields: Vec<&str> = line.split('\t').collect();
    let ["P", pid, ppid, pgid, sid, start, executable] = fields.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Linux supervisor process identity",
        ));
    };
    Ok(ProcessIdentity::new(
        parse_field(pid, "pid")?,
        parse_field(ppid, "ppid")?,
        parse_field(pgid, "process group")?,
        parse_field(sid, "session")?,
        parse_field(start, "start time")?,
        PathBuf::from(decode_text(executable)?),
    ))
}

fn read_attempt_line(reader: &mut impl BufRead) -> io::Result<SignalAttempt> {
    let line = read_protocol_line(reader)?;
    let mut fields = line.splitn(4, '\t');
    if fields.next() != Some("A") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Linux supervisor signal attempt",
        ));
    }
    let outcome = fields.next().unwrap_or_default();
    let message = decode_text(fields.next().unwrap_or_default())?;
    let process = parse_process_fields(fields.next().unwrap_or_default())?;
    // The response corresponds to the request currently in flight. The helper
    // includes the signal in each line so malformed/crossed frames fail closed.
    let (signal, outcome) = outcome.split_once(':').ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid signal attempt outcome")
    })?;
    let signal = match signal {
        "T" => ContainmentSignal::Term,
        "K" => ContainmentSignal::Kill,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid signal attempt signal",
            ));
        }
    };
    match outcome {
        "ok" => Ok(SignalAttempt::succeeded(process, signal)),
        "gone" => Ok(SignalAttempt::process_gone(process, signal)),
        "reused" => Ok(SignalAttempt::pid_reused(process, signal)),
        "failed" => Ok(SignalAttempt::failed(process, signal, message)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid signal attempt result",
        )),
    }
}

fn read_optional_signal_batch(
    reader: &mut impl BufRead,
    expected_signal: ContainmentSignal,
    attempted: &str,
    omitted: &str,
    count: &str,
) -> io::Result<Option<SignalBatch>> {
    let attempted: u8 = parse_field(attempted, "automatic signal marker")?;
    if attempted > 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid automatic signal marker",
        ));
    }
    let omitted = parse_field(omitted, "automatic omitted attempt count")?;
    let count: usize = parse_field(count, "automatic attempt count")?;
    if count > crate::MAX_SIGNAL_ATTEMPTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "automatic signal batch exceeds its protocol bound",
        ));
    }
    let mut attempts = Vec::with_capacity(count);
    for _ in 0..count {
        let attempt = read_attempt_line(reader)?;
        if attempt.signal() != expected_signal {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "automatic signal batch contains the wrong signal",
            ));
        }
        attempts.push(attempt);
    }
    if attempted == 0 {
        if omitted != 0 || !attempts.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unattempted automatic signal has diagnostics",
            ));
        }
        Ok(None)
    } else {
        Ok(Some(SignalBatch::new(attempts, omitted)))
    }
}

fn parse_field<T: std::str::FromStr>(value: &str, name: &str) -> io::Result<T> {
    value.parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid Linux supervisor {name}"),
        )
    })
}

fn encode_text(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn decode_text(value: &str) -> io::Result<String> {
    if value.len() % 2 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid hex text in Linux supervisor protocol",
        ));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let pair = std::str::from_utf8(pair)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        bytes.push(u8::from_str_radix(pair, 16).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid protocol hex digit")
        })?);
    }
    String::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
}

pub(super) struct AutomaticSignalEvidence<'a> {
    pub(super) attempted: bool,
    pub(super) attempts: &'a [SignalAttempt],
    pub(super) omitted: usize,
}

pub(super) fn send_final(
    channel: &mut std::os::unix::net::UnixStream,
    inspections: u64,
    payload_status: i32,
    term: AutomaticSignalEvidence<'_>,
    kill: AutomaticSignalEvidence<'_>,
) -> io::Result<()> {
    writeln!(
        channel,
        "F\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        inspections,
        payload_status,
        u8::from(term.attempted),
        term.omitted,
        term.attempts.len(),
        u8::from(kill.attempted),
        kill.omitted,
        kill.attempts.len(),
    )?;
    for attempt in term.attempts {
        write_attempt(channel, attempt)?;
    }
    for attempt in kill.attempts {
        write_attempt(channel, attempt)?;
    }
    channel.flush()
}

pub(super) fn send_members(
    channel: &mut std::os::unix::net::UnixStream,
    members: &[ProcessIdentity],
) -> io::Result<()> {
    let retained = members.len().min(crate::MAX_SURVIVOR_IDENTITIES);
    writeln!(channel, "M\t{}\t{retained}", members.len() - retained)?;
    for process in members.iter().take(retained) {
        write_process(channel, process)?;
    }
    channel.flush()
}

pub(super) fn send_attempts(
    channel: &mut std::os::unix::net::UnixStream,
    batch: &SignalBatch,
) -> io::Result<()> {
    writeln!(
        channel,
        "A\t{}\t{}",
        batch.omitted(),
        batch.attempts().len()
    )?;
    for attempt in batch.attempts() {
        write_attempt(channel, attempt)?;
    }
    channel.flush()
}

fn write_attempt(writer: &mut impl Write, attempt: &SignalAttempt) -> io::Result<()> {
    let signal_code = match attempt.signal() {
        ContainmentSignal::Term => "T",
        ContainmentSignal::Kill => "K",
    };
    let (outcome, message) = match attempt.outcome() {
        crate::SignalAttemptOutcome::Succeeded => ("ok", String::new()),
        crate::SignalAttemptOutcome::ProcessGone => ("gone", String::new()),
        crate::SignalAttemptOutcome::PidReused => ("reused", String::new()),
        crate::SignalAttemptOutcome::Failed(message) => ("failed", message.clone()),
    };
    write!(
        writer,
        "A\t{signal_code}:{outcome}\t{}\t",
        encode_text(&message)
    )?;
    write_process(writer, attempt.process())
}

pub(super) fn write_process(writer: &mut impl Write, process: &ProcessIdentity) -> io::Result<()> {
    writeln!(
        writer,
        "P\t{}\t{}\t{}\t{}\t{}\t{}",
        process.pid(),
        process.ppid(),
        process.process_group_id(),
        process.session_id(),
        process.start_time_identity(),
        encode_text(&process.executable().to_string_lossy())
    )
}

pub(super) fn send_error(
    channel: &mut std::os::unix::net::UnixStream,
    error: &io::Error,
) -> io::Result<()> {
    writeln!(channel, "!\t{}", encode_text(&error.to_string()))?;
    channel.flush()
}

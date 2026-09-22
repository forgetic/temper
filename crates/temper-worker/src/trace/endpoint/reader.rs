use std::io::{self, BufRead, BufReader};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::MAX_CHILD_ACTIVITY_RECORD_BYTES;

/// One persistent child stream, including a finite shutdown drain. Only socket
/// reads consume its waiting budget; durable event appends happen outside it.
pub(super) struct ActivityRecordReader {
    reader: BufReader<TcpStream>,
    wire_limit: u64,
    drain_remaining: Option<u64>,
    read_wait_remaining: Duration,
}

impl ActivityRecordReader {
    pub(super) fn new(
        stream: TcpStream,
        read_poll: Duration,
        wire_limit: u64,
    ) -> Result<Self, String> {
        stream
            .set_read_timeout(Some(read_poll))
            .map_err(|error| format!("set activity record read timeout: {error}"))?;
        Ok(Self {
            reader: BufReader::new(stream),
            wire_limit,
            drain_remaining: None,
            read_wait_remaining: read_poll,
        })
    }

    pub(super) fn read_record(&mut self, stopping: &AtomicBool) -> Result<Option<Vec<u8>>, String> {
        let mut bytes = Vec::new();
        let allowance = (MAX_CHILD_ACTIVITY_RECORD_BYTES as u64).saturating_add(3);
        loop {
            self.observe_stop(stopping);
            if self.drain_remaining == Some(0) {
                return Err("terminal activity drain byte limit exhausted".to_string());
            }
            let remaining = allowance.saturating_sub(bytes.len() as u64);
            let remaining = self
                .drain_remaining
                .map_or(remaining, |budget| budget.min(remaining));
            if remaining == 0 {
                return Err(format!(
                    "child activity record exceeds {MAX_CHILD_ACTIVITY_RECORD_BYTES} bytes"
                ));
            }
            self.prepare_read()?;
            let started = Instant::now();
            let result = self.read_chunk(&mut bytes, remaining);
            self.observe_stop(stopping);
            if self.drain_remaining.is_some() {
                self.read_wait_remaining =
                    self.read_wait_remaining.saturating_sub(started.elapsed());
            }
            let read = match result {
                Ok(read) => read,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    if stopping.load(Ordering::Acquire) {
                        return if bytes.is_empty() {
                            Ok(None)
                        } else {
                            Err("partial child activity record at shutdown".to_string())
                        };
                    }
                    continue;
                }
                Err(error) => return Err(format!("read child activity record: {error}")),
            };
            if let Some(budget) = &mut self.drain_remaining {
                *budget = budget.saturating_sub(read as u64);
            }
            if read == 0 {
                return if bytes.is_empty() {
                    Ok(None)
                } else if bytes.len() > MAX_CHILD_ACTIVITY_RECORD_BYTES {
                    Err(format!(
                        "child activity record exceeds {MAX_CHILD_ACTIVITY_RECORD_BYTES} bytes"
                    ))
                } else {
                    Err("child activity record is not newline terminated".to_string())
                };
            }
            if bytes.last() == Some(&b'\n') {
                return Ok(Some(bytes));
            }
        }
    }

    fn observe_stop(&mut self, stopping: &AtomicBool) {
        if stopping.load(Ordering::Acquire) && self.drain_remaining.is_none() {
            self.drain_remaining = Some(self.wire_limit);
        }
    }

    fn prepare_read(&self) -> Result<(), String> {
        if self.drain_remaining.is_none() || !self.reader.buffer().is_empty() {
            return Ok(());
        }
        let stream = self.reader.get_ref();
        let result = if self.read_wait_remaining.is_zero() {
            // Exhausted socket waiting still permits already-queued bytes to
            // drain. Slow durable storage never advances this network budget.
            stream.set_nonblocking(true)
        } else {
            stream.set_read_timeout(Some(self.read_wait_remaining))
        };
        result.map_err(|error| format!("prepare terminal activity drain: {error}"))
    }

    fn read_chunk(&mut self, bytes: &mut Vec<u8>, remaining: u64) -> io::Result<usize> {
        // One chunk per iteration observes shutdown even for a peer trickling
        // an unterminated frame; read_until could otherwise hide that progress.
        let available = self.reader.fill_buf()?;
        let limit = usize::try_from(remaining).unwrap_or(usize::MAX);
        let available = &available[..available.len().min(limit)];
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        bytes.extend_from_slice(&available[..count]);
        self.reader.consume(count);
        Ok(count)
    }
}

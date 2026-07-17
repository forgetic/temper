//! Byte-bounded stream capture that always drains its input.
//!
//! Machine-readable streams use [`CaptureMode::Complete`]: bytes are retained
//! only up to the configured limit and [`BoundedCapture::finish`] returns a
//! typed [`CaptureOverflow`] if the producer exceeded it. Diagnostic streams
//! use [`CaptureMode::Tail`]: the newest bytes are retained and the result
//! reports exactly how many older bytes were dropped.

use std::fmt;
use std::io::{self, Read};

/// Retention policy for a drained byte stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureMode {
    /// Preserve the complete stream or fail explicitly on overflow.
    Complete,
    /// Preserve only the newest bytes and report the number dropped.
    Tail,
}

/// A complete-mode stream exceeded its named byte limit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureOverflow {
    limit_bytes: usize,
    observed_bytes: u64,
}

impl CaptureOverflow {
    pub fn new(limit_bytes: usize, observed_bytes: u64) -> Self {
        Self {
            limit_bytes,
            observed_bytes,
        }
    }

    pub fn limit_bytes(&self) -> usize {
        self.limit_bytes
    }

    pub fn observed_bytes(&self) -> u64 {
        self.observed_bytes
    }

    pub fn dropped_bytes(&self) -> u64 {
        self.observed_bytes
            .saturating_sub(u64::try_from(self.limit_bytes).unwrap_or(u64::MAX))
    }
}

impl fmt::Display for CaptureOverflow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "complete capture exceeded {} byte limit after observing {} bytes",
            self.limit_bytes, self.observed_bytes
        )
    }
}

impl std::error::Error for CaptureOverflow {}

/// Successful bounded capture result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedBytes {
    bytes: Vec<u8>,
    observed_bytes: u64,
    dropped_bytes: u64,
}

impl CapturedBytes {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn observed_bytes(&self) -> u64 {
        self.observed_bytes
    }

    /// Number of older bytes omitted by tail mode. Complete captures always
    /// report zero here; an overflowing complete capture is an error instead.
    pub fn dropped_bytes(&self) -> u64 {
        self.dropped_bytes
    }
}

/// Incremental bounded byte capture.
///
/// Call [`push`](Self::push) for each chunk while continuing to drain the
/// producer, or use [`drain`](Self::drain) for a blocking [`Read`] stream.
#[derive(Clone, Debug)]
pub struct BoundedCapture {
    mode: CaptureMode,
    limit_bytes: usize,
    bytes: Vec<u8>,
    observed_bytes: u64,
}

impl BoundedCapture {
    pub fn new(mode: CaptureMode, limit_bytes: usize) -> Self {
        Self {
            mode,
            limit_bytes,
            bytes: Vec::with_capacity(limit_bytes.min(8 * 1024)),
            observed_bytes: 0,
        }
    }

    pub fn mode(&self) -> CaptureMode {
        self.mode
    }

    pub fn limit_bytes(&self) -> usize {
        self.limit_bytes
    }

    pub fn retained_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub fn observed_bytes(&self) -> u64 {
        self.observed_bytes
    }

    pub fn dropped_bytes(&self) -> u64 {
        self.observed_bytes
            .saturating_sub(u64::try_from(self.bytes.len()).unwrap_or(u64::MAX))
    }

    /// Retains this chunk according to the configured policy. This method never
    /// returns early on complete-mode overflow, allowing callers to keep
    /// draining a pipe and avoid deadlocking its producer.
    pub fn push(&mut self, chunk: &[u8]) {
        self.observed_bytes = self
            .observed_bytes
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        match self.mode {
            CaptureMode::Complete => {
                let remaining = self.limit_bytes.saturating_sub(self.bytes.len());
                self.bytes
                    .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            CaptureMode::Tail => self.push_tail(chunk),
        }
    }

    /// Drains a reader to EOF while retaining no more than the configured
    /// limit. Overflow is deliberately reported only by [`finish`](Self::finish)
    /// after EOF has been reached.
    pub fn drain(&mut self, reader: &mut impl Read) -> io::Result<()> {
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(read) => self.push(&buffer[..read]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }

    /// Finalizes the capture. Complete mode never exposes truncated
    /// machine-readable bytes; tail mode returns retained bytes and a precise
    /// dropped-byte count.
    pub fn finish(self) -> Result<CapturedBytes, CaptureOverflow> {
        if self.mode == CaptureMode::Complete
            && self.observed_bytes > u64::try_from(self.limit_bytes).unwrap_or(u64::MAX)
        {
            return Err(CaptureOverflow::new(self.limit_bytes, self.observed_bytes));
        }
        let dropped_bytes = self
            .observed_bytes
            .saturating_sub(u64::try_from(self.bytes.len()).unwrap_or(u64::MAX));
        Ok(CapturedBytes {
            bytes: self.bytes,
            observed_bytes: self.observed_bytes,
            dropped_bytes,
        })
    }

    fn push_tail(&mut self, chunk: &[u8]) {
        if self.limit_bytes == 0 {
            return;
        }
        if chunk.len() >= self.limit_bytes {
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&chunk[chunk.len() - self.limit_bytes..]);
            return;
        }
        let overflow = self
            .bytes
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(self.limit_bytes);
        if overflow > 0 {
            self.bytes.drain(..overflow);
        }
        self.bytes.extend_from_slice(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_capture_reports_typed_overflow_without_retaining_excess() {
        let mut capture = BoundedCapture::new(CaptureMode::Complete, 4);
        capture.push(b"abc");
        capture.push(b"def");

        assert_eq!(capture.retained_bytes(), 4);
        let overflow = capture.finish().expect_err("complete data was truncated");
        assert_eq!(overflow.limit_bytes(), 4);
        assert_eq!(overflow.observed_bytes(), 6);
        assert_eq!(overflow.dropped_bytes(), 2);
    }

    #[test]
    fn tail_capture_reports_dropped_bytes() {
        let mut capture = BoundedCapture::new(CaptureMode::Tail, 4);
        capture.push(b"ab");
        capture.push(b"cdef");

        let captured = capture.finish().expect("tail capture cannot overflow");
        assert_eq!(captured.as_bytes(), b"cdef");
        assert_eq!(captured.observed_bytes(), 6);
        assert_eq!(captured.dropped_bytes(), 2);
    }

    #[test]
    fn zero_length_tail_still_counts_drained_bytes() {
        let mut capture = BoundedCapture::new(CaptureMode::Tail, 0);
        capture.push(b"abc");
        let captured = capture.finish().unwrap();
        assert!(captured.as_bytes().is_empty());
        assert_eq!(captured.dropped_bytes(), 3);
    }
}

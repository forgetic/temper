// SPDX-License-Identifier: MPL-2.0

//! A file-backed [`LogLineSource`](crate::feeds::logtail::LogLineSource) that
//! tails a growing `temper-log` JSON-lines file.
//!
//! It reads newly-appended complete lines, returning `None` when caught up so
//! the pump idles and emits a keep-alive. This is the production source today;
//! a future in-process broadcast seam in `temper-log` swaps in behind the same
//! trait with no change to the read-model or the UI (UX §6.1 a→b).
//!
//! ## Tailing semantics
//!
//! The binary opens this source with `from_end = true`, so the board only sees
//! events emitted *after* temper-web started. This is intentional and is NOT a
//! gap: card **existence** is owned by the periodic snapshot re-poll (Part A),
//! which backfills any in-flight item the tail missed; the live tail only
//! **refines** existing cards (lane moves, gate/CI badges, activity). No bounded
//! history replay (`from_end = false` with a cap) is implemented — the snapshot
//! re-poll makes it unnecessary. See [`crate::feeds::logtail`] for the matching
//! note on the read-model side.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::feeds::logtail::LogLineSource;

/// Tails a JSON-lines log file from a tracked byte offset, yielding one complete
/// appended line per `next_line`, and `None` once caught up.
pub struct FileLogSource {
    path: PathBuf,
    reader: Option<BufReader<File>>,
    offset: u64,
}

impl FileLogSource {
    /// Open a tail over `path`. Starts at `from_end ? <end> : 0`: a live tail
    /// skips history with `from_end = true`; a replay reads from the start.
    pub fn open(path: impl AsRef<Path>, from_end: bool) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut source = Self {
            path,
            reader: None,
            offset: 0,
        };
        if from_end && let Ok(meta) = std::fs::metadata(&source.path) {
            source.offset = meta.len();
        }
        source.reopen()?;
        Ok(source)
    }

    fn reopen(&mut self) -> std::io::Result<()> {
        match File::open(&self.path) {
            Ok(mut file) => {
                file.seek(SeekFrom::Start(self.offset))?;
                self.reader = Some(BufReader::new(file));
                Ok(())
            }
            Err(_) => {
                // File not present yet; tolerate and retry on the next poll.
                self.reader = None;
                Ok(())
            }
        }
    }
}

impl LogLineSource for FileLogSource {
    fn next_line(&mut self) -> Option<String> {
        if self.reader.is_none() {
            self.reopen().ok()?;
        }
        let reader = self.reader.as_mut()?;
        let mut line = String::new();
        match reader.read_line(&mut line) {
            // A partial line (no trailing newline) means we caught up mid-write.
            // The reader has buffered those bytes past `self.offset`; reopen at
            // `self.offset` so the next poll re-reads the full line once the
            // writer finishes it. Do not advance the offset.
            Ok(0) => None,
            Ok(_) if !line.ends_with('\n') => {
                self.reader = None;
                self.reopen().ok();
                None
            }
            Ok(n) => {
                self.offset += n as u64;
                Some(line.trim_end().to_string())
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_log() -> PathBuf {
        std::env::temp_dir().join(format!(
            "temper-web-log-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn replays_existing_lines_then_tails_appends() {
        let path = temp_log();
        std::fs::write(&path, "line-1\nline-2\n").unwrap();
        let mut source = FileLogSource::open(&path, false).unwrap();
        assert_eq!(source.next_line().as_deref(), Some("line-1"));
        assert_eq!(source.next_line().as_deref(), Some("line-2"));
        assert_eq!(source.next_line(), None); // caught up

        // append a new line; the tail picks it up
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "line-3").unwrap();
        assert_eq!(source.next_line().as_deref(), Some("line-3"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn from_end_skips_history() {
        let path = temp_log();
        std::fs::write(&path, "old-1\nold-2\n").unwrap();
        let mut source = FileLogSource::open(&path, true).unwrap();
        assert_eq!(source.next_line(), None); // history skipped

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "new-1").unwrap();
        assert_eq!(source.next_line().as_deref(), Some("new-1"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn partial_trailing_line_is_not_yielded_until_complete() {
        let path = temp_log();
        std::fs::write(&path, "complete\npartial").unwrap();
        let mut source = FileLogSource::open(&path, false).unwrap();
        assert_eq!(source.next_line().as_deref(), Some("complete"));
        assert_eq!(source.next_line(), None); // partial held back

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file).unwrap(); // finish the partial line
        assert_eq!(source.next_line().as_deref(), Some("partial"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_is_tolerated() {
        let path = temp_log();
        let mut source = FileLogSource::open(&path, false).unwrap();
        assert_eq!(source.next_line(), None);
        std::fs::write(&path, "appeared\n").unwrap();
        assert_eq!(source.next_line().as_deref(), Some("appeared"));
        std::fs::remove_file(&path).ok();
    }
}

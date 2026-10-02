//! Calls as the protocol layer would decode them from what an LLM wrote.

use temper_agent_model_tools::Call;

use crate::translate::path;

/// Reads the file at `at` whole, as far as a read goes.
#[must_use]
pub fn read(at: &[u8]) -> Call {
    Call::Read { path: path(at), skip: 0, lines: None }
}

/// Reads `lines` lines of the file at `at`, after the first `skip`.
#[must_use]
pub fn read_lines(at: &[u8], skip: u32, lines: u32) -> Call {
    Call::Read { path: path(at), skip, lines: Some(lines) }
}

/// Lists the directory at `at`.
#[must_use]
pub fn list(at: &[u8]) -> Call {
    Call::List { path: path(at) }
}

/// Makes the file at `at` hold `content`.
#[must_use]
pub fn write(at: &[u8], content: &[u8]) -> Call {
    Call::Write { path: path(at), content: content.into() }
}

/// Replaces `old` with `new` in the file at `at`: its one occurrence, or every
/// one if `all`.
#[must_use]
pub fn edit(at: &[u8], old: &[u8], new: &[u8], all: bool) -> Call {
    Call::Edit { path: path(at), old: old.into(), new: new.into(), all }
}

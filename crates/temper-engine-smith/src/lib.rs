//! Pure, typed translation from the engine's owned run and call vocabulary
//! into Smith's generic agent boundary (domain/agent.md, sections 4–7;
//! domain/engine.md, section 7; smith's domain/run.md, sections 3, 5 and 10).
//!
//! The root keeps task policy, numbered calls, opaque transcripts and ordered
//! commits. This crate keeps no state and performs no IO. Its `start` takes
//! already decoded concrete history and already prepared workspace mounts from
//! the composing world; a later protocol adapter supplies the same values from
//! bytes and worker preparation. It never chooses authority or commits a call.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

mod answers;
mod calls;
mod finish;
mod forge;
mod json;
mod nested;
mod start;

pub use answers::answer;
pub use calls::{call, tools};
pub use finish::{ChangeResource, Terminal, result, turn};
pub use start::{model, section, start};

fn decimal(bytes: &[u8]) -> Result<u64, Problem> {
    if bytes.is_empty() {
        return Err(Problem::Range);
    }
    let mut value = 0_u64;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return Err(Problem::Range);
        }
        value = value.checked_mul(10).ok_or(Problem::Range)?;
        value = value.checked_add(u64::from(byte.saturating_sub(b'0'))).ok_or(Problem::Range)?;
    }
    Ok(value)
}

/// A host call that cannot be translated into the engine's declared schema.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Problem {
    Malformed,
    TooLarge,
    Missing,
    Type,
    Range,
    UnknownTool,
}

#[cfg(test)]
mod tests;

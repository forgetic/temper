//! What the agent tells whoever watches it (agent-domain.md, section 7): the
//! child domains' facts, content-free, gathered after every entry point into one
//! bounded queue the loop drains at its own pace. What does not fit is dropped
//! and counted, and nothing the domain decides depends on it.
//!
//! The run names a conversation by its token for it, and the sessions name
//! theirs by their opener's, which is the same token: the facts of a
//! conversation and of its session go together.

use alloc::boxed::Box;
use skein_lib::Token;
use temper_legacy_agent_domain_run::facts as run;
use temper_legacy_agent_domain_session as session;

/// Content the protocol projects into the channel's fact payload. The engine
/// applies capture policy; the agent's queue drops and counts overflow.
#[derive(PartialEq, Eq, Debug)]
pub enum Content {
    Text { owner: Token, text: Box<[u8]> },
    Call { owner: Token, id: Box<[u8]>, name: Box<[u8]>, input: Box<[u8]> },
    Tool { owner: Token, done: crate::tools::Done },
    Usage { owner: Token, usage: crate::llm::Usage },
}

pub(crate) fn done_bytes(done: &crate::tools::Done) -> u64 {
    use crate::tools::Done;
    match done {
        Done::Loaded { content, .. } => bytes(content),
        Done::Scanned { entries, .. } => {
            let mut held = fixed(entries.len(), size_of::<crate::tools::Entry>());
            for entry in entries {
                held = held.saturating_add(bytes(entry.name.as_bytes()));
            }
            held
        }
        Done::Exited { head, tail, .. } => bytes(head).saturating_add(bytes(tail)),
        Done::Found { hits, .. } => {
            let mut held = fixed(hits.len(), size_of::<crate::tools::Hit>());
            for hit in hits {
                held = held.saturating_add(bytes(&hit.path)).saturating_add(bytes(&hit.text));
            }
            held
        }
        Done::Stored { .. }
        | Done::Conflict { .. }
        | Done::Missing
        | Done::NotFile
        | Done::Linked
        | Done::NotDirectory
        | Done::TooLarge { .. }
        | Done::Escapes
        | Done::Failed { .. }
        | Done::TimedOut
        | Done::Cancelled => 0,
    }
}

pub(crate) fn bytes(value: &[u8]) -> u64 {
    u64::try_from(value.len()).unwrap_or(u64::MAX)
}

fn fixed(count: usize, size: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX).saturating_mul(u64::try_from(size).unwrap_or(u64::MAX))
}

/// Something that happened in a child domain.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "fixed diagnostic tails keep boundary records bounded without allocation"
)]
pub enum Fact {
    /// In the run child domain.
    Run { fact: run::Fact },
    /// In the session child domain, or its tools.
    Session { fact: session::Fact },
}

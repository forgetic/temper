//! What the tools answered, by name, as the tests count it.

use temper_agent_domain_tools::Outcome;

/// The name of the kind of `outcome`.
#[must_use]
pub fn kind(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Read { .. } => "read",
        Outcome::Listed { .. } => "listed",
        Outcome::Found { .. } => "found",
        Outcome::Written { .. } => "written",
        Outcome::Edited { .. } => "edited",
        Outcome::Exited { .. } => "exited",
        Outcome::NoMatch => "no match",
        Outcome::Ambiguous { .. } => "ambiguous",
        Outcome::Unchanged => "unchanged",
        Outcome::NotGranted => "not granted",
        Outcome::Outside => "outside",
        Outcome::ReadOnly => "read only",
        Outcome::TooLong => "too long",
        Outcome::NotFound => "not found",
        Outcome::NotFile => "not a file",
        Outcome::Linked => "linked",
        Outcome::Protected => "protected",
        Outcome::NotDirectory => "not a directory",
        Outcome::TooLarge { .. } => "too large",
        Outcome::NotRead => "not read",
        Outcome::Stale => "stale",
        Outcome::Failed { .. } => "failed",
        Outcome::TimedOut => "timed out",
        Outcome::Cancelled => "cancelled",
        Outcome::Busy => "busy",
        Outcome::NulByte => "nul byte",
    }
}

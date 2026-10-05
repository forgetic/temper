//! What a run finds in its checkout before it opens its main conversation
//! (agent-domain.md, 4.1 and 4.4). The engine never reads a repository, so the
//! run looks for itself:
//!
//! - For each repository, the start of its `AGENTS.md`, which the system text
//!   carries after the brief.
//! - For each writable repository, when the outcome spec has a change pass
//!   the checks, whether it has checks: an executable at `.temper/pre-pr`,
//!   run before a change is pushed. No executable, no checks.
//!
//! One operation at a time, in the checkout's order, each with its deadline.
//! A file that is missing, that is not text, or that io fails to read, is
//! simply not there.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{List, Time, Token};

use crate::boundary::{Place, Read, Request};
use crate::charter::{Charter, Repository, count};
use crate::limits::Limits;
use crate::outcome::ChangeSpec;

/// Where a repository's guide for LLMs is, beneath its root.
pub(crate) const GUIDE: &[u8] = b"AGENTS.md";

/// Where a repository's checks are, beneath its root.
pub(crate) const CHECKS: &[u8] = b".temper/pre-pr";

/// One thing a run looks for: in the repository at `repository` in its
/// checkout, its guide or its checks.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Step {
    pub(crate) repository: u32,
    pub(crate) look: Look,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Look {
    Guide,
    Checks,
}

/// What a run found in its checkout.
#[derive(Debug)]
pub(crate) struct Found {
    /// The guides found, in the checkout's order.
    pub(crate) guides: List<Guide>,
    /// The repositories that have checks, by their place in the checkout, in
    /// its order.
    pub(crate) checks: List<u32>,
}

/// The start of a repository's `AGENTS.md`.
#[derive(Debug)]
pub(crate) struct Guide {
    /// The repository's place in the checkout.
    pub(crate) repository: u32,
    pub(crate) text: Box<[u8]>,
    /// Whether `text` is all of the file.
    pub(crate) whole: bool,
}

impl Found {
    /// Room for what a checkout of `repositories` may hold.
    pub(crate) fn with_capacity(repositories: u32) -> Found {
        Found { guides: List::with_capacity(repositories), checks: List::with_capacity(repositories) }
    }
}

/// The first step, or the step after `after`, or `None` once there is
/// nothing left to look for.
pub(crate) fn next(charter: &Charter, after: Option<Step>) -> Option<Step> {
    let repositories = count(charter.checkout.repositories.len());
    let candidate = match after {
        None => Step { repository: 0, look: Look::Guide },
        Some(Step { repository, look: Look::Guide }) => Step { repository, look: Look::Checks },
        Some(Step { repository, look: Look::Checks }) => {
            Step { repository: repository.saturating_add(1), look: Look::Guide }
        }
    };
    let step = match candidate.look {
        Look::Guide => candidate,
        Look::Checks if wants_checks(charter, candidate.repository) => candidate,
        Look::Checks => Step { repository: candidate.repository.saturating_add(1), look: Look::Guide },
    };
    (step.repository < repositories).then_some(step)
}

/// The request that takes `step`, for the run `owner`.
pub(crate) fn request(charter: &Charter, step: Step, owner: Token, now: Time, limits: &Limits) -> Request {
    let root = repository(charter, step.repository).root;
    let deadline = now.saturating_add(limits.io_timeout);
    match step.look {
        Look::Guide => {
            let at = Place { root, path: copy_of(GUIDE) };
            Request::Read { owner, at, max: limits.guide_bytes, deadline }
        }
        Look::Checks => Request::Probe { owner, at: Place { root, path: copy_of(CHECKS) }, deadline },
    }
}

/// Keeps what reading the guide of `step` found.
pub(crate) fn guide(found: &mut Found, step: Step, read: Read, limits: &Limits) {
    assert!(step.look == Look::Guide, "a read answers a guide's step");
    match read {
        Read::Text { text, whole } => {
            assert!(count(text.len()) <= limits.guide_bytes, "io reads no more than it is asked for");
            let guide = Guide { repository: step.repository, text, whole };
            found.guides.push(guide).expect("room for a guide per repository");
        }
        Read::Missing | Read::NotText | Read::Failed => {}
    }
}

/// Keeps what looking for the checks of `step` found.
pub(crate) fn checks(found: &mut Found, step: Step, executable: bool) {
    assert!(step.look == Look::Checks, "a probe answers a checks' step");
    if executable {
        found.checks.push(step.repository).expect("room for checks per repository");
    }
}

/// Whether the run looks for the checks of the repository at `index`: it is
/// writable, and a change must pass its checks.
fn wants_checks(charter: &Charter, index: u32) -> bool {
    match charter.outcome.change {
        Some(ChangeSpec { checks: true }) => repository(charter, index).writable,
        Some(ChangeSpec { checks: false }) | None => false,
    }
}

fn repository(charter: &Charter, index: u32) -> &Repository {
    let index = usize::try_from(index).expect("a u32 fits in a usize");
    charter.checkout.repositories.get(index).expect("steps are within the checkout")
}

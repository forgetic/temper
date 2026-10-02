//! The records that cross the boundary with the brief's parent, the engine's
//! top-level model (programming-style.md, 4.5), which serves the brief's
//! reads from the forge sub-model, the notes sub-model or its configuration,
//! and puts the brief it is answered with into a run's charter. The brief
//! sub-model defines them; its parent depends on it.
//!
//! Two shapes cross it. A call in: an [`Event::Render`] carries a `reply_to`
//! and is answered by exactly one record back that echoes it:
//! [`Request::Rendered`] or [`Request::Failed`], or [`Request::Refused`] at
//! the entrance. And reads out: a [`Request::Read`] is ended by exactly one
//! [`Event::Read`] that echoes its `owner`, whether or not its brief is still
//! gathering: one that ends after its brief has answered is dropped. The
//! parent bounds each read's time and says when it failed; the brief bounds
//! its own gathering, and answers with what it has when that runs out.
//!
//! The brief knows nothing of plans, the forge's API or the workers: the
//! parent describes where a section comes from in the brief's own terms
//! ([`Source`]), and the content comes back as parts of bytes, which the
//! brief cuts to its budgets but never parses.

use alloc::boxed::Box;

use temper_lib::{ReplyTo, Token};

/// An issue or pull request of one of the deployment's repositories: the
/// repository's index in the deployment's list, and the item's number there.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Item {
    pub repository: u32,
    pub number: u64,
}

/// A commit, as the forge names it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Commit(pub [u8; 32]);

/// What a section of a brief is about (engine-model.md, section 9). Each
/// kind has a budget of its own, and its own way of being cut (see
/// [`Source`], and the crate's `cut` module).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    Item,
    Comments,
    Dependencies,
    Ci,
    Reviews,
    Pull,
    Attempts,
    Plan,
    Notes,
    Template,
}

/// Where a section's content comes from, in the brief's own terms; the
/// parent knows how to read each. The parts of each are in the order given
/// here, which is the order the cut relies on.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    /// The item and its lineage: the item first, then its parent, and so on
    /// up. The farthest ancestors are cut first, then the tail of what is
    /// left.
    Item(Item),
    /// The comments on the item after `since`, the last its record has
    /// taken (engine-model.md, 4.3), a part each, oldest first. The oldest
    /// are cut first, then the head of the oldest kept.
    Comments { item: Item, since: u64 },
    /// The outcomes of these items' runs, a part each, in this order: how
    /// results flow through a plan (5.2). Each keeps an even share, and its
    /// start.
    Dependencies(Box<[Item]>),
    /// The CI failures on exactly `head` of the item's pull request, a part
    /// per failed check with its output. Each keeps an even share, and the
    /// end of its output, where a failure shows.
    Ci { item: Item, head: Commit },
    /// The review comments on exactly `head` of the item's pull request, a
    /// part each. Each keeps an even share, and its start.
    Reviews { item: Item, head: Commit },
    /// How the item's pull request at `head` stands against its base: moved,
    /// or conflicting, and where. Its tail is cut first.
    Pull { item: Item, head: Commit },
    /// The item's earlier attempts and why they failed, a part each, oldest
    /// first. The oldest are cut first.
    Attempts(Item),
    /// The status of the plan under the goal `goal`. Its tail is cut first.
    Plan { goal: Item },
    /// The index of the notes in a run's scopes (engine-model.md, section
    /// 10): the deployment's, the repository's, and the goal's if it has
    /// one. Its tail is cut first; the notes say how many entries did not
    /// fit, within the index.
    Notes { repository: u32, goal: Option<u64> },
    /// The template at this index of the configuration. Its tail is cut
    /// first.
    Template(u32),
}

impl Source {
    /// The kind of section this source fills.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            Source::Item(_) => Kind::Item,
            Source::Comments { .. } => Kind::Comments,
            Source::Dependencies(_) => Kind::Dependencies,
            Source::Ci { .. } => Kind::Ci,
            Source::Reviews { .. } => Kind::Reviews,
            Source::Pull { .. } => Kind::Pull,
            Source::Attempts(_) => Kind::Attempts,
            Source::Plan { .. } => Kind::Plan,
            Source::Notes { .. } => Kind::Notes,
            Source::Template(_) => Kind::Template,
        }
    }
}

/// A section the parent wants in a brief: where it comes from, and whether a
/// brief without it is worth rendering. A required section that cannot be
/// read fails the brief: a repair without its failing output, or a review
/// without the pull request it reviews, would spend a run on nothing.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Wanted {
    pub source: Source,
    pub required: bool,
}

/// Which end of a part a cut keeps: its start, cutting its tail; or its end,
/// cutting its head.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Keep {
    Start,
    End,
}

/// A part of a section's content as its source has it: a comment, a
/// dependency's outcome, a failed check's output. `left` counts the bytes
/// the source left out of it to keep within the read's bounds, cut at the
/// end the read's [`Keep`] does not keep; a part left out whole has no
/// bytes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Part {
    pub bytes: Box<[u8]>,
    pub left: u64,
}

/// How a read ended.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Read {
    /// The section's content, in parts, within the read's bounds. An answer
    /// past them counts as failed.
    Got(Box<[Part]>),
    Failed,
}

/// A section of a rendered brief, in the order the parent asked for it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Section {
    pub kind: Kind,
    pub body: Body,
}

/// What a rendered section holds.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Body {
    /// Its content within its budget: its parts one after another, as their
    /// source wrote them, except where they are cut. Every cut, the source's
    /// and the brief's, is a line saying how many bytes it left out, `[N
    /// bytes cut]`, where the bytes were; adjacent cuts are one line. A cut
    /// never splits a UTF-8 sequence.
    Text(Box<[u8]>),
    /// Its content could not be read before the brief had to answer.
    Missing,
}

/// Why a brief was refused at the entrance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// No room for one more brief.
    Busy,
    /// More sections than a brief may have, or a list of more items than a
    /// source may name.
    Oversized,
}

/// parent -> brief
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Render a brief of `sections`, in this order, reading each. Answered
    /// by exactly one `Rendered` or `Failed`, or `Refused`.
    Render { reply_to: ReplyTo, sections: Box<[Wanted]> },
    /// Terminal for `Read`.
    Read { owner: Token, read: Read },
}

/// brief -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The brief of the `Render` of `reply_to`: a section for each asked, in
    /// that order, each within its kind's budget and all within the brief's;
    /// a section that could not be read in time is missing.
    Rendered { reply_to: ReplyTo, sections: Box<[Section]> },
    /// The `Render` of `reply_to` failed: the required section of this kind
    /// could not be read in time.
    Failed { reply_to: ReplyTo, missing: Kind },
    /// The `Render` of `reply_to` was refused at the entrance.
    Refused { reply_to: ReplyTo, refusal: Refusal },
    /// Read the content of `source`: at most `parts` parts and `bytes`
    /// bytes in all, a source with more cutting its parts at the end `keep`
    /// does not keep, and saying how much it left out of each.
    Read { owner: Token, source: Source, keep: Keep, parts: u32, bytes: u32 },
}

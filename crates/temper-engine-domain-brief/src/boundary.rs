//! The records that cross the boundary with the brief's parent, the engine's
//! root domain (programming-model.md, 4.5), which serves the brief's
//! reads from the forge child domain, the notes child domain or its
//! configuration, and puts the brief it is answered with into a run's charter.
//! The brief child domain defines them; its parent depends on it.
//!
//! Two shapes cross it. A call in: an [`Event::Render`] carries a `reply_to`
//! and is answered by exactly one record back that echoes it:
//! [`Request::Rendered`] or [`Request::Failed`], or [`Request::Refused`] at
//! the entrance. And reads out: a [`Request::Read`] is ended by exactly one
//! [`Event::Read`] that echoes its `owner`, whether or not its brief is still
//! gathering: one that ends after its brief has answered is dropped. The
//! brief bounds its own gathering, and answers with what it has when that
//! runs out; the parent bounds each read's time, at most the brief's time to
//! gather (`Limits::gather`), and says when it failed, so that a read
//! outliving its brief holds its room no longer than that.
//!
//! A brief refused as busy is not asked again by the brief: it tells its
//! parent once when there is room again ([`Request::Room`]), which carries
//! no answer.
//!
//! The brief knows nothing of plans, the forge's API or the workers: the
//! parent describes where a section comes from in the brief's own terms
//! ([`Source`]), and the content comes back as parts of bytes, which the
//! brief cuts to its budgets but never parses.

use alloc::boxed::Box;

use skein_lib::{ReplyTo, Token};

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

/// What a section of a brief is about (engine-domain.md, section 9). Each
/// kind has a budget of its own, and its own way of being cut (see
/// [`Source`], and the crate's `cut` module).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    Task,
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
    /// The task's spec and contract, followed by its requester lineage, nearest first. Later
    /// ancestors go first when cut. The parent supplies bounded concrete text; no child types cross
    /// this boundary.
    Task { task: u64 },
    /// The item and its lineage: the item first, then its parent, and so on
    /// up. The farthest ancestors are cut first, then the tail of what is
    /// left.
    Item(Item),
    /// The comments on the item after `since`, the last its record has
    /// taken (engine-domain.md, 4.3), a part each, oldest first. The oldest
    /// are cut first, then the head of the oldest kept.
    Comments { item: Item, since: u64 },
    /// The outcomes of these items' runs, a part each, in this order: how
    /// results flow through a plan (5.2). Each keeps an even share, and its
    /// start. A list longer than a source may name is cut to its first
    /// items at the entrance, and the section ends with a line saying how
    /// many were left out, `[N items cut]`.
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
    /// The index of the notes in a run's scopes (engine-domain.md, section
    /// 10): the deployment's, the repository's, and its goal's if it has
    /// one, in the goal's repository. A part per line, narrowest scope
    /// first, and last a part saying how many entries did not fit (empty if
    /// none). It is cut by whole lines, from its last, and keeps its last
    /// part.
    Notes { repository: u32, goal: Option<Item> },
    /// The template at this index of the configuration. Its tail is cut
    /// first.
    Template(u32),
}

impl Source {
    /// The kind of section this source fills.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            Source::Task { .. } => Kind::Task,
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

/// How a source with more than a read may bring cuts it to the read's
/// bounds, keeping what the section keeps longest, and never splitting a
/// UTF-8 sequence. Whole parts it leaves out are told in the `left` of the
/// part next to them, or of an empty part in their place.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fit {
    /// Its content as one run of bytes from the end [`Keep`] names: the
    /// first parts and the start of the last one kept, or the last parts
    /// and the end of the first one kept.
    Run,
    /// Each part to an even share of the read's bytes (its bytes over the
    /// number of parts it brings), keeping the end [`Keep`] names; past the
    /// read's parts, those farthest from that end are left out.
    Each,
    /// Whole parts from the start, and the last part, which tells what did
    /// not fit and is kept: the notes' index, a part per line.
    Lines,
}

/// A part of a section's content as its source has it: a comment, a
/// dependency's outcome, a failed check's output, a line of an index.
/// `left` counts the bytes the source left out at the end of it the read's
/// [`Keep`] does not keep, to keep within the read's bounds: of this part,
/// and of the parts beyond that end it left out whole. A part may be left
/// with no bytes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Part {
    pub bytes: Box<[u8]>,
    pub left: u64,
}

/// How a read ended.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Read {
    /// The section's content, in parts, within the read's bounds. An answer
    /// past them counts as unread, oversized.
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
    /// Its content could not be read before the brief answered, and why.
    Missing(Unread),
}

/// Why a section was not read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Unread {
    /// Its read failed.
    Failed,
    /// Its read had not ended by the brief's deadline.
    Late,
    /// Its read brought more than a read may.
    Oversized,
}

/// Why a brief was refused at the entrance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// No room for one more brief, or for the reads of one: a
    /// [`Request::Room`] says when there is again.
    Busy,
    /// More sections than a brief may have.
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
    /// could not be read, and why.
    Failed { reply_to: ReplyTo, missing: Kind, why: Unread },
    /// The `Render` of `reply_to` was refused at the entrance.
    Refused { reply_to: ReplyTo, refusal: Refusal },
    /// There is room for a brief again, after one or more were refused as
    /// busy: once, as soon as there is.
    Room,
    /// Read the content of `source`: at most `parts` parts and `bytes`
    /// bytes in all, a source with more cutting it as `fit` says, keeping
    /// the end `keep` names.
    Read { owner: Token, source: Source, keep: Keep, fit: Fit, parts: u32, bytes: u32 },
}

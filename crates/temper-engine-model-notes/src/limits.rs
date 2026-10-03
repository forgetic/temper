use alloc::boxed::Box;

use temper_lib::{Id, List, Map, Queue, Set, Slab};

use crate::boundary::{Entry, Listed, Reference, Scope};
use crate::call::{Call, Wanted};
use crate::facts::Fact;
use crate::kept::{Kept, Known};
use crate::model::Op;

/// The notes sub-model's limits (programming-model.md, section 7), handed by
/// its parent to every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Scopes whose index is kept at once. A call that needs one more, when
    /// every scope kept is in use, is refused as busy.
    pub scopes: u32,
    /// Entries a scope's index holds. Pages past them are left out of it.
    pub entries: u32,
    /// The most bytes of an entry's name, of its description (and of a
    /// query), and of its body.
    pub name_bytes: u32,
    pub description_bytes: u32,
    pub body_bytes: u32,
    /// The most references an entry carries.
    pub references: u32,
    /// Calls in flight at once. A call past them is refused as busy.
    pub calls: u32,
    /// The most lines an index or a search answers with, and entries a
    /// recall reads.
    pub lines: u32,
    pub recalled: u32,
    /// Facts kept until the parent drains them. Beyond them, facts are
    /// dropped and counted.
    pub facts: u32,
}

/// The operations in flight at once: one for each scope kept and each call,
/// twice over, as one may end and the next start in an iteration before the
/// reclaim point frees the first.
pub(crate) fn ops(limits: &Limits) -> Option<u32> {
    limits.scopes.checked_add(limits.calls)?.checked_mul(2)
}

/// The most memory the model holds under `limits`, in bytes (6.3), or `None`
/// if it does not fit a `u64`.
///
/// It counts the containers, their bookkeeping included, and the payloads,
/// not allocator overhead: each scope kept with a full index, as many pages
/// lacking and as many for its pass to read, and its queues of calls, with
/// one scope's queues more for an eviction; each call holding the most one may (a
/// note's page, or a recall's entries); the operations in flight; and the
/// event in hand, a listing of a scope's pages or a page read. What goes out
/// in a request is moved out in the step that makes it, and is its
/// receiver's to count.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.scopes == 0 || limits.calls == 0 {
        return None;
    }
    let name = u64::from(limits.name_bytes);
    let description = u64::from(limits.description_bytes);
    let references = List::<Reference>::worst_case(limits.references)?;
    let page = description.checked_add(u64::from(limits.body_bytes))?.checked_add(references)?;
    // A scope kept.
    let entries = u64::from(limits.entries);
    let known = name.checked_add(description)?.checked_add(references)?;
    let index = Map::<Box<[u8]>, Known>::worst_case(limits.entries)?.checked_add(entries.checked_mul(known)?)?;
    // The pages lacking, and those its pass reads: as many again.
    let lacking =
        Set::<Box<[u8]>>::worst_case(limits.entries)?.checked_add(entries.checked_mul(name)?)?.checked_mul(2)?;
    let queues = Queue::<Id<Call>>::worst_case(limits.calls)?.checked_mul(2)?;
    let kept = index.checked_add(lacking)?.checked_add(queues)?.checked_add(name)?;
    // A scope evicted is replaced in place, the new one's queues made while
    // the old one is still whole.
    let scopes = Slab::<Kept>::worst_case(limits.scopes)?
        .checked_add(Map::<Scope, Id<Kept>>::worst_case(limits.scopes)?)?
        .checked_add(u64::from(limits.scopes).checked_mul(kept)?)?
        .checked_add(queues)?;
    // A call: a search's query; a note's name and page, and what of it the
    // index learns once it is written; a recall's query, the names it wants
    // and the entries it read.
    let note = name.checked_mul(2)?.checked_add(page)?.checked_add(description)?.checked_add(references)?;
    let recalled = u64::from(limits.recalled);
    let wanted = List::<Wanted>::worst_case(limits.recalled)?.checked_add(recalled.checked_mul(name)?)?;
    let read =
        List::<Entry>::worst_case(limits.recalled)?.checked_add(recalled.checked_mul(name.checked_add(page)?)?)?;
    let recall = description.checked_add(wanted)?.checked_add(read)?;
    let call = note.max(recall).max(description);
    let calls = Slab::<Call>::worst_case(limits.calls)?.checked_add(u64::from(limits.calls).checked_mul(call)?)?;
    let ops = Slab::<Op>::worst_case(ops(limits)?)?;
    let ready = Queue::<Id<Call>>::worst_case(limits.calls)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    // The event in hand: a listing, whose names the index takes or drops,
    // with the names of the entries it no longer lists; or a page read.
    let listing = List::<Listed>::worst_case(limits.entries)?
        .checked_add(entries.checked_mul(name)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(limits.entries)?)?
        .checked_add(entries.checked_mul(name)?)?;
    let hand = listing.max(page);
    scopes.checked_add(calls)?.checked_add(ops)?.checked_add(ready)?.checked_add(facts)?.checked_add(hand)
}

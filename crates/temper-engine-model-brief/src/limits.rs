use temper_lib::{Deadlines, Duration, Id, List, Queue, Slab};

use crate::boundary::{Item, Kind, Part, Section, Wanted};
use crate::brief::{Brief, Reading, Slot};
use crate::cut::FLOOR;
use crate::facts::Fact;

/// The brief sub-model's limits (programming-style.md, section 7), handed by
/// its parent to every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Briefs gathered at once. A brief past them, or past the room for
    /// reads (see [`worst_case`]), is refused as busy.
    pub briefs: u32,
    /// Sections a brief may have: a brief past them is refused as
    /// oversized. And items a source may name: a longer list is cut.
    pub sections: u32,
    pub items: u32,
    /// The most parts, and bytes in all, a read may bring: what a brief
    /// reads for a section (a kind cut by its tail or head reads no more
    /// than its budget). A source with more cuts its parts to fit; an
    /// answer past them counts as unread.
    pub parts: u32,
    pub read_bytes: u32,
    /// Each kind's budget, and the brief's in all: no section is longer than
    /// its kind's, and all of a brief's together no longer than
    /// `brief_bytes`. Each is room for at least a cut line and an items
    /// line, and the brief's for both in every section.
    pub budgets: Budgets,
    pub brief_bytes: u32,
    /// How long a brief may take to gather its sections: past it, it is
    /// rendered with what it has. Not zero: a brief admitted in an
    /// iteration does not end in it. The parent bounds each read's time to
    /// at most this.
    pub gather: Duration,
    /// Facts kept until the parent drains them. Beyond them, facts are
    /// dropped and counted.
    pub facts: u32,
}

/// The most bytes of a section of each kind (engine-model.md, section 9).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Budgets {
    pub item: u32,
    pub comments: u32,
    pub dependencies: u32,
    pub ci: u32,
    pub reviews: u32,
    pub pull: u32,
    pub attempts: u32,
    pub plan: u32,
    pub notes: u32,
    pub template: u32,
}

/// The budget of a section of `kind`.
pub(crate) fn budget(budgets: &Budgets, kind: Kind) -> u32 {
    match kind {
        Kind::Item => budgets.item,
        Kind::Comments => budgets.comments,
        Kind::Dependencies => budgets.dependencies,
        Kind::Ci => budgets.ci,
        Kind::Reviews => budgets.reviews,
        Kind::Pull => budgets.pull,
        Kind::Attempts => budgets.attempts,
        Kind::Plan => budgets.plan,
        Kind::Notes => budgets.notes,
        Kind::Template => budgets.template,
    }
}

/// The kinds, in the order a brief usually has them.
pub(crate) const KINDS: [Kind; 10] = [
    Kind::Item,
    Kind::Comments,
    Kind::Dependencies,
    Kind::Ci,
    Kind::Reviews,
    Kind::Pull,
    Kind::Attempts,
    Kind::Plan,
    Kind::Notes,
    Kind::Template,
];

/// The reads in flight at once: one for each section of each brief
/// gathering, and as many again of briefs that have answered before their
/// reads ended. A render is admitted only with room for as many reads as a
/// brief may have.
pub(crate) fn reads(limits: &Limits) -> Option<u32> {
    limits.briefs.checked_mul(limits.sections)?.checked_mul(2)
}

/// The slots each slab has: room for what is live, and for as many retired
/// in the same iteration, before the reclaim point frees them. Nothing
/// admitted in an iteration ends in it, so no more than were live retire.
pub(crate) fn slots(live: u32) -> Option<u32> {
    live.checked_mul(2)
}

/// Whether every budget holds a section's lines, and the brief's those of
/// each of its sections: a section cut to nothing still says what it left
/// out.
fn announceable(limits: &Limits) -> bool {
    let floor = u32::try_from(FLOOR).expect("the lines are short");
    for kind in KINDS {
        if budget(&limits.budgets, kind) < floor {
            return false;
        }
    }
    match limits.sections.checked_mul(floor) {
        Some(lines) => limits.brief_bytes >= lines,
        None => false,
    }
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64`, or the limits cannot work: no brief, section
/// or part, no time to gather, or a budget too small to say what it cut.
///
/// It counts the containers, their bookkeeping included, and the payloads,
/// not allocator overhead: each brief gathering with every section read in
/// full; the reads in flight; and the event in hand, a render's sections
/// each naming as many items as a source may, with a list cut to them, or a
/// brief being rendered, at its budget. What goes out in a request is moved
/// out in the step that makes it, and is its receiver's to count; a longer
/// list than a source may name is the parent's, held only while the render
/// steps.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.briefs == 0
        || limits.sections == 0
        || limits.parts == 0
        || limits.gather.as_nanos() == 0
        || !announceable(limits)
    {
        return None;
    }
    let sections = u64::from(limits.sections);
    let read = List::<Part>::worst_case(limits.parts)?.checked_add(u64::from(limits.read_bytes))?;
    let brief = List::<Slot>::worst_case(limits.sections)?.checked_add(sections.checked_mul(read)?)?;
    let briefs =
        Slab::<Brief>::worst_case(slots(limits.briefs)?)?.checked_add(u64::from(limits.briefs).checked_mul(brief)?)?;
    let reads = Slab::<Reading>::worst_case(slots(reads(limits)?)?)?;
    let alarms = Deadlines::<Id<Brief>>::worst_case(limits.briefs)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    let list = List::<Item>::worst_case(limits.items)?;
    let render =
        List::<Wanted>::worst_case(limits.sections)?.checked_add(sections.checked_mul(list)?)?.checked_add(list)?;
    let rendered = List::<Section>::worst_case(limits.sections)?.checked_add(u64::from(limits.brief_bytes))?;
    let hand = render.max(rendered);
    briefs.checked_add(reads)?.checked_add(alarms)?.checked_add(facts)?.checked_add(hand)
}

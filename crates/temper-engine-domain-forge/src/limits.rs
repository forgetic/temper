use alloc::boxed::Box;
use core::mem::size_of;

use skein_lib::{Deadlines, Duration, Id, List, Map, Queue, Slab};

use crate::api::{Answer, Comment, PageName, Pull, Remark, Review, Status, Summary};
use crate::boundary::{Item, Reviewed};
use crate::calls::Call;
use crate::domain::Alarm;
use crate::facts::Fact;
use crate::items::{Entry, Held};
use crate::reads::Fetch;
use crate::scans::Scan;
use crate::writes::{Lane, Writing};

/// The forge child domain's limits (programming-model.md, section 7), handed by
/// its parent to every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// The deployment's repositories.
    pub repositories: u32,
    /// Items held in the working set at once, those leaving included. Beyond
    /// them, new work waits on the forge.
    pub items: u32,
    /// Labels an item carries or a write sets; users or items a write names
    /// as reviewers or dependencies.
    pub labels: u32,
    pub members: u32,
    /// News an item's inbox holds until the parent takes it, the last place
    /// kept for its pull request's state and verdicts. Beyond them, the rest
    /// waits on the forge until there is room. At least two.
    pub inbox: u32,
    /// The reviewers whose verdicts on a pull request's head the working set
    /// holds; those beyond them are not counted.
    pub reviewers: u32,
    /// Fresh reads in hand at once. A read beyond them is refused as busy.
    pub reads: u32,
    /// Writes in hand at once, those waiting their turn included. A write
    /// beyond them is refused as busy.
    pub writes: u32,
    /// Calls out to the forge at once.
    pub calls: u32,
    /// The most entries an answer brings: items or comments of a page, the
    /// reviews or statuses of a pull request, wiki page names. The protocol
    /// layer asks for no more.
    pub page: u32,
    /// The most bytes of a label, a key, a branch or a wiki page's name.
    pub name_bytes: u32,
    /// The most bytes of a title (and of a status's description and its
    /// URL), and of a body or a wiki page's content: of what a write carries,
    /// and of what an answer brings, which the protocol layer cuts to them
    /// (the child domain carries text and never reads it).
    pub title_bytes: u32,
    pub body_bytes: u32,
    /// The request budget: the calls made in a window of `window`, which
    /// starts with the first call after the last window ended. At least one.
    /// Of them, `reserve` are kept for keeping up and the slow pass while
    /// they wait: fewer than `rate`.
    pub rate: u32,
    pub window: Duration,
    pub reserve: u32,
    /// Keeping up: a repository's changes are listed every `poll`, and
    /// `hinted` after a webhook, but never sooner than `hinted` after the last
    /// listing began, which must be more than the forge's resolution.
    pub poll: Duration,
    pub hinted: Duration,
    /// The resolution of the forge's times (a second on Forgejo): an item
    /// listed at a time this close to a pass's start may have changed during
    /// it.
    pub resolution: Duration,
    /// The slow pass lists a page of a repository's open items every `slow`,
    /// and reads at most `probes` of its items for a record. A pull request
    /// held is read again at least every `slow`, more often when it moves.
    pub slow: Duration,
    pub probes: u32,
    /// A call that failed for a while is tried again after a backoff drawn
    /// between `backoff` and twice it, doubling with each attempt up to
    /// `backoff_max`; a read or write gives up after `attempts` of them.
    pub backoff: Duration,
    pub backoff_max: Duration,
    pub attempts: u32,
    /// The longest a call may still take effect after it went out: the
    /// protocol layer's deadline for an answer, and what the forge may take
    /// to act on what reached it. A write whose call timed out makes no call
    /// before then, and holds its lane until then if it gives up; and a new
    /// domain makes none until this long after its first moment, so nothing
    /// an engine before it asked for lands after it has read the forge.
    pub lifetime: Duration,
    /// Facts kept until the parent drains them. Beyond them, facts are
    /// dropped and counted.
    pub facts: u32,
}

/// The calls in hand at most, queued or out: one per fresh read, write, item
/// held, and repository listing, and one more per repository for the slow
/// pass. `None` if they do not fit a `u32`.
pub(crate) fn calls(limits: &Limits) -> Option<u32> {
    let listings = limits.repositories.checked_mul(2)?;
    limits.reads.checked_add(limits.writes)?.checked_add(limits.items)?.checked_add(listings)
}

/// The alarms armed at most: the first moment's, the budget's two, one per
/// repository for each of its passes, and one per item, read and write for a
/// backoff or a hold.
pub(crate) fn alarms(limits: &Limits) -> Option<u32> {
    let passes = limits.repositories.checked_mul(2)?;
    3_u32.checked_add(passes)?.checked_add(limits.items)?.checked_add(limits.reads)?.checked_add(limits.writes)
}

/// The most bytes a set of labels holds, its pointers included.
pub(crate) fn labels_bytes(limits: &Limits) -> Option<u64> {
    let each = u64::from(limits.name_bytes).checked_add(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
    u64::from(limits.labels).checked_mul(each)
}

/// The most bytes a write holds besides itself: a title, a body, labels, the
/// users or items it names, and two names (a key, or a pull request's
/// branches, or a page's name); and the labels it removes, made as its call
/// goes out.
fn write_bytes(limits: &Limits) -> Option<u64> {
    let names = u64::from(limits.name_bytes).checked_mul(2)?;
    u64::from(limits.title_bytes)
        .checked_add(u64::from(limits.body_bytes))?
        .checked_add(labels_bytes(limits)?.checked_mul(2)?)?
        .checked_add(u64::from(limits.members).checked_mul(size(size_of::<u64>())?)?)?
        .checked_add(names)
}

/// The most bytes an answer brings in: a page of items, an item and a page
/// of its comments, a pull request, a page of its reviews, of statuses or of
/// a review's inline comments, a page of wiki page names, or a wiki page.
fn answer_bytes(limits: &Limits) -> Option<u64> {
    let page = u64::from(limits.page);
    let name = u64::from(limits.name_bytes);
    let text = u64::from(limits.title_bytes).checked_add(u64::from(limits.body_bytes))?;
    let summary =
        size(size_of::<Summary>())?.checked_add(labels_bytes(limits)?)?.checked_add(text)?.checked_add(name)?;
    let items = page.checked_mul(summary)?;
    let comment = size(size_of::<Comment>())?.checked_add(u64::from(limits.body_bytes))?.checked_add(name)?;
    let item = summary.checked_add(page.checked_mul(comment)?)?;
    let review = size(size_of::<Review>())?.checked_add(u64::from(limits.body_bytes))?.checked_add(name)?;
    let reviews = page.checked_mul(review)?;
    let status =
        size(size_of::<Status>())?.checked_add(name)?.checked_add(u64::from(limits.title_bytes).checked_mul(2)?)?;
    let statuses = page.checked_mul(status)?;
    let remark = size(size_of::<Remark>())?.checked_add(name)?.checked_add(u64::from(limits.body_bytes))?;
    let remarks = page.checked_mul(remark)?;
    let pull = size(size_of::<Pull>())?.checked_add(name.checked_mul(2)?)?;
    let pages = page.checked_mul(size(size_of::<PageName>())?.checked_add(name)?)?.checked_add(name)?;
    let wiki = name.checked_add(u64::from(limits.body_bytes))?;
    items
        .max(item)
        .max(pull)
        .max(reviews)
        .max(statuses)
        .max(remarks)
        .max(pages)
        .max(wiki)
        .checked_add(size(size_of::<Answer>())?)
}

fn size(bytes: usize) -> Option<u64> {
    u64::try_from(bytes).ok()
}

/// The most memory the domain holds under `limits`, in bytes (6.3), or `None`
/// if it does not fit a `u64`, or the limits are not ones it can run under.
///
/// It counts the containers, their bookkeeping included, and the payloads,
/// not allocator overhead: each item's labels and inbox, each write's
/// payload, each read's names, each repository's slow-pass candidates, the
/// labels of its configuration, and the answer a step takes in, which it
/// holds until the step ends. What it hands up (an answer, a view, labels) is
/// made in the step that hands it, and is its receiver's to count.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.rate == 0 || limits.calls == 0 || limits.page == 0 || limits.repositories == 0 {
        return None;
    }
    // A listing as soon as a resolution after the last would settle nothing;
    // an inbox that keeps its last place holds a comment only with two; a
    // reserve of the whole budget leaves the parent none.
    if limits.hinted <= limits.resolution || limits.inbox < 2 || limits.reserve >= limits.rate {
        return None;
    }
    let calls = calls(limits)?;
    let held = Slab::<Call>::worst_case(calls)?.checked_add(Queue::<Id<Call>>::worst_case(calls)?.checked_mul(4)?)?;
    // Its labels, inbox, the verdicts on its pull request's head and those
    // being read, and its base's name.
    let item = labels_bytes(limits)?
        .checked_add(Queue::<Held>::worst_case(limits.inbox)?)?
        .checked_add(List::<Reviewed>::worst_case(limits.reviewers)?.checked_mul(2)?)?
        .checked_add(u64::from(limits.name_bytes))?;
    let items = Slab::<Entry>::worst_case(limits.items)?
        .checked_add(u64::from(limits.items).checked_mul(item)?)?
        .checked_add(Map::<Item, Id<Entry>>::worst_case(limits.items)?.checked_mul(2)?)?;
    let writes = Slab::<Writing>::worst_case(limits.writes)?
        .checked_add(u64::from(limits.writes).checked_mul(write_bytes(limits)?)?)?
        .checked_add(Map::<Lane, Id<Writing>>::worst_case(limits.writes)?)?;
    let names = u64::from(limits.name_bytes).checked_mul(2)?;
    let reads = Slab::<Fetch>::worst_case(limits.reads)?.checked_add(u64::from(limits.reads).checked_mul(names)?)?;
    let candidates = List::<u64>::worst_case(limits.page)?;
    let scans = List::<Scan>::worst_case(limits.repositories)?
        .checked_add(u64::from(limits.repositories).checked_mul(candidates)?)?;
    let alarms = Deadlines::<Alarm>::worst_case(alarms(limits)?)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    // The two labels configured and those projected, and the labels owned: a
    // copy of the three.
    let owned = Limits { labels: limits.labels.checked_add(2)?, ..*limits };
    let config = names.checked_add(labels_bytes(limits)?)?.checked_add(labels_bytes(&owned)?)?;
    held.checked_add(items)?
        .checked_add(answer_bytes(limits)?)?
        .checked_add(writes)?
        .checked_add(reads)?
        .checked_add(scans)?
        .checked_add(alarms)?
        .checked_add(facts)?
        .checked_add(config)
}

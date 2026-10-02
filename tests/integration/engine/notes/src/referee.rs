//! What the notes' scenarios expect, held by a referee (testing-pyramid.md,
//! 5.2) that sees what the wiki sees (its pages changing, the listings it
//! serves) and what the notes answer, never the notes' state:
//!
//! - no index or search line names a page that was deleted before its scope
//!   was last read, unless the page was written again since: the listing the
//!   notes last took in, as the wiki served it;
//! - a recall answers each entry with the page's content as it was at some
//!   moment between the recall's asking and its answer: read afresh, never
//!   from what the notes kept;
//! - every recall is answered, within a bound the world sets.

use std::collections::{BTreeMap, BTreeSet};

use temper_engine_model_notes::{Entry, Page, Scope};
use temper_lib::Duration;
use temper_world::{Expectations, Judge};

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The wiki's page `name` of `scope` changed: made or edited, to `page`,
    /// or deleted.
    Changed { scope: Scope, name: Vec<u8>, page: Option<Page> },
    /// The wiki served the listing `listing` of `scope`.
    Served { listing: u64, scope: Scope },
    /// The notes took the listing `listing` in.
    TakenIn { listing: u64 },
    /// The notes answered an index or a search with lines naming these
    /// pages.
    Lines { lines: Vec<(Scope, Vec<u8>)> },
    /// The parent asked the recall `call`.
    Asked { call: u64 },
    /// The recall `call` was answered with `entries`, or refused with none.
    Recalled { call: u64, entries: Vec<Entry> },
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// The recall the parent names so is answered.
    Recall(u64),
}

/// This world injects nothing of its own.
#[derive(Debug)]
pub enum Stimulus {}

/// The expectations of the notes' world.
#[derive(Debug)]
pub struct Notes {
    /// How long a recall may take to be answered.
    within: Duration,
    /// Counts the observations, which orders them.
    seen: u64,
    /// Each page's history: what it held from each observation on.
    pages: BTreeMap<(Scope, Vec<u8>), History>,
    /// The listings served and not yet taken in: their scope, when they
    /// were served, and the pages there were then.
    served: BTreeMap<u64, Listing>,
    /// The listing of each scope the notes last took in.
    read: BTreeMap<Scope, Listing>,
    /// The recalls asked and not answered, and when they were asked.
    asked: BTreeMap<u64, u64>,
    /// Lines and entries judged.
    pub lines: u64,
    pub entries: u64,
}

/// What a page held, from each observation on: its content, or none once
/// deleted.
type History = Vec<(u64, Option<Page>)>;

#[derive(Debug)]
struct Listing {
    scope: Scope,
    at: u64,
    pages: BTreeSet<Vec<u8>>,
}

impl Notes {
    /// Expectations under which every recall is answered `within` its
    /// asking.
    #[must_use]
    pub fn new(within: Duration) -> Notes {
        Notes {
            within,
            seen: 0,
            pages: BTreeMap::new(),
            served: BTreeMap::new(),
            read: BTreeMap::new(),
            asked: BTreeMap::new(),
            lines: 0,
            entries: 0,
        }
    }

    /// Whether the page `name` of `scope` was written since `at`.
    fn written_since(&self, scope: Scope, name: &[u8], at: u64) -> bool {
        let Some(history) = self.pages.get(&(scope, name.to_vec())) else {
            return false;
        };
        history.iter().any(|(when, page)| *when > at && page.is_some())
    }

    /// Whether the page `name` of `scope` held `page` at some moment from
    /// `from` on.
    fn held(&self, scope: Scope, name: &[u8], page: &Page, from: u64) -> bool {
        let Some(history) = self.pages.get(&(scope, name.to_vec())) else {
            return false;
        };
        let mut then = None;
        for (when, held) in history {
            if *when <= from {
                then = held.as_ref();
            } else if held.as_ref() == Some(page) {
                return true;
            }
        }
        then == Some(page)
    }
}

impl Expectations for Notes {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        self.seen += 1;
        let at = self.seen;
        match seen {
            Seen::Changed { scope, name, page } => self.pages.entry((scope, name)).or_default().push((at, page)),
            Seen::Served { listing, scope } => {
                let pages = self
                    .pages
                    .iter()
                    .filter(|((of, _), history)| *of == scope && history.last().is_some_and(|(_, page)| page.is_some()))
                    .map(|((_, name), _)| name.clone())
                    .collect();
                self.served.insert(listing, Listing { scope, at, pages });
            }
            Seen::TakenIn { listing } => {
                let listing = self.served.remove(&listing).expect("a listing is taken in once it was served");
                self.read.insert(listing.scope, listing);
            }
            Seen::Lines { lines } => {
                for (scope, name) in lines {
                    self.lines += 1;
                    let Some(listing) = self.read.get(&scope) else {
                        continue;
                    };
                    let fresh = listing.pages.contains(&name) || self.written_since(scope, &name, listing.at);
                    judge.check(
                        fresh,
                        format_args!(
                            "no line names a page deleted before its scope was last read: {scope:?} {}",
                            String::from_utf8_lossy(&name)
                        ),
                    );
                }
            }
            Seen::Asked { call } => {
                self.asked.insert(call, at);
                judge.expect(Expected::Recall(call), self.within);
            }
            Seen::Recalled { call, entries } => {
                judge.meet(&Expected::Recall(call));
                let asked = self.asked.remove(&call).expect("a recall is answered once it was asked");
                for entry in entries {
                    self.entries += 1;
                    let current = self.held(entry.scope, &entry.name, &entry.page, asked);
                    judge.check(
                        current,
                        format_args!(
                            "a recall answers with the page as it was meanwhile: {:?} {}",
                            entry.scope,
                            String::from_utf8_lossy(&entry.name)
                        ),
                    );
                }
            }
        }
    }
}

//! What the notes' scenarios expect, held by a referee (testing.md,
//! 5.2) that sees what the wiki sees (its pages changing, the listings it
//! serves) and what the notes answer, never the notes' state:
//!
//! - no index or search line names a page that was deleted before its scope
//!   was last read, unless the page was written again since: the listing the
//!   notes last took in, as the wiki served it;
//! - a line says what its page held at some moment: its description, who
//!   wrote it and what it refers to;
//! - a recall answers each entry with the page's content as it was at some
//!   moment between the recall's asking and its answer, read afresh, never
//!   from what the notes kept; and a recall by name that answers nothing,
//!   with no read failed, asked for a page absent at some moment meanwhile;
//! - every call is answered, within a bound the world sets.

use std::collections::{BTreeMap, BTreeSet};

use skein_lib::Duration;
use temper_engine_domain_notes::{Entry, Line, Page, Scope};
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
    /// The notes answered an index or a search with these lines.
    Lines { lines: Vec<Line> },
    /// The parent made the call `call`: a recall by name says which page.
    Asked { call: u64, page: Option<(Scope, Vec<u8>)> },
    /// The call `call` was answered, or refused, other than a recall's
    /// answer.
    Answered { call: u64 },
    /// The recall `call` was answered with `entries`, and so many pages it
    /// could not read.
    Recalled { call: u64, entries: Vec<Entry>, failed: u32 },
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// The call the parent names so is answered.
    Call(u64),
}

/// This world injects nothing of its own.
#[derive(Debug)]
pub enum Stimulus {}

/// The expectations of the notes' world.
#[derive(Debug)]
pub struct Notes {
    /// How long a call may take to be answered.
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
    /// The calls asked and not answered: when they were asked, and the page
    /// a recall by name asked for.
    asked: BTreeMap<u64, (u64, Option<Named>)>,
    /// Lines and entries judged.
    pub lines: u64,
    pub entries: u64,
}

/// A page, by its scope and name.
type Named = (Scope, Vec<u8>);

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
    /// Expectations under which every call is answered `within` its asking.
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

    /// Whether the page `name` of `scope` was absent at some moment from
    /// `from` on.
    fn absent(&self, scope: Scope, name: &[u8], from: u64) -> bool {
        let Some(history) = self.pages.get(&(scope, name.to_vec())) else {
            return true;
        };
        let mut then = true;
        for (when, held) in history {
            if *when <= from {
                then = held.is_none();
            } else if held.is_none() {
                return true;
            }
        }
        then
    }

    /// Whether the page a line names held what the line says at some
    /// moment.
    fn says(&self, line: &Line) -> bool {
        let Some(history) = self.pages.get(&(line.scope, line.name.to_vec())) else {
            return false;
        };
        history.iter().any(|(_, held)| {
            held.as_ref().is_some_and(|page| {
                page.description == line.description && page.author == line.author && page.references == line.references
            })
        })
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
                for line in lines {
                    self.lines += 1;
                    let (scope, name) = (line.scope, &line.name);
                    judge.check(
                        self.says(&line),
                        format_args!(
                            "a line says what its page held: {scope:?} {} {:?}",
                            String::from_utf8_lossy(name),
                            String::from_utf8_lossy(&line.description)
                        ),
                    );
                    let Some(listing) = self.read.get(&scope) else {
                        continue;
                    };
                    let fresh = listing.pages.contains(&**name) || self.written_since(scope, name, listing.at);
                    judge.check(
                        fresh,
                        format_args!(
                            "no line names a page deleted before its scope was last read: {scope:?} {}",
                            String::from_utf8_lossy(name)
                        ),
                    );
                }
            }
            Seen::Asked { call, page } => {
                self.asked.insert(call, (at, page));
                judge.expect(Expected::Call(call), self.within);
            }
            Seen::Answered { call } => {
                judge.meet(&Expected::Call(call));
                self.asked.remove(&call).expect("a call is answered once it was asked");
            }
            Seen::Recalled { call, entries, failed } => {
                judge.meet(&Expected::Call(call));
                let (asked, page) = self.asked.remove(&call).expect("a recall is answered once it was asked");
                if let Some((scope, name)) = page
                    && entries.is_empty()
                    && failed == 0
                {
                    judge.check(
                        self.absent(scope, &name, asked),
                        format_args!(
                            "a recall that found nothing asked for a page absent meanwhile: {scope:?} {}",
                            String::from_utf8_lossy(&name)
                        ),
                    );
                }
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

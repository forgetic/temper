//! What the forge world's scenarios expect, held by a referee
//! (testing.md, 5.2) that sees what the fake forge sees (every change,
//! by whom), the calls the child domain makes and the refusals it hears, the
//! writes the parent plans and how they end, the pull requests it links, and
//! what the child domain tells the parent; never the child domain's state:
//!
//! - **No write the parent did not plan.** Every change the engine's user
//!   makes on the forge is one the parent asked for, while it is in hand or
//!   for as long after as a call of it may still land: an issue or a comment
//!   of a key it planned, a record of an item it planned one for, labels it
//!   planned for that item, a pull request for a branch it planned, a review
//!   of a key it planned, reviewers and dependencies it planned, a merge, a
//!   close, a branch deleted, a wiki page written.
//! - **No creation made twice.** One issue per key, one comment per key, one
//!   review per key, one pull request per branch, one record per item (a
//!   person may delete it, and then it is posted again), whatever the
//!   faults, late landings and restarts.
//! - **Labels end as the last set written.** The engine adds and removes
//!   only the labels it owns, each change part of a set the parent planned,
//!   in the order it planned them, never an older set after a newer one; and
//!   once the world settles, the labels the engine owns on an item whose last
//!   planned set was written are that set, save those someone else changed
//!   since it was planned.
//! - **Every change reaches the working set within the polling bound**, even
//!   when every webhook is lost: a person's comment on a tracked item, or on
//!   its pull request, is news for it; a label change is told; a close makes
//!   it leave; a push to its pull request's head and CI reporting on that
//!   head reach its inbox, unless they come back to what it was last told; a
//!   reviewer's first verdict on that head does; an open issue a person hands
//!   in is offered; and after a restart, the cold start ends within it too,
//!   and an item a person took the tracking label off is found again by the
//!   slow pass, within a bound of its own.
//! - **The rate limit is respected after a refusal:** no call goes out
//!   before the reset a refusal named.
//!
//! It injects the engine's restarts, at moments the world draws.

use std::collections::{BTreeMap, BTreeSet};

use skein_lib::{Duration, Time};
use temper_engine_domain_forge::{Ci, Item, News};
use temper_forge_domain::Observation;
use temper_forge_domain::api::{Check, Kind, Verdict};
use temper_world::{Expectations, Judge};

use crate::translate;
use crate::world::{ENGINE, HAND_IN, OWNED, REPOSITORIES, TRACKING};

/// A write the parent planned, as the referee matches it to what the forge
/// sees.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Planned {
    CreateIssue { repository: u32, key: Vec<u8> },
    Comment { item: Item, key: Vec<u8> },
    Record { item: Item },
    SetLabels { item: Item, labels: Vec<Vec<u8>> },
    OpenPull { repository: u32, head: Vec<u8> },
    Review { item: Item, key: Vec<u8> },
    SetReviewers { item: Item },
    SetDependencies { item: Item },
    Merge { item: Item, head: u64 },
    Close { item: Item },
    DeleteBranch { repository: u32, branch: Vec<u8> },
    PutPage { repository: u32, name: Vec<u8> },
}

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The parent planned the write `plan`.
    Planned {
        plan: u64,
        write: Planned,
    },
    /// The write `plan` was answered: written or not, and if not, whether
    /// for a record someone else changed.
    Wrote {
        plan: u64,
        written: bool,
        edited: bool,
    },
    /// A change on the forge.
    Forge(Observation),
    /// The child domain told its parent: `item` is announced, with `labels`,
    /// its record saying the head (the fake's count) and CI taken; news for it;
    /// its labels are `labels`; it left; an issue is offered.
    Announced {
        item: Item,
        labels: Vec<Vec<u8>>,
        head: Option<u64>,
        ci: Ci,
    },
    News {
        item: Item,
        news: News,
    },
    Changed {
        item: Item,
        labels: Vec<Vec<u8>>,
    },
    Left {
        item: Item,
    },
    Offered {
        item: Item,
    },
    /// The parent stopped tracking `item`; linked it to the pull request
    /// `pull`, or none.
    Untracked {
        item: Item,
    },
    Linked {
        item: Item,
        pull: Option<u64>,
    },
    /// The cold start ended.
    Loaded,
    /// A call went out; a refusal for the rate came back, naming `reset`.
    Sent,
    Limited {
        reset: Time,
    },
    /// The engine restarted: a new child domain, starting cold.
    Restarted,
    /// The world settled: the checks at the end.
    Settled,
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// A person's comment `id` on `item`, or on its pull request, is news for
    /// it.
    Comment { item: Item, id: u64 },
    /// A person's label change on `item`, the `change`th the referee saw, is
    /// told.
    Labels { item: Item, change: u64 },
    /// `item`, closed by a person, leaves.
    Closed { item: Item },
    /// The cold start after the `restart`th restart ends.
    Loaded { restart: u64 },
    /// `item`'s pull request at the head `commit` with `ci` reaches its
    /// inbox.
    Pull { item: Item, commit: u64, ci: Ci },
    /// A verdict of `author`'s, new on `item`'s pull request's head `commit`,
    /// reaches its inbox.
    Review { item: Item, author: u64, commit: u64 },
    /// The open issue `item`, handed in, is offered or taken in.
    Offered { item: Item },
    /// `item`, tracked once and its tracking label taken off, is found again
    /// after a restart.
    Refound { item: Item },
}

/// What the referee injects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stimulus {
    /// The engine restarts.
    Restart,
}

/// The bounds the referee holds the child domain to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounds {
    /// How long a change may take to reach the working set; an item the
    /// slow pass is to find again, to be found.
    pub within: Duration,
    pub slow: Duration,
    /// How long after a write is answered, or abandoned by a restart, a call
    /// of it may still land.
    pub lifetime: Duration,
    /// The items the working set holds at most: one it may have no room for
    /// is not owed a refind.
    pub room: u32,
}

/// A pull request on the forge: its head branch and commit, and whether it
/// is open.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Head {
    branch: Vec<u8>,
    commit: u64,
    open: bool,
}

/// The expectations of the forge's world.
#[derive(Debug)]
pub struct Forge {
    bounds: Bounds,
    /// The plans, in the order they reached the child domain: what each is, and
    /// whether it was written; each plan's place, by the parent's name; when
    /// a plan answered or abandoned is retired, and when, in the order of
    /// what the forge saw, each was made.
    plans: BTreeMap<u64, (Planned, Option<bool>)>,
    /// The places of the plans, by what each is; and of the label sets
    /// planned, by item.
    by_write: BTreeMap<Planned, BTreeSet<u64>>,
    by_labels: BTreeMap<Item, BTreeSet<u64>>,
    places: BTreeMap<u64, u64>,
    retire: BTreeMap<u64, Time>,
    planned_at: BTreeMap<u64, u64>,
    /// Creations seen: keys of issues, comments and reviews, items with a
    /// record and the records' comments, branches with a pull request.
    issues: BTreeSet<(u32, Vec<u8>)>,
    comments: BTreeSet<(Item, Vec<u8>)>,
    reviews: BTreeSet<(Item, Vec<u8>)>,
    records: BTreeSet<Item>,
    record_ids: BTreeMap<(u32, u64), Item>,
    pulls: BTreeSet<(u32, Vec<u8>)>,
    /// Per item: the place of the plan of the last engine label write seen,
    /// and the labels it carries now; per label the engine owns on an item,
    /// who last added or removed it, and when, in the order of what the
    /// forge saw.
    labelled: BTreeMap<Item, u64>,
    labels: BTreeMap<Item, Vec<Vec<u8>>>,
    touched_labels: BTreeMap<(Item, Vec<u8>), (u64, u64)>,
    seen: u64,
    /// What the forge holds of items: their kinds, those closed, pull
    /// requests' heads, the latest status of each context on a commit, and
    /// the reviewers with a verdict on a pull request's head.
    kinds: BTreeMap<Item, Kind>,
    closed: BTreeSet<Item>,
    heads: BTreeMap<Item, Head>,
    statuses: BTreeMap<(u32, u64), BTreeMap<Vec<u8>, Check>>,
    verdicts: BTreeMap<(Item, u64), BTreeSet<u64>>,
    /// The items whose record someone else edited since the engine last wrote
    /// it.
    touched: BTreeSet<Item>,
    /// The items the child domain holds, as it told them, the labels it last
    /// told of each, the pull request each is linked to, and the head and CI
    /// it last told of it.
    tracked: BTreeSet<Item>,
    told: BTreeMap<Item, Vec<Vec<u8>>>,
    linked: BTreeMap<Item, Item>,
    told_pull: BTreeMap<Item, (Option<u64>, Ci)>,
    /// The label changes pending, by item: the change's count and its labels;
    /// the comments, closes, pull requests, verdicts, hand-ins and refinds
    /// awaited.
    changes: BTreeMap<Item, (u64, Vec<Vec<u8>>)>,
    awaited: BTreeSet<(Item, u64)>,
    closing: BTreeSet<Item>,
    pulling: BTreeMap<Item, (u64, Ci)>,
    reviewing: BTreeSet<(Item, u64, u64)>,
    offering: BTreeSet<Item>,
    refinding: BTreeSet<Item>,
    count: u64,
    restarts: u64,
    /// A reset a refusal named, not yet passed.
    reset: Option<Time>,
    /// What it judged: changes reaching the working set, and engine writes.
    pub reached: u64,
    pub writes: u64,
}

impl Forge {
    #[must_use]
    pub fn new(bounds: Bounds) -> Forge {
        Forge {
            bounds,
            plans: BTreeMap::new(),
            by_write: BTreeMap::new(),
            by_labels: BTreeMap::new(),
            places: BTreeMap::new(),
            retire: BTreeMap::new(),
            planned_at: BTreeMap::new(),
            issues: BTreeSet::new(),
            comments: BTreeSet::new(),
            reviews: BTreeSet::new(),
            records: BTreeSet::new(),
            record_ids: BTreeMap::new(),
            pulls: BTreeSet::new(),
            labelled: BTreeMap::new(),
            labels: BTreeMap::new(),
            touched_labels: BTreeMap::new(),
            seen: 0,
            kinds: BTreeMap::new(),
            closed: BTreeSet::new(),
            heads: BTreeMap::new(),
            statuses: BTreeMap::new(),
            verdicts: BTreeMap::new(),
            touched: BTreeSet::new(),
            tracked: BTreeSet::new(),
            told: BTreeMap::new(),
            linked: BTreeMap::new(),
            told_pull: BTreeMap::new(),
            changes: BTreeMap::new(),
            awaited: BTreeSet::new(),
            closing: BTreeSet::new(),
            pulling: BTreeMap::new(),
            reviewing: BTreeSet::new(),
            offering: BTreeSet::new(),
            refinding: BTreeSet::new(),
            count: 0,
            restarts: 0,
            reset: None,
            reached: 0,
            writes: 0,
        }
    }

    /// Whether the plan at `place` may still be what the forge sees: in hand,
    /// or answered less than a lifetime ago.
    fn live(&self, place: u64, now: Time) -> bool {
        match self.retire.get(&place) {
            Some(at) => now < *at,
            None => true,
        }
    }

    /// The first live plan that is `write`, if one is.
    fn planned(&self, now: Time, write: &Planned) -> Option<u64> {
        let places = self.by_write.get(write)?;
        places.iter().copied().find(|place| self.live(*place, now))
    }

    /// The first live label set planned for `item` from the place `from` on
    /// that explains the labels `added` and `removed`, if one does.
    fn planned_labels(&self, now: Time, item: Item, from: u64, added: &[Vec<u8>], removed: &[Vec<u8>]) -> Option<u64> {
        let places = self.by_labels.get(&item)?;
        places.range(from..).copied().find(|place| {
            self.live(*place, now)
                && self.plans.get(place).is_some_and(|(plan, _)| explains(plan, item, added, removed))
        })
    }

    /// The engine changed the forge: as the parent planned, and once per
    /// creation.
    fn engine(&mut self, observation: &Observation, judge: &mut Judge<Expected, Stimulus>) {
        let now = judge.now();
        self.writes += 1;
        match observation {
            Observation::Opened { repository, number: _, kind, title: _, body, labels: _, branches, by: _ } => {
                let repository = index(repository);
                match kind {
                    Kind::Issue => {
                        let key = translate::key_of(body).unwrap_or_default();
                        let planned = self.planned(now, &Planned::CreateIssue { repository, key: key.clone() });
                        judge.check(planned.is_some(), format_args!("an issue the parent planned: {key:?}"));
                        let fresh = self.issues.insert((repository, key.clone()));
                        judge.check(fresh, format_args!("one issue per key: {}", String::from_utf8_lossy(&key)));
                    }
                    Kind::Pull => {
                        let head = branches.as_ref().map(|branches| branches.head.to_vec()).unwrap_or_default();
                        let planned = self.planned(now, &Planned::OpenPull { repository, head: head.clone() });
                        judge.check(planned.is_some(), "a pull request the parent planned");
                        let fresh = self.pulls.insert((repository, head.clone()));
                        judge.check(
                            fresh,
                            format_args!("one pull request per branch: {}", String::from_utf8_lossy(&head)),
                        );
                    }
                }
            }
            Observation::Commented { repository, number, id, body, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                if translate::is_record(body) {
                    let planned = self.planned(now, &Planned::Record { item });
                    judge.check(planned.is_some(), format_args!("a record the parent planned: {item:?}"));
                    let fresh = self.records.insert(item);
                    judge.check(fresh, format_args!("one record per item: {item:?}"));
                    self.record_ids.insert((item.repository, *id), item);
                    self.touched.remove(&item);
                } else {
                    let key = translate::key_of(body).unwrap_or_default();
                    let planned = self.planned(now, &Planned::Comment { item, key: key.clone() });
                    judge.check(planned.is_some(), format_args!("a comment the parent planned: {item:?}"));
                    let fresh = self.comments.insert((item, key.clone()));
                    judge.check(fresh, format_args!("one comment per key: {}", String::from_utf8_lossy(&key)));
                }
            }
            Observation::Edited { repository, number, id: _, body: _, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let planned = self.planned(now, &Planned::Record { item });
                judge.check(planned.is_some(), format_args!("a record edit the parent planned: {item:?}"));
                self.touched.remove(&item);
            }
            Observation::Labelled { .. } | Observation::Moved { .. } | Observation::Refused { .. } => {
                // Labels are judged as they change, with what they were
                // before; a merge moves its base, as the merge's own doing.
                self.writes -= 1;
            }
            Observation::Reviewed { repository, number, id: _, commit: _, verdict: _, body, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let key = translate::key_of(body).unwrap_or_default();
                let planned = self.planned(now, &Planned::Review { item, key: key.clone() });
                judge.check(planned.is_some(), format_args!("a review the parent planned: {item:?}"));
                let fresh = self.reviews.insert((item, key.clone()));
                judge.check(fresh, format_args!("one review per key: {}", String::from_utf8_lossy(&key)));
            }
            Observation::Requested { repository, number, reviewers: _, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let planned = self.planned(now, &Planned::SetReviewers { item });
                judge.check(planned.is_some(), format_args!("reviewers the parent planned: {item:?}"));
            }
            Observation::Depends { repository, number, dependencies: _, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let planned = self.planned(now, &Planned::SetDependencies { item });
                judge.check(planned.is_some(), format_args!("dependencies the parent planned: {item:?}"));
            }
            Observation::Closed { repository, number, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let planned = self.planned(now, &Planned::Close { item });
                judge.check(planned.is_some(), format_args!("a close the parent planned: {item:?}"));
            }
            Observation::Merged { repository, number, base: _, head, commit: _, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let planned = self.planned(now, &Planned::Merge { item, head: *head });
                judge.check(planned.is_some(), format_args!("a merge the parent planned: {item:?}"));
            }
            Observation::Deleted { repository, branch, at: _, by: _ } => {
                let repository = index(repository);
                let planned = self.planned(now, &Planned::DeleteBranch { repository, branch: branch.to_vec() });
                judge.check(planned.is_some(), "a branch deletion the parent planned");
            }
            Observation::Wiki { repository, name, content: _, revision: _, by: _ } => {
                let repository = index(repository);
                let planned = self.planned(now, &Planned::PutPage { repository, name: name.to_vec() });
                judge.check(planned.is_some(), "a wiki page the parent planned");
            }
            Observation::Reopened { .. }
            | Observation::Revised { .. }
            | Observation::Defined { .. }
            | Observation::Removed { .. }
            | Observation::Reported { .. }
            | Observation::Rejected { .. } => judge.fail(format_args!("the engine never does this: {observation:?}")),
        }
    }

    /// Someone else changed the forge: what of it the working set must come
    /// to know.
    fn person(&mut self, observation: &Observation, judge: &mut Judge<Expected, Stimulus>) {
        match observation {
            Observation::Commented { repository, number, id, body: _, by: _ } => {
                let on = Item { repository: index(repository), number: *number };
                // News for the item, or for the item its pull request it is.
                let item = self.linked.iter().find(|(_, pull)| **pull == on).map(|(item, _)| *item);
                for item in [Some(on), item].into_iter().flatten() {
                    if self.tracked.contains(&item) && self.awaited.insert((item, *id)) {
                        judge.expect(Expected::Comment { item, id: *id }, self.bounds.within);
                    }
                }
            }
            Observation::Closed { repository, number, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                if self.tracked.contains(&item) && self.closing.insert(item) {
                    judge.expect(Expected::Closed { item }, self.bounds.within);
                }
            }
            Observation::Edited { repository, number, .. } => {
                self.touched.insert(Item { repository: index(repository), number: *number });
            }
            Observation::Removed { repository, number, id, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                self.touched.insert(item);
                if self.record_ids.remove(&(item.repository, *id)).is_some() {
                    // The record deleted: one is posted again.
                    self.records.remove(&item);
                    if self.refinding.remove(&item) {
                        judge.withdraw(&Expected::Refound { item });
                    }
                }
            }
            Observation::Reviewed { repository, number, id: _, commit, verdict, body: _, by } => {
                let pull = Item { repository: index(repository), number: *number };
                self.reviewed(pull, *commit, *verdict, *by, judge);
            }
            Observation::Labelled { .. }
            | Observation::Moved { .. }
            | Observation::Deleted { .. }
            | Observation::Opened { .. }
            | Observation::Reopened { .. }
            | Observation::Revised { .. }
            | Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Reported { .. }
            | Observation::Merged { .. }
            | Observation::Refused { .. }
            | Observation::Rejected { .. }
            | Observation::Wiki { .. } => {}
        }
    }

    /// A person's verdict on `pull` at `commit`: a reviewer's first on the
    /// head of a pull request linked to an item tracked reaches its inbox.
    fn reviewed(&mut self, pull: Item, commit: u64, verdict: Verdict, by: u64, judge: &mut Judge<Expected, Stimulus>) {
        let counts = match verdict {
            Verdict::Approve | Verdict::RequestChanges => true,
            Verdict::Comment => false,
        };
        if !counts || !self.verdicts.entry((pull, commit)).or_default().insert(by) {
            return;
        }
        let Some(item) = self.linked.iter().find(|(_, linked)| **linked == pull).map(|(item, _)| *item) else {
            return;
        };
        let head = self.heads.get(&pull).map(|head| (head.commit, head.open));
        if self.tracked.contains(&item) && head == Some((commit, true)) && self.reviewing.insert((item, by, commit)) {
            judge.expect(Expected::Review { item, author: by, commit }, self.bounds.within);
        }
    }

    /// What the forge holds changed: items' kinds, states and labels, pull
    /// requests' heads, statuses.
    fn holds(&mut self, observation: &Observation, judge: &mut Judge<Expected, Stimulus>) {
        match observation {
            Observation::Opened { repository, number, kind, labels, branches, by, .. } => {
                let item = Item { repository: index(repository), number: *number };
                self.kinds.insert(item, *kind);
                let labels: Vec<Vec<u8>> = set(labels.iter().map(|label| label.to_vec()).collect());
                self.labels.insert(item, labels.clone());
                if let Some(branches) = branches {
                    let head = Head { branch: branches.head.to_vec(), commit: branches.commit, open: true };
                    self.heads.insert(item, head);
                }
                if *by != ENGINE {
                    self.handed(item, &labels, judge);
                }
            }
            Observation::Labelled { repository, number, labels, by } => {
                let item = Item { repository: index(repository), number: *number };
                let labels = set(labels.iter().map(|label| label.to_vec()).collect());
                self.relabelled(item, labels.clone(), *by, judge);
                self.label_change(item, &labels, judge);
                if *by != ENGINE {
                    self.handed(item, &labels, judge);
                }
            }
            Observation::Closed { repository, number, .. } | Observation::Merged { repository, number, .. } => {
                let item = Item { repository: index(repository), number: *number };
                self.closed.insert(item);
                if let Some(head) = self.heads.get_mut(&item) {
                    head.open = false;
                }
                if self.offering.remove(&item) {
                    judge.withdraw(&Expected::Offered { item });
                }
                if self.refinding.remove(&item) {
                    judge.withdraw(&Expected::Refound { item });
                }
                self.pulls_moved(item.repository, judge);
            }
            Observation::Reopened { repository, number, .. } => {
                let item = Item { repository: index(repository), number: *number };
                self.closed.remove(&item);
            }
            Observation::Moved { repository, branch, from: _, to, by: _ } => {
                let repository = index(repository);
                for (pull, head) in &mut self.heads {
                    if pull.repository == repository && head.open && *head.branch == **branch {
                        head.commit = *to;
                    }
                }
                self.pulls_moved(repository, judge);
            }
            Observation::Reported { repository, commit, context, state, by: _ } => {
                let repository = index(repository);
                self.statuses.entry((repository, *commit)).or_default().insert(context.to_vec(), *state);
                self.pulls_moved(repository, judge);
            }
            Observation::Deleted { .. }
            | Observation::Revised { .. }
            | Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Commented { .. }
            | Observation::Edited { .. }
            | Observation::Removed { .. }
            | Observation::Reviewed { .. }
            | Observation::Refused { .. }
            | Observation::Rejected { .. }
            | Observation::Wiki { .. } => {}
        }
    }

    /// An open issue not tracked carrying `labels`, which hand it in and do
    /// not track it, is to be offered; one that no longer does is not.
    fn handed(&mut self, item: Item, labels: &[Vec<u8>], judge: &mut Judge<Expected, Stimulus>) {
        let handed = labels.iter().any(|label| label == HAND_IN) && !labels.iter().any(|label| label == TRACKING);
        let issue = self.kinds.get(&item) == Some(&Kind::Issue);
        if handed && issue && !self.closed.contains(&item) && !self.tracked.contains(&item) {
            if self.offering.insert(item) {
                judge.expect(Expected::Offered { item }, self.bounds.within);
            }
        } else if self.offering.remove(&item) {
            judge.withdraw(&Expected::Offered { item });
        }
        let refound = labels.iter().any(|label| label == TRACKING || label == HAND_IN);
        if refound && self.refinding.remove(&item) {
            // Labelled again: found by its labels, or not by the slow pass.
            judge.withdraw(&Expected::Refound { item });
        }
    }

    /// The head and CI of the pull request `pull`, as the forge has them, if
    /// it is open: CI over its contexts, as Forgejo combines it.
    fn truth(&self, pull: Item) -> Option<(u64, Ci)> {
        let head = self.heads.get(&pull)?;
        if !head.open {
            return None;
        }
        let ci = match self.statuses.get(&(pull.repository, head.commit)) {
            None => Ci::None,
            Some(contexts) if contexts.is_empty() => Ci::None,
            Some(contexts) if contexts.values().any(|check| *check == Check::Failed) => Ci::Failed,
            Some(contexts) if contexts.values().any(|check| *check == Check::Pending) => Ci::Pending,
            Some(_) => Ci::Passed,
        };
        Some((head.commit, ci))
    }

    /// The pull requests of `repository` may have moved: what each item
    /// tracked is owed of its own is armed again.
    fn pulls_moved(&mut self, repository: u32, judge: &mut Judge<Expected, Stimulus>) {
        let items: Vec<Item> = self.linked.keys().filter(|item| item.repository == repository).copied().collect();
        for item in items {
            self.owe_pull(item, judge);
        }
    }

    /// What `item`'s inbox is owed of its pull request: its head and CI as
    /// the forge has them, unless that is what it was last told.
    fn owe_pull(&mut self, item: Item, judge: &mut Judge<Expected, Stimulus>) {
        let truth = match self.linked.get(&item) {
            Some(pull) if self.tracked.contains(&item) => self.truth(*pull),
            Some(_) | None => None,
        };
        let told = self.told_pull.get(&item).copied();
        let owed = match truth {
            Some((commit, ci)) if told != Some((Some(commit), ci)) => Some((commit, ci)),
            Some(_) | None => None,
        };
        let pending = self.pulling.get(&item).copied();
        if pending == owed {
            return;
        }
        if let Some((commit, ci)) = pending {
            self.pulling.remove(&item);
            judge.withdraw(&Expected::Pull { item, commit, ci });
        }
        if let Some((commit, ci)) = owed {
            self.pulling.insert(item, (commit, ci));
            judge.expect(Expected::Pull { item, commit, ci }, self.bounds.within);
        }
        // A verdict on a head that is no longer the head is owed nothing.
        let head = truth.map(|(commit, _)| commit);
        let stale: Vec<(Item, u64, u64)> =
            self.reviewing.iter().filter(|(of, _, commit)| *of == item && Some(*commit) != head).copied().collect();
        for (item, author, commit) in stale {
            self.reviewing.remove(&(item, author, commit));
            judge.withdraw(&Expected::Review { item, author, commit });
        }
    }

    /// A label change: a change pending before it is superseded, and this one
    /// expected, if it is a change of an item tracked.
    fn label_change(&mut self, item: Item, labels: &[Vec<u8>], judge: &mut Judge<Expected, Stimulus>) {
        if let Some((change, _)) = self.changes.remove(&item) {
            judge.meet(&Expected::Labels { item, change });
        }
        if self.tracked.contains(&item) && self.told.get(&item).map(Vec::as_slice) != Some(labels) {
            self.count += 1;
            judge.expect(Expected::Labels { item, change: self.count }, self.bounds.within);
            self.changes.insert(item, (self.count, labels.to_vec()));
        }
    }

    /// The labels of `item` reached the working set as `labels`.
    fn labels_told(&mut self, item: Item, labels: &[Vec<u8>], judge: &mut Judge<Expected, Stimulus>) {
        let labels = set(labels.to_vec());
        self.told.insert(item, labels.clone());
        let Some((change, expected)) = self.changes.get(&item) else {
            return;
        };
        if *expected == labels {
            judge.meet(&Expected::Labels { item, change: *change });
            self.reached += 1;
            self.changes.remove(&item);
        }
    }

    /// `item` is no longer tracked: what was expected of it is moot.
    fn forget(&mut self, item: Item, judge: &mut Judge<Expected, Stimulus>) {
        self.tracked.remove(&item);
        self.told.remove(&item);
        self.told_pull.remove(&item);
        self.linked.remove(&item);
        if let Some((change, _)) = self.changes.remove(&item) {
            judge.meet(&Expected::Labels { item, change });
        }
        let comments: Vec<(Item, u64)> = self.awaited.range((item, 0)..=(item, u64::MAX)).copied().collect();
        for (item, id) in comments {
            judge.withdraw(&Expected::Comment { item, id });
            self.awaited.remove(&(item, id));
        }
        if self.closing.remove(&item) {
            judge.withdraw(&Expected::Closed { item });
        }
        if let Some((commit, ci)) = self.pulling.remove(&item) {
            judge.withdraw(&Expected::Pull { item, commit, ci });
        }
        let verdicts: Vec<(Item, u64, u64)> = self.reviewing.iter().filter(|(of, _, _)| *of == item).copied().collect();
        for (item, author, commit) in verdicts {
            self.reviewing.remove(&(item, author, commit));
            judge.withdraw(&Expected::Review { item, author, commit });
        }
    }

    /// The labels of `item` are now `labels`, changed by `by`: a change of
    /// the engine's is one it owns, part of a set planned, not older than the
    /// last it made.
    fn relabelled(&mut self, item: Item, labels: Vec<Vec<u8>>, by: u64, judge: &mut Judge<Expected, Stimulus>) {
        let now = judge.now();
        let before = self.labels.remove(&item).unwrap_or_default();
        let added: Vec<Vec<u8>> = labels.iter().filter(|label| !before.contains(label)).cloned().collect();
        let removed: Vec<Vec<u8>> = before.iter().filter(|label| !labels.contains(label)).cloned().collect();
        let owned = |label: &Vec<u8>| OWNED.contains(&label.as_slice());
        for label in added.iter().chain(&removed).filter(|label| owned(label)) {
            self.touched_labels.insert((item, label.clone()), (by, self.seen));
        }
        if by == ENGINE {
            let theirs: Vec<&Vec<u8>> = added.iter().chain(&removed).filter(|label| !owned(label)).collect();
            judge.check(theirs.is_empty(), format_args!("the engine changes only labels it owns: {item:?} {theirs:?}"));
            let planned = self.planned_labels(now, item, 0, &added, &removed);
            judge.check(planned.is_some(), format_args!("labels the parent planned: {item:?} +{added:?} -{removed:?}"));
            // The plan this change is part of: the earliest live one that
            // explains it, not older than the last one seen.
            let last = self.labelled.get(&item).copied().unwrap_or(0);
            let plan = self.planned_labels(now, item, last, &added, &removed);
            judge.check(
                plan.is_some(),
                format_args!("labels land in the order planned: {item:?} +{added:?} -{removed:?}"),
            );
            if let Some(plan) = plan {
                self.labelled.insert(item, plan);
            }
        }
        self.labels.insert(item, labels);
    }

    /// The checks once the world has settled: the labels the engine owns of
    /// each item whose last planned set was written are that set, save those
    /// someone else added or removed since it was planned.
    fn settled(&self, judge: &mut Judge<Expected, Stimulus>) {
        // The place of each item's last label plan.
        let mut last: BTreeMap<Item, u64> = BTreeMap::new();
        for (place, (plan, _)) in &self.plans {
            match plan {
                Planned::SetLabels { item, .. } => {
                    last.insert(*item, *place);
                }
                Planned::CreateIssue { .. }
                | Planned::Comment { .. }
                | Planned::Record { .. }
                | Planned::OpenPull { .. }
                | Planned::Review { .. }
                | Planned::SetReviewers { .. }
                | Planned::SetDependencies { .. }
                | Planned::Merge { .. }
                | Planned::Close { .. }
                | Planned::DeleteBranch { .. }
                | Planned::PutPage { .. } => {}
            }
        }
        for (item, place) in last {
            let (labels, written) = match self.plans.get(&place) {
                Some((Planned::SetLabels { labels, .. }, written)) => (labels, *written),
                Some(_) | None => continue,
            };
            let Some(now) = self.labels.get(&item) else {
                continue;
            };
            if written != Some(true) {
                continue;
            }
            let since = self.planned_at.get(&place).copied().unwrap_or(0);
            for label in OWNED {
                let theirs = match self.touched_labels.get(&(item, label.to_vec())) {
                    Some((by, at)) => *by != ENGINE && *at >= since,
                    None => false,
                };
                let carried = now.iter().any(|kept| kept == label);
                let wanted = labels.iter().any(|kept| kept == label);
                judge.check(
                    theirs || carried == wanted,
                    format_args!("{item:?} ends with the last set written, {labels:?}: {now:?}"),
                );
            }
        }
    }

    /// The engine restarted: what the old child domain had in hand is the cold
    /// start's to reach again; what the old parent planned may land for a
    /// lifetime yet; an item tracked once that no label finds is the slow
    /// pass's to find.
    fn restarted(&mut self, judge: &mut Judge<Expected, Stimulus>) {
        let now = judge.now();
        for item in std::mem::take(&mut self.tracked) {
            self.forget(item, judge);
        }
        let open: Vec<u64> =
            self.plans.iter().filter(|(_, (_, answered))| answered.is_none()).map(|(place, _)| *place).collect();
        for place in open {
            self.retire.entry(place).or_insert(now.saturating_add(self.bounds.lifetime));
        }
        self.reset = None;
        let lost: Vec<Item> = self
            .records
            .iter()
            .filter(|item| {
                let labels = self.labels.get(item).cloned().unwrap_or_default();
                !self.closed.contains(item) && !labels.iter().any(|label| label == TRACKING || label == HAND_IN)
            })
            .copied()
            .collect();
        for item in lost {
            if self.refinding.insert(item) {
                judge.expect(Expected::Refound { item }, self.bounds.slow);
            }
        }
        // A cold start the restart interrupts is moot.
        judge.withdraw(&Expected::Loaded { restart: self.restarts });
        self.restarts += 1;
        judge.expect(Expected::Loaded { restart: self.restarts }, self.bounds.within);
    }

    /// The write `plan` was answered: retired a lifetime after, and a
    /// record held as edited was someone else's to edit.
    fn wrote(&mut self, plan: u64, written: bool, edited: bool, judge: &mut Judge<Expected, Stimulus>) {
        let place = *self.places.get(&plan).expect("a write answered was planned");
        self.retire.insert(place, judge.now().saturating_add(self.bounds.lifetime));
        if let Some((write, answered)) = self.plans.get_mut(&place) {
            *answered = Some(written);
            let held = match write {
                Planned::Record { item } if edited => Some(*item),
                Planned::Record { .. }
                | Planned::CreateIssue { .. }
                | Planned::Comment { .. }
                | Planned::SetLabels { .. }
                | Planned::OpenPull { .. }
                | Planned::Review { .. }
                | Planned::SetReviewers { .. }
                | Planned::SetDependencies { .. }
                | Planned::Merge { .. }
                | Planned::Close { .. }
                | Planned::DeleteBranch { .. }
                | Planned::PutPage { .. } => None,
            };
            if let Some(item) = held {
                let touched = self.touched.contains(&item);
                judge.check(touched, format_args!("a record held as edited was edited by someone else: {item:?}"));
            }
        }
    }

    /// The parent linked `item` to the pull request `pull`, or none: what
    /// its inbox is owed of it is armed.
    fn link(&mut self, item: Item, pull: Option<u64>, judge: &mut Judge<Expected, Stimulus>) {
        let pull = pull.map(|number| Item { repository: item.repository, number });
        let was = self.linked.get(&item).copied();
        if was.is_some() && was != pull {
            // Relinked: nothing of the other is taken.
            self.told_pull.insert(item, (None, Ci::None));
        }
        match pull {
            Some(pull) => {
                self.linked.insert(item, pull);
            }
            None => {
                self.linked.remove(&item);
            }
        }
        self.owe_pull(item, judge);
    }

    /// `item` was announced: tracked, its labels and what its record says it
    /// took of its pull request told.
    fn announced(
        &mut self,
        item: Item,
        labels: &[Vec<u8>],
        head: Option<u64>,
        ci: Ci,
        judge: &mut Judge<Expected, Stimulus>,
    ) {
        self.tracked.insert(item);
        if self.tracked.len().saturating_add(1) >= usize::try_from(self.bounds.room).expect("fits") {
            // The working set may be full: what the slow pass finds may wait
            // for room as long as it lasts.
            for item in std::mem::take(&mut self.refinding) {
                judge.withdraw(&Expected::Refound { item });
            }
        }
        self.told_pull.insert(item, (head, ci));
        self.labels_told(item, labels, judge);
        for name in [Expected::Offered { item }, Expected::Refound { item }] {
            if judge.meet(&name) {
                self.reached += 1;
            }
        }
        self.offering.remove(&item);
        self.refinding.remove(&item);
    }

    /// News for `item` reached its inbox.
    fn news(&mut self, item: Item, news: News, judge: &mut Judge<Expected, Stimulus>) {
        match news {
            News::Comment { on: _, id, author: _ } => {
                if self.awaited.remove(&(item, id)) {
                    judge.meet(&Expected::Comment { item, id });
                    self.reached += 1;
                }
            }
            News::Pull { commit, ci, .. } => {
                let commit = translate::count(commit);
                self.told_pull.insert(item, (Some(commit), ci));
                if self.pulling.get(&item) == Some(&(commit, ci)) {
                    self.pulling.remove(&item);
                    judge.meet(&Expected::Pull { item, commit, ci });
                    self.reached += 1;
                }
            }
            News::Reviews { commit } => {
                let commit = translate::count(commit);
                let met: Vec<(Item, u64, u64)> =
                    self.reviewing.iter().filter(|(of, _, on)| *of == item && *on == commit).copied().collect();
                for (item, author, commit) in met {
                    self.reviewing.remove(&(item, author, commit));
                    judge.meet(&Expected::Review { item, author, commit });
                    self.reached += 1;
                }
            }
        }
    }
}

impl Expectations for Forge {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        match seen {
            Seen::Planned { plan, write } => {
                let write = match write {
                    Planned::SetLabels { item, labels } => Planned::SetLabels { item, labels: set(labels) },
                    Planned::CreateIssue { .. }
                    | Planned::Comment { .. }
                    | Planned::Record { .. }
                    | Planned::OpenPull { .. }
                    | Planned::Review { .. }
                    | Planned::SetReviewers { .. }
                    | Planned::SetDependencies { .. }
                    | Planned::Merge { .. }
                    | Planned::Close { .. }
                    | Planned::DeleteBranch { .. }
                    | Planned::PutPage { .. } => write,
                };
                let place = u64::try_from(self.places.len()).expect("fits") + 1;
                self.places.insert(plan, place);
                if let Planned::SetLabels { item, .. } = &write {
                    self.by_labels.entry(*item).or_default().insert(place);
                }
                self.by_write.entry(write.clone()).or_default().insert(place);
                self.plans.insert(place, (write, None));
                self.planned_at.insert(place, self.seen);
            }
            Seen::Wrote { plan, written, edited } => self.wrote(plan, written, edited, judge),
            Seen::Forge(observation) => {
                self.seen += 1;
                self.holds(&observation, judge);
                if by(&observation) == ENGINE {
                    self.engine(&observation, judge);
                } else {
                    self.person(&observation, judge);
                }
            }
            Seen::Announced { item, labels, head, ci } => self.announced(item, &labels, head, ci, judge),
            Seen::News { item, news } => self.news(item, news, judge),
            Seen::Changed { item, labels } => self.labels_told(item, &labels, judge),
            Seen::Left { item } => {
                if self.closing.remove(&item) {
                    judge.meet(&Expected::Closed { item });
                    self.reached += 1;
                }
                self.forget(item, judge);
            }
            Seen::Offered { item } => {
                if self.offering.remove(&item) && judge.meet(&Expected::Offered { item }) {
                    self.reached += 1;
                }
            }
            Seen::Untracked { item } => self.forget(item, judge),
            Seen::Linked { item, pull } => self.link(item, pull, judge),
            Seen::Loaded => {
                judge.meet(&Expected::Loaded { restart: self.restarts });
            }
            Seen::Sent => {
                let held = match self.reset {
                    Some(reset) if judge.now() < reset => Some(reset),
                    Some(_) | None => None,
                };
                judge
                    .check(held.is_none(), format_args!("no call goes out before the reset a refusal named: {held:?}"));
            }
            Seen::Limited { reset } => self.reset = Some(self.reset.map_or(reset, |held| held.max(reset))),
            Seen::Restarted => self.restarted(judge),
            Seen::Settled => self.settled(judge),
        }
    }
}

/// Who made a change.
fn by(observation: &Observation) -> u64 {
    match observation {
        Observation::Moved { by, .. }
        | Observation::Deleted { by, .. }
        | Observation::Opened { by, .. }
        | Observation::Closed { by, .. }
        | Observation::Reopened { by, .. }
        | Observation::Labelled { by, .. }
        | Observation::Revised { by, .. }
        | Observation::Depends { by, .. }
        | Observation::Requested { by, .. }
        | Observation::Defined { by, .. }
        | Observation::Commented { by, .. }
        | Observation::Edited { by, .. }
        | Observation::Removed { by, .. }
        | Observation::Reviewed { by, .. }
        | Observation::Reported { by, .. }
        | Observation::Merged { by, .. }
        | Observation::Refused { by, .. }
        | Observation::Rejected { by, .. }
        | Observation::Wiki { by, .. } => *by,
    }
}

/// The deployment's index of a repository the forge names.
fn index(name: &[u8]) -> u32 {
    let index = REPOSITORIES.iter().position(|repository| *repository == name).expect("a deployment's repository");
    u32::try_from(index).expect("few repositories")
}

/// Whether `plan`, setting `item`'s labels, explains a change of them that
/// added `added` and removed `removed`.
fn explains(plan: &Planned, item: Item, added: &[Vec<u8>], removed: &[Vec<u8>]) -> bool {
    match plan {
        Planned::SetLabels { item: of, labels } => {
            *of == item
                && added.iter().all(|label| labels.contains(label))
                && removed.iter().all(|label| !labels.contains(label))
        }
        Planned::CreateIssue { .. }
        | Planned::Comment { .. }
        | Planned::Record { .. }
        | Planned::OpenPull { .. }
        | Planned::Review { .. }
        | Planned::SetReviewers { .. }
        | Planned::SetDependencies { .. }
        | Planned::Merge { .. }
        | Planned::Close { .. }
        | Planned::DeleteBranch { .. }
        | Planned::PutPage { .. } => false,
    }
}

/// Labels as a set, in order: the forge keeps them so.
fn set(mut labels: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    labels.sort();
    labels.dedup();
    labels
}

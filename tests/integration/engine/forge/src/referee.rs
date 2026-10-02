//! What the forge world's scenarios expect, held by a referee
//! (testing-pyramid.md, 5.2) that sees what the fake forge sees (every change,
//! by whom), the calls the sub-model makes and the refusals it hears, the
//! writes the parent plans and how they end, and what the sub-model tells
//! the parent; never the sub-model's state:
//!
//! - **No write the parent did not plan.** Every change the engine's user
//!   makes on the forge is one the parent asked for: an issue or a comment of
//!   a key it planned, a record of an item it planned one for, labels it
//!   planned for that item, a pull request for a branch it planned, a merge,
//!   a close, a branch deleted, a wiki page written.
//! - **No creation made twice.** One issue per key, one comment per key, one
//!   record per item, one pull request per branch, whatever the faults and
//!   restarts.
//! - **Labels end as the last set written.** The engine's label writes on an
//!   item land in the order the parent planned them, never an older set after
//!   a newer one; and once the world settles, an item whose last planned set
//!   was written carries it, unless someone else labelled it since.
//! - **Every change reaches the working set within the polling bound**, even
//!   when every webhook is lost: a person's comment on a tracked item is
//!   news for it, a label change is told, a close makes it leave, within a
//!   bound the world sets; and after a restart, the cold start ends within
//!   it too.
//! - **The rate limit is respected after a refusal:** no call goes out
//!   before the reset a refusal named.
//!
//! It injects the engine's restarts, at moments the world draws.

use std::collections::{BTreeMap, BTreeSet};

use temper_engine_model_forge::Item;
use temper_forge_model::Observation;
use temper_lib::{Duration, Time};
use temper_world::{Expectations, Judge};

use crate::translate;
use crate::world::{ENGINE, REPOSITORIES};

/// A write the parent planned, as the referee matches it to what the forge
/// sees.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Planned {
    CreateIssue { repository: u32, key: Vec<u8> },
    Comment { item: Item, key: Vec<u8> },
    Record { item: Item },
    SetLabels { item: Item, labels: Vec<Vec<u8>> },
    OpenPull { repository: u32, head: Vec<u8> },
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
    /// The sub-model told its parent: `item` is announced, with `labels`;
    /// news of the comment `comment`; its labels are `labels`; it left.
    Announced {
        item: Item,
        labels: Vec<Vec<u8>>,
    },
    News {
        item: Item,
        comment: Option<u64>,
    },
    Changed {
        item: Item,
        labels: Vec<Vec<u8>>,
    },
    Left {
        item: Item,
    },
    /// The parent stopped tracking `item`.
    Untracked {
        item: Item,
    },
    /// The cold start ended.
    Loaded,
    /// A call went out; a refusal for the rate came back, naming `reset`.
    Sent,
    Limited {
        reset: Time,
    },
    /// The engine restarted: a new sub-model, starting cold.
    Restarted,
    /// The world settled: the checks at the end.
    Settled,
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// A person's comment `id` on `item` is news for it.
    Comment { item: Item, id: u64 },
    /// A person's label change on `item`, the `change`th the referee saw, is
    /// told.
    Labels { item: Item, change: u64 },
    /// `item`, closed by a person, leaves.
    Closed { item: Item },
    /// The cold start after the `restart`th restart ends.
    Loaded { restart: u64 },
}

/// What the referee injects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stimulus {
    /// The engine restarts.
    Restart,
}

/// The expectations of the forge's world.
#[derive(Debug)]
pub struct Forge {
    /// How long a change may take to reach the working set.
    within: Duration,
    /// The plans, in the order they reached the sub-model: what each is, and
    /// whether it was written; and each plan's place, by the parent's name.
    plans: BTreeMap<u64, (Planned, Option<bool>)>,
    places: BTreeMap<u64, u64>,
    /// Creations seen: keys of issues and comments, items with a record,
    /// branches with a pull request.
    issues: BTreeSet<(u32, Vec<u8>)>,
    comments: BTreeSet<(Item, Vec<u8>)>,
    records: BTreeSet<Item>,
    pulls: BTreeSet<(u32, Vec<u8>)>,
    /// Per item: the place of the plan of the last engine label write seen,
    /// the labels it carries now and who set them last.
    labelled: BTreeMap<Item, u64>,
    labels: BTreeMap<Item, (Vec<Vec<u8>>, u64)>,
    /// The items whose record someone else edited since the engine last wrote
    /// it.
    touched: BTreeSet<Item>,
    /// The items the sub-model holds, as it told them, and the labels it last
    /// told of each.
    tracked: BTreeSet<Item>,
    told: BTreeMap<Item, Vec<Vec<u8>>>,
    /// The label changes pending, by item: the change's count and its labels;
    /// the comments and closes awaited.
    changes: BTreeMap<Item, (u64, Vec<Vec<u8>>)>,
    awaited: BTreeSet<(Item, u64)>,
    closing: BTreeSet<Item>,
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
    pub fn new(within: Duration) -> Forge {
        Forge {
            within,
            plans: BTreeMap::new(),
            places: BTreeMap::new(),
            issues: BTreeSet::new(),
            comments: BTreeSet::new(),
            records: BTreeSet::new(),
            pulls: BTreeSet::new(),
            labelled: BTreeMap::new(),
            labels: BTreeMap::new(),
            touched: BTreeSet::new(),
            tracked: BTreeSet::new(),
            told: BTreeMap::new(),
            changes: BTreeMap::new(),
            awaited: BTreeSet::new(),
            closing: BTreeSet::new(),
            count: 0,
            restarts: 0,
            reset: None,
            reached: 0,
            writes: 0,
        }
    }

    /// Whether a plan matches, and the first that does.
    fn planned(&self, matches: impl Fn(&Planned) -> bool) -> Option<u64> {
        self.plans.iter().find(|(_, (plan, _))| matches(plan)).map(|(plan, _)| *plan)
    }

    /// The engine changed the forge: as the parent planned, and once per
    /// creation.
    fn engine(&mut self, observation: &Observation, judge: &mut Judge<Expected, Stimulus>) {
        self.writes += 1;
        match observation {
            Observation::Opened { repository, number: _, kind, title: _, body, labels: _, branches, by: _ } => {
                let repository = index(repository);
                match kind {
                    temper_forge_model::api::Kind::Issue => {
                        let key = translate::key_of(body).unwrap_or_default();
                        let planned =
                            self.planned(|plan| *plan == Planned::CreateIssue { repository, key: key.clone() });
                        judge.check(planned.is_some(), format_args!("an issue the parent planned: {key:?}"));
                        let fresh = self.issues.insert((repository, key.clone()));
                        judge.check(fresh, format_args!("one issue per key: {}", String::from_utf8_lossy(&key)));
                    }
                    temper_forge_model::api::Kind::Pull => {
                        let head = branches.as_ref().map(|branches| branches.head.to_vec()).unwrap_or_default();
                        let planned =
                            self.planned(|plan| *plan == Planned::OpenPull { repository, head: head.clone() });
                        judge.check(planned.is_some(), "a pull request the parent planned");
                        let fresh = self.pulls.insert((repository, head.clone()));
                        judge.check(
                            fresh,
                            format_args!("one pull request per branch: {}", String::from_utf8_lossy(&head)),
                        );
                    }
                }
            }
            Observation::Commented { repository, number, id: _, body, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                if translate::is_record(body) {
                    let planned = self.planned(|plan| *plan == Planned::Record { item });
                    judge.check(planned.is_some(), format_args!("a record the parent planned: {item:?}"));
                    let fresh = self.records.insert(item);
                    judge.check(fresh, format_args!("one record per item: {item:?}"));
                    self.touched.remove(&item);
                } else {
                    let key = translate::key_of(body).unwrap_or_default();
                    let planned = self.planned(|plan| *plan == Planned::Comment { item, key: key.clone() });
                    judge.check(planned.is_some(), format_args!("a comment the parent planned: {item:?}"));
                    let fresh = self.comments.insert((item, key.clone()));
                    judge.check(fresh, format_args!("one comment per key: {}", String::from_utf8_lossy(&key)));
                }
            }
            Observation::Edited { repository, number, id: _, body: _, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let planned = self.planned(|plan| *plan == Planned::Record { item });
                judge.check(planned.is_some(), format_args!("a record edit the parent planned: {item:?}"));
                self.touched.remove(&item);
            }
            Observation::Labelled { repository, number, labels, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let labels = set(labels.iter().map(|label| label.to_vec()).collect());
                let last = self.labelled.get(&item).copied().unwrap_or(0);
                let planned = self.planned(|plan| sets(plan, item, &labels));
                judge.check(planned.is_some(), format_args!("labels the parent planned: {item:?} {labels:?}"));
                // The plan this write is: the earliest of these labels not
                // older than the last one seen.
                let plan = self.plans.range(last..).find(|(_, (plan, _))| sets(plan, item, &labels));
                judge.check(plan.is_some(), format_args!("labels land in the order planned: {item:?} {labels:?}"));
                if let Some((plan, _)) = plan {
                    self.labelled.insert(item, *plan);
                }
            }
            Observation::Closed { repository, number, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let planned = self.planned(|plan| *plan == Planned::Close { item });
                judge.check(planned.is_some(), format_args!("a close the parent planned: {item:?}"));
            }
            Observation::Merged { repository, number, base: _, head, commit: _, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                let planned = self.planned(|plan| *plan == Planned::Merge { item, head: *head });
                judge.check(planned.is_some(), format_args!("a merge the parent planned: {item:?}"));
            }
            Observation::Deleted { repository, branch, at: _, by: _ } => {
                let repository = index(repository);
                let planned =
                    self.planned(|plan| *plan == Planned::DeleteBranch { repository, branch: branch.to_vec() });
                judge.check(planned.is_some(), "a branch deletion the parent planned");
            }
            Observation::Wiki { repository, name, content: _, revision: _, by: _ } => {
                let repository = index(repository);
                let planned = self.planned(|plan| *plan == Planned::PutPage { repository, name: name.to_vec() });
                judge.check(planned.is_some(), "a wiki page the parent planned");
            }
            // A merge moves its base, as the merge's own doing.
            Observation::Moved { .. } | Observation::Refused { .. } => self.writes -= 1,
            Observation::Reopened { .. }
            | Observation::Revised { .. }
            | Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Removed { .. }
            | Observation::Reviewed { .. }
            | Observation::Reported { .. }
            | Observation::Rejected { .. } => judge.fail(format_args!("the engine never does this: {observation:?}")),
        }
    }

    /// Someone else changed the forge: what of it the working set must come
    /// to know.
    fn person(&mut self, observation: &Observation, judge: &mut Judge<Expected, Stimulus>) {
        match observation {
            Observation::Commented { repository, number, id, body: _, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                if self.tracked.contains(&item) {
                    judge.expect(Expected::Comment { item, id: *id }, self.within);
                    self.awaited.insert((item, *id));
                }
            }
            Observation::Labelled { repository, number, labels, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                self.label_change(item, labels, judge);
            }
            Observation::Closed { repository, number, by: _ } => {
                let item = Item { repository: index(repository), number: *number };
                if self.tracked.contains(&item) && self.closing.insert(item) {
                    judge.expect(Expected::Closed { item }, self.within);
                }
            }
            Observation::Edited { repository, number, .. } | Observation::Removed { repository, number, .. } => {
                self.touched.insert(Item { repository: index(repository), number: *number });
            }
            Observation::Moved { .. }
            | Observation::Deleted { .. }
            | Observation::Opened { .. }
            | Observation::Reopened { .. }
            | Observation::Revised { .. }
            | Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Reviewed { .. }
            | Observation::Reported { .. }
            | Observation::Merged { .. }
            | Observation::Refused { .. }
            | Observation::Rejected { .. }
            | Observation::Wiki { .. } => {}
        }
    }

    /// A person's label change: a change pending before it is superseded,
    /// and this one expected, if it is a change of an item tracked.
    fn label_change(&mut self, item: Item, labels: &[Box<[u8]>], judge: &mut Judge<Expected, Stimulus>) {
        let labels = set(labels.iter().map(|label| label.to_vec()).collect());
        if let Some((change, _)) = self.changes.remove(&item) {
            judge.meet(&Expected::Labels { item, change });
        }
        if self.tracked.contains(&item) && self.told.get(&item) != Some(&labels) {
            self.count += 1;
            judge.expect(Expected::Labels { item, change: self.count }, self.within);
            self.changes.insert(item, (self.count, labels));
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
        if let Some((change, _)) = self.changes.remove(&item) {
            judge.meet(&Expected::Labels { item, change });
        }
        self.moot(item, judge);
    }

    /// A comment or a close expected of `item` is moot.
    fn moot(&mut self, item: Item, judge: &mut Judge<Expected, Stimulus>) {
        let comments: Vec<(Item, u64)> = self.awaited.range((item, 0)..=(item, u64::MAX)).copied().collect();
        for (item, id) in comments {
            judge.meet(&Expected::Comment { item, id });
            self.awaited.remove(&(item, id));
        }
        if self.closing.remove(&item) {
            judge.meet(&Expected::Closed { item });
        }
    }

    /// The checks once the world has settled: the labels of each item whose
    /// last planned set was written.
    fn settled(&self, judge: &mut Judge<Expected, Stimulus>) {
        let mut last: BTreeMap<Item, (&Vec<Vec<u8>>, Option<bool>)> = BTreeMap::new();
        for (plan, written) in self.plans.values() {
            if let Planned::SetLabels { item, labels } = plan {
                last.insert(*item, (labels, *written));
            }
        }
        for (item, (labels, written)) in last {
            if written != Some(true) {
                continue;
            }
            let Some((now, by)) = self.labels.get(&item) else {
                continue;
            };
            if *by == ENGINE {
                judge.check(
                    now == labels,
                    format_args!("{item:?} ends with the last set written: {now:?}, not {labels:?}"),
                );
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
                    | Planned::Merge { .. }
                    | Planned::Close { .. }
                    | Planned::DeleteBranch { .. }
                    | Planned::PutPage { .. } => write,
                };
                let place = u64::try_from(self.places.len()).expect("fits") + 1;
                self.places.insert(plan, place);
                self.plans.insert(place, (write, None));
            }
            Seen::Wrote { plan, written, edited } => {
                let place = self.places.get(&plan).expect("a write answered was planned");
                if let Some((write, answered)) = self.plans.get_mut(place) {
                    *answered = Some(written);
                    if edited && let Planned::Record { item } = write {
                        let touched = self.touched.contains(item);
                        judge.check(
                            touched,
                            format_args!("a record held as edited was edited by someone else: {item:?}"),
                        );
                    }
                }
            }
            Seen::Forge(observation) => {
                if let Observation::Labelled { repository, number, labels, by } = &observation {
                    let item = Item { repository: index(repository), number: *number };
                    let labels = set(labels.iter().map(|label| label.to_vec()).collect());
                    self.labels.insert(item, (labels, *by));
                }
                if by(&observation) == ENGINE {
                    if let Observation::Labelled { repository, number, labels, .. } = &observation {
                        let item = Item { repository: index(repository), number: *number };
                        self.label_change(item, labels, judge);
                    }
                    self.engine(&observation, judge);
                } else {
                    self.person(&observation, judge);
                }
            }
            Seen::Announced { item, labels } => {
                self.tracked.insert(item);
                self.labels_told(item, &labels, judge);
            }
            Seen::News { item, comment } => {
                if let Some(id) = comment
                    && self.awaited.remove(&(item, id))
                {
                    judge.meet(&Expected::Comment { item, id });
                    self.reached += 1;
                }
            }
            Seen::Changed { item, labels } => self.labels_told(item, &labels, judge),
            Seen::Left { item } => {
                if self.closing.remove(&item) {
                    judge.meet(&Expected::Closed { item });
                    self.reached += 1;
                }
                self.forget(item, judge);
            }
            Seen::Untracked { item } => self.forget(item, judge),
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
            Seen::Restarted => {
                // A cold start reads everything again: what was pending is
                // the cold start's to reach.
                for item in std::mem::take(&mut self.tracked) {
                    self.forget(item, judge);
                }
                self.reset = None;
                // A cold start the restart interrupts is moot.
                judge.meet(&Expected::Loaded { restart: self.restarts });
                self.restarts += 1;
                judge.expect(Expected::Loaded { restart: self.restarts }, self.within);
            }
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

/// Whether `plan` sets `item`'s labels to `labels`.
fn sets(plan: &Planned, item: Item, labels: &[Vec<u8>]) -> bool {
    if let Planned::SetLabels { item: of, labels: set } = plan { *of == item && set == labels } else { false }
}

/// Labels as a set, in order: the forge keeps them so.
fn set(mut labels: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    labels.sort();
    labels.dedup();
    labels
}

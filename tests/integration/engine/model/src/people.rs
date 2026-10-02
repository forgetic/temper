//! Scripted people (testing-pyramid.md, 4.4; engine-model.md, sections 6
//! and 14): forge users who hand issues in, review, and correct notes in
//! the wiki; and clients of the engine's web who open sessions and message
//! them. Each story has its person, who looks at the forge as observed
//! ([`Mirror`]) every so often and does what the story calls for next, one
//! thing at a time: level-triggered, so that a call that failed, an answer
//! lost with a restarting engine, or a session that took its time, is
//! simply looked at again. A reviewer approves every pull request the
//! engine opens once CI passed on its exact head.

use std::collections::{BTreeMap, BTreeSet};

use temper_engine_model::notes::Author;
use temper_engine_model::{Ask, Item, Refusal, Reply};
use temper_forge_model::api::{self as forge, Kind, Verdict, Write};

use crate::codec;
use crate::deployment::{self, ENGINE, HAND_IN, REPOSITORIES, REVIEWER, STRANGER};
use crate::mirror::Mirror;
use crate::script::NOTE;

/// What a scenario's people do.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Story {
    /// A session from the web that says hello, is thanked, and finishes.
    Hello,
    /// An issue handed in on the forge: its session asks for a change, whose
    /// first push fails CI and whose repair passes; a person reviews it, and
    /// it lands on the protected branch.
    Fix,
    /// A session chatting from the web: it replies through its outlet, waits
    /// for a message, parks, and is resumed by the next.
    Chat,
    /// A session that writes a note; a person corrects it in the wiki; the
    /// session's next run recalls the correction.
    Notes,
    /// A session proposes a plan (engine-model.md, 5.2), its person accepts
    /// it, decides once the spikes have reported, and the build grows it
    /// within its envelope; its changes land as the fix's does.
    Plan,
    /// The same plan, whose build grows beyond its envelope: held for its
    /// person's acceptance, which it gets.
    Grow,
    /// A plan proposed and rejected: the session drops it.
    Reject,
    /// An issue handed in where CI never reports: its change stalls and is
    /// held, the caretaker releases it, it stalls again, and its person
    /// closes the session, leaving the change held.
    Stall,
}

/// Every story, which random worlds draw from.
pub const STORIES: [Story; 8] =
    [Story::Hello, Story::Fix, Story::Chat, Story::Notes, Story::Plan, Story::Grow, Story::Reject, Story::Stall];

/// Whether an item's phase is held.
fn matches_held(phase: temper_engine_model::work::Phase) -> bool {
    use temper_engine_model::work::Phase;
    match phase {
        Phase::Held { .. } => true,
        Phase::Waiting | Phase::Parked | Phase::Retrying(_) | Phase::Claimed | Phase::Applying { .. } | Phase::Done => {
            false
        }
    }
}

/// Who asks the engine: a story's person, or the caretaker, who releases
/// a held item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Asker {
    Tale(usize),
    Caretaker(Item),
}

/// The person who releases items held for failures or stalls, up to
/// `RELEASES` times each.
pub const CARETAKER: u64 = deployment::PEOPLE[0];
pub const RELEASES: u32 = 5;

/// Something a person does.
#[derive(Debug)]
pub enum Act {
    /// Through the engine's web.
    Ask { asker: Asker, person: u64, ask: Ask },
    /// On the forge, as a forge user: `tale` is the story's, if any.
    Forge { tale: Option<usize>, user: u64, repository: usize, op: forge::Op },
}

/// A story as it goes.
#[derive(Debug)]
struct Tale {
    story: Story,
    person: u64,
    key: Vec<u8>,
    /// The item it is about, once it knows it.
    item: Option<Item>,
    /// Whether something it did is in flight.
    pending: bool,
    /// How many messages it has sent, and whether it corrected its note.
    sent: u32,
    corrected: bool,
}

/// What people did, counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    pub asks: u32,
    pub refused: u32,
    pub lost: u32,
    pub calls: u32,
    pub reviews: u32,
    pub releases: u32,
}

#[derive(Debug)]
pub struct People {
    tales: Vec<Tale>,
    /// Reviews in flight, by repository, pull request and head.
    reviewing: BTreeSet<(usize, u64, u64)>,
    /// Releases asked per item, and those in flight.
    released: BTreeMap<Item, u32>,
    releasing: BTreeSet<Item>,
    tally: Tally,
}

impl People {
    #[must_use]
    pub fn new(stories: &[Story]) -> People {
        let tales = stories
            .iter()
            .enumerate()
            .map(|(at, story)| Tale {
                story: *story,
                person: deployment::PEOPLE[at % deployment::PEOPLE.len()],
                key: format!("tale-{at}").into_bytes(),
                item: None,
                pending: false,
                sent: 0,
                corrected: false,
            })
            .collect();
        People {
            tales,
            reviewing: BTreeSet::new(),
            released: BTreeMap::new(),
            releasing: BTreeSet::new(),
            tally: Tally::default(),
        }
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// The stories, in order.
    #[must_use]
    pub fn stories(&self) -> Vec<Story> {
        self.tales.iter().map(|tale| tale.story).collect()
    }

    /// The item of the story `tale`, once its person knows it.
    #[must_use]
    pub fn item(&self, tale: usize) -> Option<Item> {
        self.tales[tale].item
    }

    /// Whether every story's item is closed, and nothing is in flight.
    #[must_use]
    pub fn is_done(&self, mirror: &Mirror) -> bool {
        self.reviewing.is_empty()
            && self.releasing.is_empty()
            && self.tales.iter().all(|tale| {
                !tale.pending
                    && tale.item.is_some_and(|item| {
                        mirror.issue(deployment::name(item.repository), item.number).is_some_and(|issue| !issue.open)
                    })
            })
    }

    /// What people do now.
    pub fn act(&mut self, mirror: &Mirror, out: &mut Vec<Act>) {
        for at in 0..self.tales.len() {
            self.find(at, mirror);
            if self.tales[at].pending {
                continue;
            }
            if let Some(act) = self.next(at, mirror) {
                self.tales[at].pending = true;
                out.push(act);
            }
        }
        self.review(mirror, out);
        self.release(mirror, out);
    }

    /// The caretaker releases what is held for failures or a stall.
    fn release(&mut self, mirror: &Mirror, out: &mut Vec<Act>) {
        use temper_engine_model::work::{Hold, Phase};
        for (repository, number, issue) in mirror.items() {
            let Some(index) = deployment::index(repository) else { continue };
            let item = Item { repository: index, number };
            if !issue.open || self.releasing.contains(&item) {
                continue;
            }
            let Some(record) = mirror.record(repository, number) else { continue };
            let releasable = match record.lifecycle.phase {
                Phase::Held { why, .. } => match why {
                    Hold::Failures(_) | Hold::Plan { .. } | Hold::Stopped => true,
                    Hold::Acceptance | Hold::Writes | Hold::Record => false,
                },
                Phase::Waiting
                | Phase::Parked
                | Phase::Retrying(_)
                | Phase::Claimed
                | Phase::Applying { .. }
                | Phase::Done => false,
            };
            let count = self.released.get(&item).copied().unwrap_or_default();
            if !releasable || count >= RELEASES {
                continue;
            }
            self.releasing.insert(item);
            out.push(Act::Ask { asker: Asker::Caretaker(item), person: CARETAKER, ask: Ask::Release { item } });
        }
    }

    /// A story's item, found on the forge by what its person made there.
    fn find(&mut self, at: usize, mirror: &Mirror) {
        let tale = &mut self.tales[at];
        if tale.item.is_some() || !(tale.story == Story::Fix || tale.story == Story::Stall) {
            return;
        }
        // The first it made: a call that timed out may have made one too.
        for (repository, number, issue) in mirror.items() {
            if issue.by == tale.person && issue.body == tale.key {
                let repository = deployment::index(repository).expect("one of the deployment's");
                tale.item = Some(Item { repository, number });
                return;
            }
        }
    }

    fn next(&mut self, at: usize, mirror: &Mirror) -> Option<Act> {
        let tale = &self.tales[at];
        let person = tale.person;
        let Some(item) = tale.item else {
            return Some(match tale.story {
                Story::Fix | Story::Stall => {
                    let op = forge::Op::Write(Write::CreateIssue {
                        title: b"#fix the build".as_slice().into(),
                        body: tale.key.clone().into(),
                        labels: Box::new([HAND_IN.into()]),
                    });
                    // The stalling story's in the repository whose CI never reports.
                    let repository = usize::from(tale.story == Story::Stall);
                    Act::Forge { tale: Some(at), user: person, repository, op }
                }
                Story::Hello | Story::Chat | Story::Notes | Story::Plan | Story::Grow | Story::Reject => {
                    let title: &[u8] = match tale.story {
                        Story::Hello => b"#hello",
                        Story::Chat => b"#chat",
                        Story::Notes | Story::Fix | Story::Stall => b"#note",
                        Story::Plan => b"#plan",
                        Story::Grow => b"#grow",
                        Story::Reject => b"#reject",
                    };
                    let ask = Ask::Open {
                        repository: 0,
                        key: tale.key.clone().into(),
                        title: title.into(),
                        message: b"hi".as_slice().into(),
                    };
                    Act::Ask { asker: Asker::Tale(at), person, ask }
                }
            });
        };
        let name = deployment::name(item.repository);
        let issue = mirror.issue(name, item.number)?;
        if !issue.open {
            return None;
        }
        match tale.story {
            // An issue it handed in twice, its first call made though it
            // failed: it closes the other.
            Story::Stall => {
                // Once its change stalled again after a release, the person
                // gives up on it: closes the session.
                let stalled_again = mirror.items().any(|(repository, number, _)| {
                    mirror.record(repository, number).is_some_and(|record| {
                        record.relations.parent == Some(item)
                            && record.step.progress.released.is_some()
                            && matches_held(record.lifecycle.phase)
                    })
                });
                stalled_again.then(|| {
                    let op = forge::Op::Write(Write::Close { number: item.number });
                    let repository = usize::try_from(item.repository).expect("few");
                    Act::Forge { tale: Some(at), user: person, repository, op }
                })
            }
            Story::Fix => mirror.items().find_map(|(repository, number, issue)| {
                let twice = issue.by == person && issue.body == tale.key && issue.open && number != item.number;
                twice.then(|| {
                    let repository = REPOSITORIES.iter().position(|name| **name == *repository).expect("ours");
                    let op = forge::Op::Write(Write::Close { number });
                    Act::Forge { tale: Some(at), user: person, repository, op }
                })
            }),
            Story::Hello => {
                let replied = !mirror.outcomes(name, item.number).is_empty();
                (replied && tale.sent == 0).then(|| self.message(at, item, b"thanks"))
            }
            Story::Chat => {
                // Answers the session's first words, then wakes it once it
                // has parked.
                let words = mirror.words(name, item.number, ENGINE);
                let parked = mirror
                    .record(name, item.number)
                    .is_some_and(|record| record.lifecycle.phase == temper_engine_model::work::Phase::Parked);
                let due = (tale.sent == 0 && words > 0) || (tale.sent == 1 && parked);
                due.then(|| self.message(at, item, b"more"))
            }
            Story::Plan | Story::Grow | Story::Reject => self.decide(at, item, mirror),
            Story::Notes => {
                // Corrects the note once it is in the wiki; and asks the
                // session again each time it has answered, to note it while
                // it is not there, and what it learned once it is.
                let replies = u32::try_from(mirror.outcomes(name, item.number).len()).expect("few");
                let page = mirror.pages.get(&(name.to_vec(), NOTE.to_vec()));
                if let Some(page) = page
                    && !tale.corrected
                {
                    let mut note = codec::page_of(page)?;
                    note.body = b"cache it, and clean it weekly".as_slice().into();
                    note.author = Author::Person(person);
                    let content = codec::page(&note).into_boxed_slice();
                    let op = forge::Op::Write(Write::PutPage { name: NOTE.into(), content });
                    let repository = usize::try_from(item.repository).expect("few");
                    return Some(Act::Forge { tale: Some(at), user: person, repository, op });
                }
                let message: &[u8] = if page.is_some() { b"what did you learn?" } else { b"please note it" };
                (replies > tale.sent).then(|| self.message(at, item, message))
            }
        }
    }

    /// A plan's person: accepts, or for `Reject` rejects, the session's
    /// proposal; accepts the plan's decision once its spikes are done, and
    /// any growth held for acceptance.
    fn decide(&self, at: usize, session: Item, mirror: &Mirror) -> Option<Act> {
        use temper_engine_model::plan::{WaitSpec, Work};
        use temper_engine_model::work::{Hold, Phase};
        let tale = &self.tales[at];
        // Held for acceptance: of an outcome or an action, or of a run the
        // rules want accepted (the engine's own hold, coded 7).
        let held = |record: &temper_engine_model::Record| match record.lifecycle.phase {
            Phase::Held { why: Hold::Acceptance | Hold::Plan { reason: 7 }, .. } => true,
            Phase::Held { .. }
            | Phase::Waiting
            | Phase::Parked
            | Phase::Retrying(_)
            | Phase::Claimed
            | Phase::Applying { .. }
            | Phase::Done => false,
        };
        let name = deployment::name(session.repository);
        let record = mirror.record(name, session.number);
        if tale.story == Story::Reject
            && tale.sent == 0
            && record.as_ref().is_some_and(|record| record.step.progress.rejections > 0)
        {
            // Rejected: the person says why, which wakes the session.
            return Some(self.message(at, session, b"not now, thanks"));
        }
        if let Some(record) = record
            && held(record)
        {
            let ask =
                if tale.story == Story::Reject { Ask::Reject { item: session } } else { Ask::Accept { item: session } };
            return Some(Act::Ask { asker: Asker::Tale(at), person: tale.person, ask });
        }
        for (repository, number, issue) in mirror.items() {
            let Some(index) = deployment::index(repository) else { continue };
            let Some(record) = mirror.record(repository, number) else { continue };
            if !issue.open || record.relations.goal != Some(session) || record.relations.decision.is_some() {
                continue;
            }
            let ready = record.relations.dependencies.iter().all(|dependency| {
                let name = deployment::name(dependency.item.repository);
                mirror.issue(name, dependency.item.number).is_some_and(|issue| !issue.open)
            });
            let decision = record.step.step.work == Work::Wait(WaitSpec::Decision);
            if (decision && ready) || held(record) {
                let item = Item { repository: index, number };
                return Some(Act::Ask { asker: Asker::Tale(at), person: tale.person, ask: Ask::Accept { item } });
            }
        }
        None
    }

    fn message(&self, at: usize, item: Item, message: &[u8]) -> Act {
        let tale = &self.tales[at];
        let mut key = tale.key.clone();
        key.extend_from_slice(format!("/m{}", tale.sent).as_bytes());
        let ask = Ask::Message { item, key: key.into(), message: message.into() };
        Act::Ask { asker: Asker::Tale(at), person: tale.person, ask }
    }

    /// The reviewer approves each pull request the engine opened, once CI
    /// passed on its head.
    fn review(&mut self, mirror: &Mirror, out: &mut Vec<Act>) {
        for (repository, number, issue) in mirror.items() {
            let Some(pull) = &issue.pull else { continue };
            if issue.kind != Kind::Pull || !issue.open || issue.by != ENGINE || pull.merged.is_some() {
                continue;
            }
            let Some(at) = REPOSITORIES.iter().position(|name| **name == *repository) else { continue };
            let head = pull.commit;
            let reviewed = issue.reviews.iter().any(|review| review.by == REVIEWER && review.commit == head);
            if reviewed || !mirror.is_green(repository, head) || !self.reviewing.insert((at, number, head)) {
                continue;
            }
            self.tally.reviews += 1;
            // Someone who may only read approves first: the engine counts no
            // approval of theirs.
            let op = forge::Op::Write(Write::Review {
                number,
                verdict: Some(Verdict::Approve),
                body: b"ship it".as_slice().into(),
            });
            out.push(Act::Forge { tale: None, user: STRANGER, repository: at, op });
            // The reviewer asks for changes on the first head of every third
            // pull request, and approves the head that repairs it.
            let first = !issue.reviews.iter().any(|review| review.by == REVIEWER);
            let (verdict, body): (Verdict, &[u8]) = if first && number % 3 == 0 {
                (Verdict::RequestChanges, b"please name it better")
            } else {
                (Verdict::Approve, b"looks good")
            };
            let op = forge::Op::Write(Write::Review { number, verdict: Some(verdict), body: body.into() });
            out.push(Act::Forge { tale: None, user: REVIEWER, repository: at, op });
        }
    }

    /// A review's call ended.
    pub fn reviewed(&mut self, repository: usize, number: u64, head: u64) {
        self.reviewing.remove(&(repository, number, head));
    }

    /// The engine answered an ask: a message, if `message`.
    pub fn replied(&mut self, asker: Asker, reply: Reply, message: bool) {
        self.tally.asks += 1;
        let at = match asker {
            Asker::Tale(at) => at,
            Asker::Caretaker(item) => {
                self.releasing.remove(&item);
                match reply {
                    Reply::Done => {
                        self.tally.releases += 1;
                        *self.released.entry(item).or_default() += 1;
                    }
                    Reply::Opened { .. } | Reply::Watching { .. } | Reply::Refused(_) => self.tally.refused += 1,
                }
                return;
            }
        };
        let tale = &mut self.tales[at];
        tale.pending = false;
        match reply {
            Reply::Opened { item } => tale.item = Some(item),
            Reply::Done => {
                if message {
                    tale.sent += 1;
                }
            }
            Reply::Watching { .. } => {}
            Reply::Refused(refusal) => {
                self.tally.refused += 1;
                match refusal {
                    // Not tracked yet, as an engine that restarted may not
                    // have read it back yet: asked again later.
                    // Or no longer held, as a person decided meanwhile.
                    Refusal::Busy | Refusal::Failed | Refusal::Unknown | Refusal::Unheld => {}
                    Refusal::Unpermitted | Refusal::Idle | Refusal::Unfollowed => {
                        panic!("a story asks only what it may: {refusal:?}")
                    }
                }
            }
        }
    }

    /// An ask was lost with the engine that had it.
    pub fn lost(&mut self, asker: Asker) {
        self.tally.lost += 1;
        match asker {
            Asker::Tale(at) => self.tales[at].pending = false,
            Asker::Caretaker(item) => {
                self.releasing.remove(&item);
            }
        }
    }

    /// The story's call to the forge ended.
    pub fn called(&mut self, at: usize, made: bool) {
        self.tally.calls += 1;
        let tale = &mut self.tales[at];
        tale.pending = false;
        if made && tale.story == Story::Notes {
            tale.corrected = true;
        }
    }
}

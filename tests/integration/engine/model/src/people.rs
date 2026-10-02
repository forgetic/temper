//! Scripted people (testing-pyramid.md, 4.4; engine-model.md, sections 6
//! and 14): forge users who hand issues in, review, and correct notes in
//! the wiki; and clients of the engine's web who open sessions and message
//! them. Each story has its person, who looks at the forge as observed
//! ([`Mirror`]) every so often and does what the story calls for next, one
//! thing at a time: level-triggered, so that a call that failed, an answer
//! lost with a restarting engine, or a session that took its time, is
//! simply looked at again. A reviewer approves every pull request the
//! engine opens once CI passed on its exact head.

use std::collections::BTreeSet;

use temper_engine_model::notes::{Author, Page};
use temper_engine_model::{Ask, Item, Refusal, Reply};
use temper_forge_model::api::{self as forge, Kind, Verdict, Write};

use crate::codec;
use crate::deployment::{self, ENGINE, HAND_IN, REPOSITORIES, REVIEWER};
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
}

pub const STORIES: [Story; 4] = [Story::Hello, Story::Fix, Story::Chat, Story::Notes];

/// Something a person does.
#[derive(Debug)]
pub enum Act {
    /// Through the engine's web.
    Ask { tale: usize, person: u64, ask: Ask },
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
}

#[derive(Debug)]
pub struct People {
    tales: Vec<Tale>,
    /// Reviews in flight, by repository, pull request and head.
    reviewing: BTreeSet<(usize, u64, u64)>,
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
        People { tales, reviewing: BTreeSet::new(), tally: Tally::default() }
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
    }

    /// A story's item, found on the forge by what its person made there.
    fn find(&mut self, at: usize, mirror: &Mirror) {
        let tale = &mut self.tales[at];
        if tale.item.is_some() || tale.story != Story::Fix {
            return;
        }
        for ((repository, number), issue) in &mirror.issues {
            if issue.by == tale.person && issue.body == tale.key {
                let repository = deployment::index(repository).expect("one of the deployment's");
                tale.item = Some(Item { repository, number: *number });
            }
        }
    }

    fn next(&mut self, at: usize, mirror: &Mirror) -> Option<Act> {
        let tale = &self.tales[at];
        let person = tale.person;
        let Some(item) = tale.item else {
            return Some(match tale.story {
                Story::Fix => {
                    let op = forge::Op::Write(Write::CreateIssue {
                        title: b"#fix the build".as_slice().into(),
                        body: tale.key.clone().into(),
                        labels: Box::new([HAND_IN.into()]),
                    });
                    Act::Forge { tale: Some(at), user: person, repository: 0, op }
                }
                Story::Hello | Story::Chat | Story::Notes => {
                    let title: &[u8] = match tale.story {
                        Story::Hello => b"#hello",
                        Story::Chat => b"#chat",
                        Story::Notes | Story::Fix => b"#note",
                    };
                    let ask = Ask::Open {
                        repository: 0,
                        key: tale.key.clone().into(),
                        title: title.into(),
                        message: b"hi".as_slice().into(),
                    };
                    Act::Ask { tale: at, person, ask }
                }
            });
        };
        let name = deployment::name(item.repository);
        let issue = mirror.issue(name, item.number)?;
        if !issue.open {
            return None;
        }
        match tale.story {
            Story::Fix => None,
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
            Story::Notes => {
                let page = mirror.pages.get(&(name.to_vec(), NOTE.to_vec()))?;
                if !tale.corrected {
                    let mut note = codec::page_of(page)?;
                    note.body = b"cache it, and clean it weekly".as_slice().into();
                    note.author = Author::Person(person);
                    let content = codec::page(&Page { ..note }).into_boxed_slice();
                    let op = forge::Op::Write(Write::PutPage { name: NOTE.into(), content });
                    let repository = usize::try_from(item.repository).expect("few");
                    return Some(Act::Forge { tale: Some(at), user: person, repository, op });
                }
                (tale.sent == 0).then(|| self.message(at, item, b"what did you learn?"))
            }
        }
    }

    fn message(&self, at: usize, item: Item, message: &[u8]) -> Act {
        let tale = &self.tales[at];
        let mut key = tale.key.clone();
        key.extend_from_slice(format!("/m{}", tale.sent).as_bytes());
        let ask = Ask::Message { item, key: key.into(), message: message.into() };
        Act::Ask { tale: at, person: tale.person, ask }
    }

    /// The reviewer approves each pull request the engine opened, once CI
    /// passed on its head.
    fn review(&mut self, mirror: &Mirror, out: &mut Vec<Act>) {
        for ((repository, number), issue) in &mirror.issues {
            let Some(pull) = &issue.pull else { continue };
            if issue.kind != Kind::Pull || !issue.open || issue.by != ENGINE || pull.merged.is_some() {
                continue;
            }
            let Some(at) = REPOSITORIES.iter().position(|name| **name == **repository) else { continue };
            let head = pull.commit;
            let reviewed = issue.reviews.iter().any(|review| review.by == REVIEWER && review.commit == head);
            if reviewed || !mirror.is_green(repository, head) || !self.reviewing.insert((at, *number, head)) {
                continue;
            }
            self.tally.reviews += 1;
            let op = forge::Op::Write(Write::Review {
                number: *number,
                verdict: Some(Verdict::Approve),
                body: b"looks good".as_slice().into(),
            });
            out.push(Act::Forge { tale: None, user: REVIEWER, repository: at, op });
        }
    }

    /// A review's call ended.
    pub fn reviewed(&mut self, repository: usize, number: u64, head: u64) {
        self.reviewing.remove(&(repository, number, head));
    }

    /// The engine answered the story's ask.
    pub fn replied(&mut self, at: usize, reply: Reply) {
        self.tally.asks += 1;
        let tale = &mut self.tales[at];
        tale.pending = false;
        match reply {
            Reply::Opened { item } => tale.item = Some(item),
            Reply::Done => tale.sent += 1,
            Reply::Watching { .. } => {}
            Reply::Refused(refusal) => {
                self.tally.refused += 1;
                match refusal {
                    Refusal::Busy | Refusal::Failed => {}
                    Refusal::Unknown | Refusal::Unpermitted | Refusal::Idle | Refusal::Unheld | Refusal::Unfollowed => {
                        panic!("a story asks only what it may: {refusal:?}")
                    }
                }
            }
        }
    }

    /// The story's ask was lost with the engine that had it.
    pub fn lost(&mut self, at: usize) {
        self.tally.lost += 1;
        self.tales[at].pending = false;
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

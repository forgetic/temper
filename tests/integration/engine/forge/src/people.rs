//! People acting on the fake forge as forge users (testing-pyramid.md, 4.4),
//! scripted: they open issues, some handed to temper, comment, label and
//! unlabel (the tracking label among them), review pull requests, push to
//! their branches, close items, and now and then one with admin permission
//! edits the engine's record by hand, mangling it. Each picks what to act on
//! from the forge as it is, read without faults.

use temper_forge_model::api::{Kind, Op, Read, State, Verdict, Write};
use temper_forge_model::{Config, Model};
use temper_lib::{Rng, Time};
use temper_world::Span;

use crate::translate;
use crate::world::{ENGINE, HAND_IN, LABELS, REPOSITORIES, TRACKING};

/// The people, by their forge users: the first is an admin.
pub const PEOPLE: [u64; 3] = [10, 11, 12];

/// What people do, by weight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Weights {
    pub opens: u32,
    pub comments: u32,
    pub labels: u32,
    pub removals: u32,
    pub hand_ins: u32,
    pub reviews: u32,
    pub pushes: u32,
    pub closes: u32,
    pub mangles: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    /// Actions in all, one every `gap`.
    pub actions: u32,
    pub gap: Span,
    pub weights: Weights,
}

/// What a person does.
#[derive(Debug)]
pub enum Act {
    /// A call to the forge as `user`.
    Call { user: u64, repository: usize, op: Op },
    /// `user` pushes a commit onto `branch`, as another party does.
    Push { user: u64, repository: usize, branch: Vec<u8> },
}

/// What people did.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tally {
    pub opens: u32,
    pub comments: u32,
    pub labels: u32,
    pub removals: u32,
    pub hand_ins: u32,
    pub reviews: u32,
    pub pushes: u32,
    pub closes: u32,
    pub mangles: u32,
}

pub struct People {
    script: Script,
    rng: Rng,
    left: u32,
    tally: Tally,
}

impl People {
    #[must_use]
    pub fn new(script: Script, seed: u64) -> People {
        People { script, rng: Rng::new(seed), left: script.actions, tally: Tally::default() }
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// Whether people have acted all they will.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.left == 0
    }

    /// The time to the next action.
    pub fn gap(&mut self) -> temper_lib::Duration {
        self.script.gap.draw(&mut self.rng)
    }

    /// The next action, chosen by weight, on what the forge holds; none if
    /// what was chosen has nothing to act on.
    #[expect(clippy::too_many_lines, reason = "one arm per action")]
    pub fn act(&mut self, forge: &Model, config: &Config) -> Option<Act> {
        self.left = self.left.checked_sub(1)?;
        let weights = self.script.weights;
        let choices = [
            weights.opens,
            weights.comments,
            weights.labels,
            weights.removals,
            weights.hand_ins,
            weights.reviews,
            weights.pushes,
            weights.closes,
            weights.mangles,
        ];
        let total: u32 = choices.iter().sum();
        let mut pick = u32::try_from(self.rng.below(u64::from(total.max(1)))).expect("below a u32");
        let mut choice = 0;
        for (index, weight) in choices.iter().enumerate() {
            if pick < *weight {
                choice = index;
                break;
            }
            pick -= weight;
        }
        let repository = usize::try_from(self.rng.below(REPOSITORIES.len() as u64)).expect("few repositories");
        let user = PEOPLE[usize::try_from(self.rng.below(PEOPLE.len() as u64)).expect("few people")];
        let open = open(forge, config, repository);
        let act = match choice {
            0 => {
                self.tally.opens += 1;
                let labels: Box<[Box<[u8]>]> = match self.rng.below(3) {
                    0 => Box::new([]),
                    1 => Box::new([HAND_IN.into()]),
                    _ => Box::new([b"bug".as_slice().into()]),
                };
                let write = Write::CreateIssue {
                    title: b"an issue".as_slice().into(),
                    body: b"please".as_slice().into(),
                    labels,
                };
                Act::Call { user, repository, op: Op::Write(write) }
            }
            1 => {
                let (number, _, _) = self.pick(&open)?;
                self.tally.comments += 1;
                let body = format!("a word from {user}").into_bytes().into_boxed_slice();
                Act::Call { user, repository, op: Op::Write(Write::Comment { number, body }) }
            }
            2 => {
                let (number, _, labels) = self.pick(&open)?;
                self.tally.labels += 1;
                let mut labels = labels;
                let label = LABELS[usize::try_from(self.rng.below(LABELS.len() as u64)).expect("few labels")].to_vec();
                if let Some(at) = labels.iter().position(|carried| *carried == label) {
                    if label != TRACKING {
                        labels.remove(at);
                    }
                } else if label != TRACKING {
                    labels.push(label);
                }
                Act::Call { user, repository, op: Op::Write(Write::SetLabels { number, labels: boxed(&labels) }) }
            }
            3 => {
                let tracked: Vec<_> = open
                    .iter()
                    .filter(|(_, _, labels)| labels.iter().any(|label| label == TRACKING))
                    .cloned()
                    .collect();
                let (number, _, labels) = self.pick(&tracked)?;
                self.tally.removals += 1;
                let labels: Vec<Vec<u8>> = labels.into_iter().filter(|label| label != TRACKING).collect();
                Act::Call { user, repository, op: Op::Write(Write::SetLabels { number, labels: boxed(&labels) }) }
            }
            4 => {
                let untracked: Vec<_> = open
                    .iter()
                    .filter(|(_, kind, labels)| {
                        *kind == Kind::Issue && !labels.iter().any(|label| label == TRACKING || label == HAND_IN)
                    })
                    .cloned()
                    .collect();
                let (number, _, mut labels) = self.pick(&untracked)?;
                self.tally.hand_ins += 1;
                labels.push(HAND_IN.to_vec());
                Act::Call { user, repository, op: Op::Write(Write::SetLabels { number, labels: boxed(&labels) }) }
            }
            5 => {
                let pulls: Vec<_> = open.iter().filter(|(_, kind, _)| *kind == Kind::Pull).cloned().collect();
                let (number, _, _) = self.pick(&pulls)?;
                self.tally.reviews += 1;
                let verdict = if self.rng.chance(700) { Verdict::Approve } else { Verdict::RequestChanges };
                let write = Write::Review { number, verdict, body: b"looked".as_slice().into() };
                Act::Call { user, repository, op: Op::Write(write) }
            }
            6 => {
                let pulls: Vec<_> = open.iter().filter(|(_, kind, _)| *kind == Kind::Pull).cloned().collect();
                let (number, _, _) = self.pick(&pulls)?;
                let Ok(temper_forge_model::api::Answer::Pull(pull)) =
                    forge.inspect(config, REPOSITORIES[repository], &Read::Pull { number })
                else {
                    return None;
                };
                self.tally.pushes += 1;
                Act::Push { user, repository, branch: pull.head.to_vec() }
            }
            7 => {
                let (number, _, _) = self.pick(&open)?;
                self.tally.closes += 1;
                Act::Call { user, repository, op: Op::Write(Write::Close { number }) }
            }
            _ => {
                let records = records(forge, config, repository, &open);
                let id = *records.get(usize::try_from(self.rng.below(records.len().max(1) as u64)).expect("few"))?;
                self.tally.mangles += 1;
                let body = b"<!-- temper:record this is mine now".as_slice().into();
                Act::Call { user: PEOPLE[0], repository, op: Op::Write(Write::EditComment { id, body }) }
            }
        };
        Some(act)
    }

    fn pick<T: Clone>(&mut self, from: &[T]) -> Option<T> {
        if from.is_empty() {
            return None;
        }
        let at = usize::try_from(self.rng.below(from.len() as u64)).expect("few");
        Some(from[at].clone())
    }
}

/// The open items of a repository: number, kind and labels.
fn open(forge: &Model, config: &Config, repository: usize) -> Vec<(u64, Kind, Vec<Vec<u8>>)> {
    let mut items = Vec::new();
    for page in 1..=16 {
        let read = Read::Items {
            state: Some(State::Open),
            kind: None,
            labels: Box::new([]),
            author: None,
            since: Time::ZERO,
            page,
            limit: 0,
        };
        let Ok(temper_forge_model::api::Answer::Items { items: listed, more }) =
            forge.inspect(config, REPOSITORIES[repository], &read)
        else {
            break;
        };
        for item in &listed {
            items.push((item.number, item.kind, item.labels.iter().map(|label| label.to_vec()).collect()));
        }
        if !more {
            break;
        }
    }
    items
}

/// The engine's record comments on the open items of a repository.
fn records(forge: &Model, config: &Config, repository: usize, open: &[(u64, Kind, Vec<Vec<u8>>)]) -> Vec<u64> {
    let mut records = Vec::new();
    for (number, _, _) in open {
        let read = Read::Item { number: *number, after: 0 };
        let Ok(temper_forge_model::api::Answer::Item { comments, .. }) =
            forge.inspect(config, REPOSITORIES[repository], &read)
        else {
            continue;
        };
        for comment in &comments {
            if comment.author == ENGINE && translate::is_record(&comment.body) {
                records.push(comment.id);
            }
        }
    }
    records
}

fn boxed(labels: &[Vec<u8>]) -> Box<[Box<[u8]>]> {
    labels.iter().map(|label| label.clone().into_boxed_slice()).collect()
}

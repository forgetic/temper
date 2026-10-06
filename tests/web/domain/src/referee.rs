//! Expectations from durable engine facts and what the tree/address show.
use skein_lib::{Duration, Time};
use std::collections::{BTreeMap, BTreeSet};
use temper_web_domain::{Address, Key};
use temper_world::{Expectations, Judge};

#[derive(Debug)]
pub struct Rules {
    durable: BTreeMap<Key, u64>,
    tasks: BTreeSet<u64>,
    task_keys: BTreeMap<u64, Key>,
    submitted: BTreeSet<Key>,
}

#[derive(Debug)]
pub enum Seen {
    Existing { task: u64 },
    Submitted { key: Key },
    Durable { key: Key, task: u64 },
    Reloaded { before: Vec<u8>, after: Vec<u8> },
    Refused { shown: bool },
    Visible { address: Address, link_live: bool, watch_live: bool, refused: bool },
    WatchLost,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Name {
    Answer(Key),
    Reconnect,
}

#[derive(Debug)]
pub enum Stimulus {
    Restart,
    Reload,
    ExpireSignIn,
}

impl Rules {
    #[must_use]
    pub fn new() -> Rules {
        Rules {
            durable: BTreeMap::new(),
            tasks: BTreeSet::new(),
            task_keys: BTreeMap::new(),
            submitted: BTreeSet::new(),
        }
    }
}

impl Default for Rules {
    fn default() -> Self {
        Self::new()
    }
}

impl Expectations for Rules {
    type Seen = Seen;
    type Name = Name;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Name, Stimulus>) {
        match seen {
            Seen::Existing { task } => {
                self.tasks.insert(task);
            }
            Seen::Submitted { key } => {
                if self.submitted.insert(key) {
                    judge.expect(Name::Answer(key), Duration::from_secs(2));
                }
            }
            Seen::Durable { key, task } => {
                if let Some(old) = self.durable.insert(key, task) {
                    judge.check(old == task, "one key has one durable result");
                }
                self.tasks.insert(task);
                if let Some(old) = self.task_keys.insert(task, key) {
                    judge.check(old == key, "a task is made once");
                }
            }
            Seen::Reloaded { before, after } => judge.check(before == after, "unsent words lost on reload"),
            Seen::Refused { shown } => judge.check(shown, "refusal has no visible reason"),
            Seen::Visible { address, link_live, watch_live, refused } => {
                judge.check(!link_live || watch_live, "link says live without a live watch");
                if let Address::Task { number, .. } = address {
                    judge.check(self.tasks.contains(&number), "new chat shown before durable");
                    for key in &self.submitted {
                        if self.durable.get(key) == Some(&number) {
                            let _ = judge.meet(&Name::Answer(*key));
                        }
                    }
                }
                if refused {
                    for key in &self.submitted {
                        let _ = judge.meet(&Name::Answer(*key));
                    }
                }
                if link_live {
                    let _ = judge.meet(&Name::Reconnect);
                }
            }
            Seen::WatchLost => judge.rearm(Name::Reconnect, Duration::from_secs(2)),
        }
    }
}

#[must_use]
pub fn is_link_live(tree: &temper_web_view::Tree) -> bool {
    tree.nodes().iter().enumerate().any(|(index, node)| {
        node.name.as_deref() == Some(b"Connection status".as_slice())
            && node.role == Some(temper_web_view::Role::Status)
            && tree.nodes()[index..index + usize::try_from(node.size).expect("size fits")]
                .iter()
                .any(|child| child.text.as_deref() == Some(b"Live".as_slice()))
    })
}

#[must_use]
pub fn is_refused(tree: &temper_web_view::Tree) -> bool {
    tree.nodes().iter().any(|node| {
        node.text.as_deref().is_some_and(|text| {
            [b"cannot".as_slice(), b"limit".as_slice(), b"not found".as_slice(), b"already used".as_slice()]
                .iter()
                .any(|phrase| text.windows(phrase.len()).any(|window| window.eq_ignore_ascii_case(phrase)))
        })
    })
}

pub fn observe_at(referee: &mut temper_world::Referee<Rules>, now: Time, seen: Seen) {
    let mut stimuli = Vec::new();
    referee.observe(now, seen, &mut stimuli);
    assert!(stimuli.is_empty(), "referee has no immediate stimuli");
}

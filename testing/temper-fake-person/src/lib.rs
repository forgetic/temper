//! A person acts through an accessible face, one step at a time. The face
//! may answer immediately (the view tree) or after a browser round trip.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

mod random;
#[cfg(test)]
mod tests;
mod tree;

use alloc::boxed::Box;
use skein_lib::{Duration, Time};
use temper_web_domain::Address;
use temper_web_view::{NodeId, Role};

pub use random::RandomPerson;
pub use tree::{TreeFace, dom_event};

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Find {
    pub within: Box<[Target]>,
    pub target: Target,
}

impl Find {
    #[must_use]
    pub fn named(role: Role, name: &[u8]) -> Find {
        Find { within: Box::new([]), target: Target::Role { role, name: Box::from(name) } }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Role { role: Role, name: Box<[u8]> },
    Text { text: Box<[u8]> },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    Go { address: Address },
    Press { find: Find },
    Type { find: Find, words: Box<[u8]> },
    Send { find: Find },
    See { find: Find, within: Duration },
    Gone { find: Find, within: Duration },
    Reload,
    Pause { span: Duration },
}

#[derive(Debug)]
pub struct Person {
    script: Box<[Step]>,
    at: u32,
    since: Option<Time>,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Next {
    Do(Doing),
    Wait { until: Option<Time> },
    Done,
    Failed { step: u32, why: Why },
}

#[derive(PartialEq, Eq, Debug)]
pub enum Doing {
    Press { node: NodeId },
    Type { node: NodeId, words: Box<[u8]> },
    Send { node: NodeId },
    Go { address: Address },
    Reload,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why {
    Missing,
    Unexpected,
    Timeout,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Found {
    Node(NodeId),
    Absent,
    Pending,
}

pub trait Face {
    fn find(&mut self, find: &Find) -> Found;
}

impl Person {
    #[must_use]
    pub fn new(script: Box<[Step]>) -> Person {
        Person { script, at: 0, since: None }
    }

    #[must_use]
    pub fn at(&self) -> u32 {
        self.at
    }

    fn advance(&mut self) {
        self.at = self.at.saturating_add(1);
        self.since = None;
    }

    /// Query the face and advance at most one script step.
    pub fn next<F: Face>(&mut self, face: &mut F, now: Time) -> Next {
        let Some(step) = self.script.get(usize::try_from(self.at).expect("step index fits")) else {
            return Next::Done;
        };
        let since = *self.since.get_or_insert(now);
        let result = match step {
            Step::Go { address } => Next::Do(Doing::Go { address: *address }),
            Step::Reload => Next::Do(Doing::Reload),
            Step::Pause { span } => {
                let until = since.saturating_add(*span);
                if now >= until {
                    self.advance();
                    return Next::Wait { until: None };
                }
                Next::Wait { until: Some(until) }
            }
            Step::Press { find } | Step::Type { find, .. } | Step::Send { find } => match face.find(find) {
                Found::Node(node) => match step {
                    Step::Press { .. } => Next::Do(Doing::Press { node }),
                    Step::Type { words, .. } => Next::Do(Doing::Type { node, words: words.clone() }),
                    Step::Send { .. } => Next::Do(Doing::Send { node }),
                    Step::Go { .. } | Step::See { .. } | Step::Gone { .. } | Step::Reload | Step::Pause { .. } => {
                        unreachable!("matched interactive step")
                    }
                },
                Found::Absent => Next::Failed { step: self.at, why: Why::Missing },
                Found::Pending => Next::Wait { until: None },
            },
            Step::See { find, within } | Step::Gone { find, within } => {
                let found = face.find(find);
                let met = match (step, found) {
                    (Step::See { .. }, Found::Node(_)) | (Step::Gone { .. }, Found::Absent) => true,
                    (Step::See { .. } | Step::Gone { .. }, Found::Absent | Found::Node(_) | Found::Pending)
                    | (
                        Step::Go { .. }
                        | Step::Press { .. }
                        | Step::Type { .. }
                        | Step::Send { .. }
                        | Step::Reload
                        | Step::Pause { .. },
                        _,
                    ) => false,
                };
                if met {
                    self.advance();
                    return Next::Wait { until: None };
                }
                let until = since.saturating_add(*within);
                if now >= until {
                    Next::Failed { step: self.at, why: Why::Timeout }
                } else {
                    Next::Wait { until: Some(until) }
                }
            }
        };
        if let Next::Do(_) = result {
            self.advance();
        }
        result
    }
}

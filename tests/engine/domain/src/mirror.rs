//! What the forge shows, as the world sees it from outside: built only from
//! the fake forge's observations (testing.md, 4.2), never from its store or
//! from the engine's state. People and the scripted workers look at it to
//! decide what to do, as a person looks at the forge's pages, and the referee
//! judges against it.

use std::collections::BTreeMap;

use temper_engine_domain::{Decoded, Posted, Record};
use temper_fake_forge_domain::Observation;
use temper_fake_forge_domain::api::{Check, Kind, Verdict};

use crate::codec;

/// An issue or a pull request, as observed.
#[derive(Clone, Debug)]
pub struct Issue {
    pub kind: Kind,
    pub title: Vec<u8>,
    pub body: Vec<u8>,
    pub labels: Vec<Vec<u8>>,
    pub open: bool,
    pub by: u64,
    pub comments: Vec<Comment>,
    /// A pull request's branches and head, and the commit its merge made.
    pub pull: Option<Pull>,
    pub reviews: Vec<Review>,
}

#[derive(Clone, Debug)]
pub struct Comment {
    pub id: u64,
    pub body: Vec<u8>,
    pub by: u64,
    /// What the body holds of the engine's, decoded once as it was observed.
    pub decoded: Option<Decoded>,
}

impl Comment {
    fn new(id: u64, body: &[u8], by: u64) -> Comment {
        Comment { id, body: body.to_vec(), by, decoded: codec::comment(id, body) }
    }
}

#[derive(Clone, Debug)]
pub struct Pull {
    pub head: Vec<u8>,
    pub base: Vec<u8>,
    pub commit: u64,
    pub merged: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
pub struct Review {
    pub commit: u64,
    pub verdict: Verdict,
    pub by: u64,
}

/// The forge as observed, by repository's name.
#[derive(Default, Debug)]
pub struct Mirror {
    /// The issues and pull requests, by repository's name, then number.
    pub issues: BTreeMap<Vec<u8>, BTreeMap<u64, Issue>>,
    /// Each commit's latest state per context, by repository.
    pub statuses: BTreeMap<(Vec<u8>, u64), BTreeMap<Vec<u8>, Check>>,
    pub branches: BTreeMap<(Vec<u8>, Vec<u8>), u64>,
    pub pages: BTreeMap<(Vec<u8>, Vec<u8>), Vec<u8>>,
}

impl Mirror {
    /// Takes in what the forge did.
    #[expect(clippy::too_many_lines, reason = "one arm per observation")]
    pub fn observe(&mut self, observation: &Observation) {
        match observation {
            Observation::Opened { repository, number, kind, title, body, labels, branches, by } => {
                let pull = branches.as_ref().map(|branches| Pull {
                    head: branches.head.to_vec(),
                    base: branches.base.to_vec(),
                    commit: branches.commit,
                    merged: None,
                });
                let issue = Issue {
                    kind: *kind,
                    title: title.to_vec(),
                    body: body.to_vec(),
                    labels: labels.iter().map(|label| label.to_vec()).collect(),
                    open: true,
                    by: *by,
                    comments: Vec::new(),
                    pull,
                    reviews: Vec::new(),
                };
                self.issues.entry(repository.to_vec()).or_default().insert(*number, issue);
            }
            Observation::Closed { repository, number, .. } => {
                if let Some(issue) = self.issue_mut(repository, *number) {
                    issue.open = false;
                }
            }
            Observation::Reopened { repository, number, .. } => {
                if let Some(issue) = self.issue_mut(repository, *number) {
                    issue.open = true;
                }
            }
            Observation::Labelled { repository, number, labels, .. } => {
                if let Some(issue) = self.issue_mut(repository, *number) {
                    issue.labels = labels.iter().map(|label| label.to_vec()).collect();
                }
            }
            Observation::Revised { repository, number, title, body, .. } => {
                if let Some(issue) = self.issue_mut(repository, *number) {
                    issue.title = title.to_vec();
                    issue.body = body.to_vec();
                }
            }
            Observation::Commented { repository, number, id, body, by } => {
                if let Some(issue) = self.issue_mut(repository, *number) {
                    issue.comments.push(Comment::new(*id, body, *by));
                }
            }
            Observation::Edited { repository, number, id, body, .. } => {
                if let Some(issue) = self.issue_mut(repository, *number)
                    && let Some(comment) = issue.comments.iter_mut().find(|comment| comment.id == *id)
                {
                    *comment = Comment::new(*id, body, comment.by);
                }
            }
            Observation::Removed { repository, number, id, .. } => {
                if let Some(issue) = self.issue_mut(repository, *number) {
                    issue.comments.retain(|comment| comment.id != *id);
                }
            }
            Observation::Reviewed { repository, number, commit, verdict, by, .. } => {
                if let Some(issue) = self.issue_mut(repository, *number) {
                    issue.reviews.push(Review { commit: *commit, verdict: *verdict, by: *by });
                }
            }
            Observation::Reported { repository, commit, context, state, .. } => {
                self.statuses.entry((repository.to_vec(), *commit)).or_default().insert(context.to_vec(), *state);
            }
            Observation::Merged { repository, number, commit, .. } => {
                if let Some(issue) = self.issue_mut(repository, *number)
                    && let Some(pull) = &mut issue.pull
                {
                    pull.merged = Some(*commit);
                    issue.open = false;
                }
            }
            Observation::Moved { repository, branch, to, .. } => {
                self.branches.insert((repository.to_vec(), branch.to_vec()), *to);
                for issue in self.issues.get_mut(&**repository).into_iter().flat_map(BTreeMap::values_mut) {
                    if !issue.open {
                        continue;
                    }
                    if let Some(pull) = &mut issue.pull
                        && *pull.head == **branch
                    {
                        pull.commit = *to;
                    }
                }
            }
            Observation::Deleted { repository, branch, .. } => {
                self.branches.remove(&(repository.to_vec(), branch.to_vec()));
            }
            Observation::Wiki { repository, name, content, .. } => match content {
                Some(content) => {
                    self.pages.insert((repository.to_vec(), name.to_vec()), content.to_vec());
                }
                None => {
                    self.pages.remove(&(repository.to_vec(), name.to_vec()));
                }
            },
            Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Refused { .. }
            | Observation::Rejected { .. } => {}
        }
    }

    #[must_use]
    pub fn issue(&self, repository: &[u8], number: u64) -> Option<&Issue> {
        self.issues.get(repository)?.get(&number)
    }

    fn issue_mut(&mut self, repository: &[u8], number: u64) -> Option<&mut Issue> {
        self.issues.get_mut(repository)?.get_mut(&number)
    }

    /// Every issue and pull request, with its repository's name and its
    /// number, in that order.
    pub fn items(&self) -> impl Iterator<Item = (&[u8], u64, &Issue)> {
        self.issues.iter().flat_map(|(repository, issues)| {
            issues.iter().map(move |(number, issue)| (repository.as_slice(), *number, issue))
        })
    }

    /// The record the engine keeps on an item, if it has one that decodes:
    /// the last of its comments that holds one.
    #[must_use]
    pub fn record(&self, repository: &[u8], number: u64) -> Option<&Record> {
        let issue = self.issue(repository, number)?;
        issue.comments.iter().rev().find_map(|comment| match comment.decoded.as_ref()? {
            Decoded::Record { record, .. } => Some(&**record),
            Decoded::Outcome { .. } | Decoded::Page { .. } => None,
        })
    }

    /// The outcomes posted on an item, in order.
    #[must_use]
    pub fn outcomes(&self, repository: &[u8], number: u64) -> Vec<Posted> {
        let Some(issue) = self.issue(repository, number) else {
            return Vec::new();
        };
        issue
            .comments
            .iter()
            .filter_map(|comment| match comment.decoded.as_ref()? {
                Decoded::Outcome { posted, .. } => Some(Posted::clone(posted)),
                Decoded::Record { .. } | Decoded::Page { .. } => None,
            })
            .collect()
    }

    /// How many comments on an item a run wrote through its outlet: the
    /// engine's that are neither a record, nor an outcome, nor written for
    /// a person.
    #[must_use]
    pub fn words(&self, repository: &[u8], number: u64, engine: u64) -> usize {
        let Some(issue) = self.issue(repository, number) else {
            return 0;
        };
        issue
            .comments
            .iter()
            .filter(|comment| comment.by == engine && comment.decoded.is_none() && !is_for_person(&comment.body))
            .count()
    }

    /// How many comments on an item are people's: written on the forge by
    /// a person, or by the engine for one.
    #[must_use]
    pub fn messages(&self, repository: &[u8], number: u64, engine: u64) -> usize {
        let Some(issue) = self.issue(repository, number) else {
            return 0;
        };
        issue.comments.iter().filter(|comment| comment.by != engine || is_for_person(&comment.body)).count()
    }

    /// Whether every context on `commit` in `repository` passed, and one at
    /// least reported.
    #[must_use]
    pub fn is_green(&self, repository: &[u8], commit: u64) -> bool {
        match self.statuses.get(&(repository.to_vec(), commit)) {
            Some(states) => !states.is_empty() && states.values().all(|state| *state == Check::Passed),
            None => false,
        }
    }
}

/// Whether a comment the engine wrote is written for a person.
fn is_for_person(body: &[u8]) -> bool {
    use temper_engine_domain::forge::api::Mark;
    match temper_engine_forge_world::translate::mark(body) {
        Mark::Key { person, .. } => person.is_some(),
        Mark::None | Mark::Record { .. } | Mark::Mangled => false,
    }
}

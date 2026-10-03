//! Scripted people on the forge (testing-pyramid.md, 4.4): a reviewer, who
//! looks at the forge as observed ([`Mirror`]) every so often and approves
//! every pull request the engine opens once CI passed on its exact head,
//! once per head. What people hand in is the desk's ([`crate::desk`]); a
//! person who stops a run asks the engine through its web, as the world
//! draws it.

use std::collections::BTreeSet;

use temper_engine_model_tests::deployment::{ENGINE, REPOSITORIES, REVIEWER};
use temper_engine_model_tests::mirror::Mirror;
use temper_forge_model::api::{Kind, Op, Verdict, Write};

/// A review the reviewer makes: of the pull request `number` of the
/// deployment's repository `repository`, at `head`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Review {
    pub repository: usize,
    pub number: u64,
    pub head: u64,
    pub op: Op,
}

/// The reviewer.
#[derive(Default, Debug)]
pub struct Reviewer {
    /// Reviews in flight, by repository, pull request and head.
    reviewing: BTreeSet<(usize, u64, u64)>,
    /// Reviews made.
    pub reviews: u32,
}

impl Reviewer {
    /// The reviews due now, as the forge is observed.
    pub fn look(&mut self, mirror: &Mirror, out: &mut Vec<Review>) {
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
            self.reviews += 1;
            let op = Op::Write(Write::Review {
                number,
                verdict: Some(Verdict::Approve),
                body: b"looks good".as_slice().into(),
            });
            out.push(Review { repository: at, number, head, op });
        }
    }

    /// A review's call ended.
    pub fn reviewed(&mut self, repository: usize, number: u64, head: u64) {
        self.reviewing.remove(&(repository, number, head));
    }

    /// Whether no review is in flight.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.reviewing.is_empty()
    }
}

//! Forge judgements over a change's own facts (jig's domain/connectors.md, section 7).
//!
//! The connector keeps each criterion's parameters. The root supplies verified
//! project roles for authenticated reviewers; it does not interpret the forge's
//! CI, containment, gate or review facts. Judgements are for one named head.

use alloc::boxed::Box;

use temper_engine_domain_forge_change as change;
use temper_engine_domain_forge_client as client;

use crate::{ChangeEvidence, Repository};

/// Which prior head can still carry a gate or review verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Freshness {
    /// The report names the landing head itself.
    Exact,
    /// The report names the head or a verified clean predecessor.
    Clean,
}

/// Parameters for one forge requirement, retained by the connector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Criterion {
    /// CI at the named head, or all adopted checks where CI is unavailable.
    Ci,
    /// The change contains the observed base tip.
    UpToDate,
    /// One blocking check or agent gate.
    Gate { number: u32, freshness: Freshness },
    /// Distinct approvals by authenticated people in a project role.
    Approval { role: u32, people: u32, freshness: Freshness },
}

/// Verdict returned to the root for one criterion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// The criterion holds at this head.
    Met,
    /// Facts are absent, incomplete or pending.
    Wait,
    /// A required fact failed.
    Refuse,
}

/// A reviewer identity and project role verified by the root's people child.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reviewer {
    pub person: u64,
    pub role: u32,
    pub head: client::api::Commit,
    pub verdict: client::api::Verdict,
}

/// Connector-owned configuration for requirements named by authority policy.
#[derive(Debug)]
pub struct Judges {
    pub deployment: Box<[Criterion]>,
    pub projects: skein_lib::Map<u32, Box<[Criterion]>>,
}

impl Judges {
    /// An empty configuration for a connector with no landing requirements.
    #[must_use]
    pub fn empty(projects: u32) -> Judges {
        Judges { deployment: Box::new([]), projects: skein_lib::Map::with_capacity(projects) }
    }
}

pub(crate) fn criterion(judges: &Judges, project: u32, parameters: u32) -> Option<Criterion> {
    let index = usize::try_from(parameters & 0x7fff_ffff).ok()?;
    if parameters & 0x8000_0000 == 0 {
        judges.deployment.get(index).copied()
    } else {
        judges.projects.get(&project)?.get(index).copied()
    }
}

fn status(value: change::Status) -> Verdict {
    match value {
        change::Status::Unknown | change::Status::Pending => Verdict::Wait,
        change::Status::Passed => Verdict::Met,
        change::Status::Failed => Verdict::Refuse,
    }
}

fn valid(
    report: client::api::Commit,
    head: client::api::Commit,
    clean: &[client::api::Commit],
    freshness: Freshness,
) -> bool {
    if report == head {
        return true;
    }
    match freshness {
        Freshness::Exact => false,
        Freshness::Clean => clean.contains(&report),
    }
}

fn gate(
    number: u32,
    freshness: Freshness,
    head: client::api::Commit,
    clean: &[client::api::Commit],
    evidence: &ChangeEvidence,
) -> Verdict {
    let mut met = false;
    let mut waiting = false;
    for report in &evidence.gates {
        if report.number != u64::from(number) || !valid(report.head, head, clean, freshness) {
            continue;
        }
        match status(report.status) {
            Verdict::Met => met = true,
            Verdict::Wait => waiting = true,
            Verdict::Refuse => return Verdict::Refuse,
        }
    }
    if waiting || !met { Verdict::Wait } else { Verdict::Met }
}

fn approval(
    role: u32,
    people: u32,
    freshness: Freshness,
    head: client::api::Commit,
    clean: &[client::api::Commit],
    reviewers: &[Reviewer],
    complete: bool,
) -> Verdict {
    if !complete {
        return Verdict::Wait;
    }
    let mut have = 0_u32;
    for (position, reviewer) in reviewers.iter().enumerate() {
        if reviewer.role != role || !valid(reviewer.head, head, clean, freshness) {
            continue;
        }
        match reviewer.verdict {
            client::api::Verdict::RequestChanges => return Verdict::Refuse,
            client::api::Verdict::Comment => {}
            client::api::Verdict::Approve => {
                let mut duplicate = false;
                let mut conflicting = false;
                for other in reviewers {
                    if other.person == reviewer.person
                        && other.role == role
                        && valid(other.head, head, clean, freshness)
                    {
                        match other.verdict {
                            client::api::Verdict::Approve => {}
                            client::api::Verdict::RequestChanges | client::api::Verdict::Comment => conflicting = true,
                        }
                    }
                }
                for earlier in reviewers.get(..position).expect("review position in bounds") {
                    duplicate |= earlier.person == reviewer.person
                        && earlier.role == role
                        && valid(earlier.head, head, clean, freshness)
                        && earlier.verdict == client::api::Verdict::Approve;
                }
                if !duplicate && !conflicting {
                    have = have.checked_add(1).expect("admitted reviewer bound fits u32");
                }
            }
        }
    }
    if have < people { Verdict::Wait } else { Verdict::Met }
}

/// Judge one configured criterion against the change's coherent evidence.
#[must_use]
pub fn judge(
    criterion: Criterion,
    repository: &Repository,
    head: client::api::Commit,
    clean: &[client::api::Commit],
    evidence: &ChangeEvidence,
    reviewers: &[Reviewer],
) -> Verdict {
    if evidence.head != Some(head) {
        return Verdict::Wait;
    }
    match criterion {
        Criterion::Ci => {
            if repository.ci {
                status(evidence.ci)
            } else if repository.checks.is_empty() {
                Verdict::Refuse
            } else {
                let mut result = Verdict::Met;
                for number in &repository.checks {
                    match gate(*number, Freshness::Exact, head, clean, evidence) {
                        Verdict::Refuse => return Verdict::Refuse,
                        Verdict::Wait => result = Verdict::Wait,
                        Verdict::Met => {}
                    }
                }
                result
            }
        }
        Criterion::UpToDate => status(evidence.contains_base),
        Criterion::Gate { number, freshness } => gate(number, freshness, head, clean, evidence),
        Criterion::Approval { role, people, freshness } => {
            approval(role, people, freshness, head, clean, reviewers, evidence.reviews_complete)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Freshness, Reviewer, Verdict, approval};
    use temper_engine_domain_forge_client::api;

    #[test]
    fn approvals_count_distinct_people_at_the_named_head() {
        let head = [1; 32];
        let old = [2; 32];
        let review = |person, role, head, verdict| Reviewer { person, role, head, verdict };
        let approved = api::Verdict::Approve;
        let first = review(7, 3, head, approved);
        let second = review(8, 3, head, approved);
        assert_eq!(approval(3, 2, Freshness::Exact, head, &[], &[first, first], true), Verdict::Wait);
        assert_eq!(approval(3, 2, Freshness::Exact, head, &[], &[first, second], false), Verdict::Wait);
        assert_eq!(approval(3, 2, Freshness::Exact, head, &[], &[first, second], true), Verdict::Met);
        assert_eq!(
            approval(3, 2, Freshness::Exact, head, &[], &[first, review(8, 4, head, approved)], true),
            Verdict::Wait
        );
        assert_eq!(
            approval(3, 2, Freshness::Exact, head, &[], &[first, review(8, 3, old, approved)], true),
            Verdict::Wait
        );
        assert_eq!(
            approval(3, 2, Freshness::Clean, head, &[old], &[first, review(8, 3, old, approved)], true),
            Verdict::Met
        );
        assert_eq!(
            approval(
                3,
                2,
                Freshness::Exact,
                head,
                &[],
                &[first, second, review(7, 3, head, api::Verdict::RequestChanges)],
                true
            ),
            Verdict::Refuse
        );
    }
}

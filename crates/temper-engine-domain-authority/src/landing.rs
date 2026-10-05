//! Landing requirements over owned pinned facts, borrowed without allocation
//! (domain/authority.md, sections 8.2 and 10; domain/forge.md, 8.3).

use skein_lib::Queue;

use crate::{
    Answer, Approval, Domain, Effect, EffectAsk, Finding, Freshness, Gate, Head, Landing, LandingRule, Policy, Review,
    Status, pattern_covers,
};

fn find(answer: &mut Answer, why: &mut Queue<Finding>, strict: Answer, finding: Finding) {
    *answer = (*answer).max(strict);
    why.push(finding);
}

fn applies(rule: &LandingRule, effect: &Effect) -> bool {
    rule.connector == effect.connector && rule.kind == effect.kind && pattern_covers(&rule.pattern, &effect.name)
}

pub(crate) fn check(domain: &Domain, policy: &Policy, ask: &EffectAsk, answer: &mut Answer, why: &mut Queue<Finding>) {
    let mut required = false;
    for rules in [&domain.rules().landing, &policy.landing] {
        for rule in rules {
            if applies(rule, &ask.effect) {
                required = true;
                for approval in &rule.approvals {
                    if domain.role(ask.project, approval.role).is_none() {
                        find(answer, why, Answer::Refuse, Finding::UnknownRole);
                    }
                }
            }
        }
    }
    let Some(landing) = &ask.landing else {
        if required {
            find(answer, why, Answer::Wait, Finding::LandingMissing);
        }
        return;
    };
    if landing.head != ask.effect.state {
        find(answer, why, Answer::Refuse, Finding::LandingPin);
        return;
    }
    for rules in [&domain.rules().landing, &policy.landing] {
        for rule in rules {
            if !applies(rule, &ask.effect) {
                continue;
            }
            if rule.ci {
                let status = if landing.ci.head == landing.head { landing.ci.status } else { Status::Unknown };
                status_check(status, Finding::Ci { status }, answer, why);
            }
            if rule.up_to_date {
                status_check(landing.contains_tip, Finding::Behind { status: landing.contains_tip }, answer, why);
            }
            for gate in &rule.gates {
                gate_check(*gate, landing, answer, why);
            }
            for approval in &rule.approvals {
                // A missing role has already refused independently of facts.
                if domain.role(ask.project, approval.role).is_some() {
                    approval_check(approval, landing, answer, why);
                }
            }
        }
    }
    for gate in &landing.gates {
        gate_check(*gate, landing, answer, why);
    }
}

fn status_check(status: Status, finding: Finding, answer: &mut Answer, why: &mut Queue<Finding>) {
    match status {
        Status::Passed => {}
        Status::Unknown | Status::Pending => find(answer, why, Answer::Wait, finding),
        Status::Failed => find(answer, why, Answer::Refuse, finding),
    }
}

fn valid(head: &Head, freshness: Freshness, landing: &Landing) -> bool {
    if *head == landing.head {
        return true;
    }
    match freshness {
        Freshness::Exact => false,
        Freshness::Clean => landing.clean.contains(head),
    }
}

fn gate_check(gate: Gate, landing: &Landing, answer: &mut Answer, why: &mut Queue<Finding>) {
    if !gate.blocking {
        return;
    }
    let mut passed = false;
    let mut pending = false;
    let mut failed = false;
    for verdict in &landing.verdicts {
        if verdict.gate != gate.number || !valid(&verdict.head, gate.freshness, landing) {
            continue;
        }
        match verdict.status {
            Status::Passed => passed = true,
            Status::Unknown | Status::Pending => pending = true,
            Status::Failed => failed = true,
        }
    }
    let status = if failed {
        Status::Failed
    } else if pending {
        Status::Pending
    } else if passed {
        Status::Passed
    } else {
        Status::Unknown
    };
    status_check(status, Finding::Gate { number: gate.number, status }, answer, why);
}

fn eligible(review: &Review, approval: &Approval, landing: &Landing) -> bool {
    review.role == approval.role && valid(&review.head, approval.freshness, landing)
}

/// Conflicting reports by one person never increase the approval count.
fn uncertain(person: u64, approval: &Approval, landing: &Landing) -> bool {
    for review in &landing.reviews {
        if review.person != person || !eligible(review, approval, landing) {
            continue;
        }
        match review.status {
            Status::Passed => {}
            Status::Unknown | Status::Pending | Status::Failed => return true,
        }
    }
    false
}

fn approval_check(approval: &Approval, landing: &Landing, answer: &mut Answer, why: &mut Queue<Finding>) {
    let mut failed = false;
    let mut have = 0_u32;
    for (position, review) in landing.reviews.iter().enumerate() {
        if !eligible(review, approval, landing) {
            continue;
        }
        match review.status {
            Status::Failed => failed = true,
            Status::Unknown | Status::Pending => {}
            Status::Passed => {
                let mut duplicate = false;
                for earlier in landing.reviews.get(..position).expect("enumerated review position is in bounds") {
                    duplicate |= earlier.person == review.person
                        && eligible(earlier, approval, landing)
                        && earlier.status == Status::Passed;
                }
                if !duplicate && !uncertain(review.person, approval, landing) {
                    have = have.checked_add(1).expect("review admission bounds the distinct count by u32");
                }
            }
        }
    }
    if failed {
        find(answer, why, Answer::Refuse, Finding::ReviewFailed { role: approval.role });
    }
    if have < approval.people {
        find(answer, why, Answer::Wait, Finding::Approval { role: approval.role, have, want: approval.people });
    }
}

//! Held person-chat decisions, independent of root authentication, policy and
//! transport history (domain/tasks.md, section 8).

use crate::domain::{Domain, activate, publish, record, task_mut};
use crate::{Active, Hold, Limits, Party, Phase, Request, Tries, Was};
use alloc::boxed::Box;
use skein_lib::{Env, Queue, ReplyTo};

/// Semantic recipient of one held chat decision. Root verifies current membership
/// and coverage; tasks keeps only this bounded identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EscalationHolder {
    /// The chat's person requester, eligible under current policy.
    Person(
        /// Positive requester identity, supplied by root.
        u64,
    ),
    /// Final project policy role; any authenticated current holder may decide
    /// first, and passing further refuses.
    Role {
        /// Project of the held chat, checked by tasks.
        project: u32,
        /// Policy role selected and checked by root, bounded by authority roles.
        role: u32,
    },
}

/// One semantic state per task; no receipt table, inbox or historical archive.
/// Root archives decisions; rejected chats remain held.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Escalation {
    /// No current held decision; retain the last revision to prevent reuse
    Unheld {
        /// Zero initially; checked revision advances on each hold/pass
        revision: u64,
    },
    /// Root owes an eligible requester or final policy role in this same atomic
    /// decision; not a durable unresolved route.
    Routing {
        /// Positive per-task semantic revision.
        revision: u64,
    },
    /// One durable pending decision; no task activation can escape the hold
    Waiting {
        /// Positive revision echoed by reads and decisions.
        revision: u64,
        /// Root-resolved current recipient; membership stays root's concern
        holder: EscalationHolder,
    },
    /// One completed rejection; retain reason while held without reopening or
    /// rerouting. Historical transport outcome stays root-owned.
    Rejected {
        revision: u64,
        by: u64,
        /// Rejection words, at most task `result_bytes`.
        reason: Box<[u8]>,
    },
}

impl Escalation {
    /// Pure projection of this task's checked semantic revision; no allocation,
    /// mutation or root-issued identity is involved.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        match self {
            Escalation::Unheld { revision }
            | Escalation::Routing { revision }
            | Escalation::Waiting { revision, .. }
            | Escalation::Rejected { revision, .. } => *revision,
        }
    }
}

/// Root-authorized semantic choice for the exact waiting revision. No authority
/// widening or generic release surface is implied.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum EscalationDecision {
    /// Lift a retry-exhaustion hold, reset tries, and rejudge readiness.
    Release,
    /// Decide once while leaving the task held.
    Reject {
        /// At most `result_bytes`, checked before mutation.
        reason: Box<[u8]>,
    },
    /// Persist the root-verified final policy role and advance revision; a role
    /// holder cannot pass further.
    Pass {
        /// Final policy role, checked by root and structurally by tasks
        holder: EscalationHolder,
    },
}

/// Semantic terminal for one root decision call. Root commits accepted state
/// and its own typed archive with people's keyed answer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EscalationOutcome {
    /// Retry hold lifted, with normal activation consequences.
    Released,
    /// Reason retained; no activation or automatic reroute.
    Rejected,
    /// Waiting moved to the final policy role.
    Passed {
        /// New checked pending revision.
        revision: u64,
    },
    /// No exact live pending revision; root may read its own historical outcome
    Stale,
    /// A policy-role decision cannot pass further.
    NoFurther,
    /// This hold requires a real amendment route, currently absent
    NeedsAmend,
    /// Input/revision capacity failed before mutation.
    Limit,
}

/// Temporary task-to-root held view; not a second task/funding ledger. Root
/// authenticates reads and decisions against this exact context.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EscalationContext {
    /// Positive held task identity.
    pub task: u64,
    /// Policy project checked by root.
    pub project: u32,
    pub requester: u64,
    /// Preserved semantic hold reason.
    pub why: Hold,
    /// Exactly one bounded current revision/status; rejection owns at most
    /// `result_bytes`.
    pub escalation: Escalation,
}

pub(crate) fn context(domain: &Domain, number: u64) -> Option<Box<EscalationContext>> {
    let task = record(domain, number)?;
    let requester = match task.requester {
        Party::Person(person) => person,
        Party::Task(_) | Party::Deployment { .. } => return None,
    };
    let why = match &task.phase {
        Phase::Held { why, .. } => *why,
        Phase::Waiting | Phase::Active(_) | Phase::Closing(_) | Phase::Ended(_) => return None,
    };
    Some(Box::new(EscalationContext {
        task: number,
        project: task.project,
        requester,
        why,
        escalation: task.escalation.clone(),
    }))
}

pub(crate) fn begin(record: &mut crate::TaskRecord) -> bool {
    match record.requester {
        Party::Person(_) => {}
        Party::Task(_) | Party::Deployment { .. } => return false,
    }
    match record.phase {
        Phase::Held { .. } => {}
        Phase::Waiting | Phase::Active(_) | Phase::Closing(_) | Phase::Ended(_) => {
            // Ending/cancellation retires current semantic decision state; root owns history.
            record.escalation = Escalation::Unheld { revision: record.escalation.revision() };
            return false;
        }
    }
    let revision = match record.escalation {
        Escalation::Unheld { revision } => revision,
        Escalation::Routing { .. } | Escalation::Waiting { .. } | Escalation::Rejected { .. } => return false,
    };
    record.escalation = Escalation::Routing {
        revision: revision.checked_add(1).expect("release and restore reserve the next revision"),
    };
    true
}

pub(crate) fn routed(
    domain: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    revision: u64,
    holder: EscalationHolder,
    out: &mut Queue<Request>,
) {
    let Some(task) = record(domain, number) else { return };
    let old = match task.escalation {
        Escalation::Routing { revision: current } if current == revision => None,
        Escalation::Waiting { revision: current, holder } if current == revision => Some(holder),
        Escalation::Unheld { .. }
        | Escalation::Routing { .. }
        | Escalation::Waiting { .. }
        | Escalation::Rejected { .. } => return,
    };
    if old == Some(holder) {
        return;
    }
    let revision = match old {
        Some(_) => match revision.checked_add(1) {
            Some(revision) => revision,
            None => return,
        },
        None => revision,
    };
    let valid = match holder {
        EscalationHolder::Person(person) => task.requester == Party::Person(person) && person != 0,
        EscalationHolder::Role { project, .. } => project == task.project,
    };
    if !valid {
        return;
    }
    task_mut(domain, number).expect("routed task exists").record.escalation = Escalation::Waiting { revision, holder };
    publish(domain, env, number, out);
}

#[expect(
    clippy::too_many_arguments,
    reason = "one explicit semantic event carries its identity, revision, actor, choice and terminal destination"
)]
pub(crate) fn decide(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    revision: u64,
    by: u64,
    decision: EscalationDecision,
    out: &mut Queue<Request>,
) {
    let mut outcome = EscalationOutcome::Stale;
    if domain.ready()
        && by != 0
        && let Some(task) = record(domain, number)
    {
        match task.escalation {
            Escalation::Waiting { revision: current, holder } if current == revision => {
                let standing = match holder {
                    EscalationHolder::Person(person) => person == by,
                    EscalationHolder::Role { .. } => true,
                };
                if standing {
                    outcome = validate(task, &decision, holder, &env.limits);
                }
            }
            Escalation::Unheld { .. }
            | Escalation::Routing { .. }
            | Escalation::Waiting { .. }
            | Escalation::Rejected { .. } => {}
        }
    }
    let changed = match outcome {
        EscalationOutcome::Released | EscalationOutcome::Rejected | EscalationOutcome::Passed { .. } => true,
        EscalationOutcome::Stale
        | EscalationOutcome::NoFurther
        | EscalationOutcome::NeedsAmend
        | EscalationOutcome::Limit => false,
    };
    if changed {
        let task = task_mut(domain, number).expect("decision validated live task");
        match decision {
            EscalationDecision::Release => {
                task.record.escalation = Escalation::Unheld { revision };
                task.record.tries = Tries::NONE;
                task.record.refusals = 0;
                task.record.phase = Phase::Active(Active::Due);
            }
            EscalationDecision::Reject { reason } => {
                task.record.escalation = Escalation::Rejected { revision, by, reason }
            }
            EscalationDecision::Pass { holder } => {
                task.record.escalation =
                    Escalation::Waiting { revision: revision.checked_add(1).expect("pass preflight"), holder }
            }
        }
        publish(domain, env, number, out);
        if outcome == EscalationOutcome::Released {
            activate(domain, number, out);
        }
    }
    out.push(Request::EscalationDecided { reply_to: to, task: number, revision, outcome });
}

fn validate(
    task: &crate::TaskRecord,
    decision: &EscalationDecision,
    holder: EscalationHolder,
    limits: &Limits,
) -> EscalationOutcome {
    match decision {
        EscalationDecision::Release => {
            // Current slice can reopen only a retry-exhausted active task; other holds
            // need amendment routes and answer NeedsAmend here.
            if task.escalation.revision() == u64::MAX {
                return EscalationOutcome::Limit;
            }
            match &task.phase {
                Phase::Held { why: Hold::Failures(_), was: Was::Active(Active::Due) } => EscalationOutcome::Released,
                Phase::Waiting | Phase::Active(_) | Phase::Closing(_) | Phase::Held { .. } | Phase::Ended(_) => {
                    EscalationOutcome::NeedsAmend
                }
            }
        }
        EscalationDecision::Reject { reason } => {
            if reason.len() <= usize::try_from(limits.result_bytes).expect("u32 fits usize") {
                EscalationOutcome::Rejected
            } else {
                EscalationOutcome::Limit
            }
        }
        EscalationDecision::Pass { holder: next } => {
            match holder {
                EscalationHolder::Role { .. } => return EscalationOutcome::NoFurther,
                EscalationHolder::Person(_) => {}
            }
            match next {
                EscalationHolder::Person(_) => return EscalationOutcome::NoFurther,
                EscalationHolder::Role { project, .. } if *project != task.project => {
                    return EscalationOutcome::NoFurther;
                }
                EscalationHolder::Role { .. } => {}
            }
            match task.escalation.revision().checked_add(1) {
                Some(revision) => EscalationOutcome::Passed { revision },
                None => EscalationOutcome::Limit,
            }
        }
    }
}

/// Read-only root preflight/recheck snapshot, at most one Waiting context per
/// live task, with explicit readiness terminal.
pub(crate) fn project_contexts(
    domain: &Domain,
    limits: &Limits,
    project: u32,
) -> Result<Box<[EscalationContext]>, crate::Refusal> {
    if !domain.ready() {
        return Err(crate::Refusal::NotReady);
    }
    let mut contexts = skein_lib::List::with_capacity(limits.tasks);
    for (number, _) in &domain.names {
        let task = record(domain, *number).expect("live name");
        let waiting = match task.escalation {
            Escalation::Waiting { .. } => true,
            Escalation::Unheld { .. } | Escalation::Routing { .. } | Escalation::Rejected { .. } => false,
        };
        if task.project == project
            && waiting
            && let Some(context) = context(domain, *number)
        {
            contexts.push(*context).expect("one context per bounded live task");
        }
    }
    Ok(contexts.into_boxed())
}

//! Assignments and briefs assembled from their owners' parts (jig's domain/root.md, 7).
use crate::Decision;
use crate::boundary::{BriefBody, BriefConnector, BriefKind, BriefSection, ForgeBriefKind, Payload, Work};
use crate::domain::Domain;
use crate::limits::{BriefBudgets, Limits};
use jig_core_brief as brief;
use skein_lib::{Env, Id, List, Queue, Token};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_change as forge_change;

pub(crate) fn brief_outputs(
    domain: &mut Domain,
    env: &Env<Limits>,
    _decision: &mut Decision,
    out: &mut Queue<brief::GatherRequest>,
) {
    for _ in 0..out.len() {
        match out.pop().expect("brief output count") {
            brief::GatherRequest::Gather { connector: 0, section, budget } => {
                if domain.brief_connectors.get(Id::from_token(section)).is_none() {
                    continue;
                }
                domain.work.push(Work::Forge(forge::Event::GatherPlanned {
                    section,
                    parts: env.limits.brief.parts,
                    bytes: budget,
                    ci_budget: env.limits.brief.budgets.ci,
                }));
            }
            brief::GatherRequest::CutTo { connector: 0, section, size } => {
                if let Some(row) = domain.brief_connectors.get_mut(Id::from_token(section)) {
                    row.cutting = true;
                    domain.work.push(Work::Forge(forge::Event::CutBrief { section, bytes: size }));
                }
            }
            brief::GatherRequest::Drop { connector: 0, section } => {
                let id = Id::from_token(section);
                if domain.brief_connectors.get(id).is_some() {
                    domain.brief_connectors.retire(id);
                    domain.work.push(Work::Forge(forge::Event::DropBrief { section }));
                }
            }
            brief::GatherRequest::Gather { .. }
            | brief::GatherRequest::CutTo { .. }
            | brief::GatherRequest::Drop { .. } => unreachable!("temper's sole connector is numbered zero"),
            brief::GatherRequest::Complete { brief, order } => {
                let task = brief.raw();
                let mut sections = List::with_capacity(u32::try_from(order.len()).expect("bounded section count"));
                let mut missing_owner = false;
                for placed in order {
                    let section = match placed {
                        brief::GatherPlaced::Core { kind, text } => {
                            BriefSection { kind: BriefKind::Core(kind), body: BriefBody::Text(text) }
                        }
                        brief::GatherPlaced::CoreMissing { kind, why } => {
                            BriefSection { kind: BriefKind::Core(kind), body: BriefBody::Missing(why) }
                        }
                        brief::GatherPlaced::Connector { token, size, .. } => {
                            let id = Id::from_token(token);
                            let Some(row) = domain.brief_connectors.get(id) else {
                                missing_owner = true;
                                continue;
                            };
                            let kind = row.kind;
                            domain.brief_connectors.retire(id);
                            let bytes = domain.forge.take_brief(token);
                            match bytes {
                                Some(bytes) if bytes.len() == usize::try_from(size).expect("bounded section") => {
                                    BriefSection { kind: BriefKind::Forge(kind), body: BriefBody::Text(bytes) }
                                }
                                Some(_) | None => {
                                    missing_owner = true;
                                    continue;
                                }
                            }
                        }
                        brief::GatherPlaced::Missing { kind, why, .. } => BriefSection {
                            kind: BriefKind::Forge(numbered_brief_kind(kind)),
                            body: BriefBody::Missing(why),
                        },
                    };
                    sections.push(section).expect("bounded brief sections");
                }
                if missing_owner {
                    domain.work.push(Work::Core(jig_core::Event::BriefAssembled { task, ready: false }));
                    continue;
                }
                assert!(domain.brief_sections.insert(task, sections.into_boxed()) == Ok(None), "one brief assembly");
                domain.work.push(Work::Core(jig_core::Event::BriefAssembled { task, ready: true }));
            }
            brief::GatherRequest::Failed { .. } | brief::GatherRequest::Refused { .. } => {
                unreachable!("the core abandons refused brief preparation")
            }
        }
    }
}

pub(crate) fn numbered_brief_kind(kind: u16) -> ForgeBriefKind {
    match kind {
        1 => ForgeBriefKind::Ci,
        2 => ForgeBriefKind::Reviews,
        3 => ForgeBriefKind::Pull,
        _ => unreachable!("temper's forge connector uses three section kinds"),
    }
}

pub(crate) fn take_payload(domain: &mut Domain, token: Token) -> Option<Payload> {
    let id = Id::from_token(token);
    let payload = domain.payloads.get_mut(id)?.take()?;
    domain.payloads.retire(id);
    Some(payload)
}

pub(crate) fn finish_brief_plan(
    domain: &mut Domain,
    env: &Env<Limits>,
    task: u64,
    parent: Option<u64>,
    mut wanted: List<brief::Planned>,
) {
    if let Some(parent) = parent
        && let Some(row) = domain.forge.change(parent)
        && let Some((child, _)) = row.delegate
        && child == task
        && let Some(number) = row.pull
        && let Some(head) = row.change.last_head
        && wanted.room() > 0
    {
        let item = forge::BriefItem { repository: row.repository.repository, number };
        let source = match row.delegate.expect("matched delegate").1 {
            forge_change::Delegate::Repair(forge_change::Repair::Gate(_)) => {
                forge::BriefSource::Reviews { item, head: forge::BriefCommit(head) }
            }
            forge_change::Delegate::Repair(forge_change::Repair::Ci | forge_change::Repair::Semantic) => {
                forge::BriefSource::Ci { item, head: forge::BriefCommit(head) }
            }
            forge_change::Delegate::Resolve { .. } => forge::BriefSource::Pull { item, head: forge::BriefCommit(head) },
            forge_change::Delegate::Produce | forge_change::Delegate::Gate { .. } => {
                forge::BriefSource::Pull { item, head: forge::BriefCommit(head) }
            }
        };
        let kind = forge_brief_kind(source);
        let token = domain
            .brief_connectors
            .insert(BriefConnector { task, kind, cutting: false })
            .expect("reserved connector section room")
            .token();
        domain.work.push(Work::Forge(forge::Event::PlanBrief { section: token, source }));
        wanted
            .push(brief::Planned::Connector {
                connector: 0,
                kind: forge_kind_number(kind),
                token,
                size: 0,
                limit: forge_brief_budget(kind, &domain.limits.brief.budgets),
                priority: 5,
                required: true,
            })
            .expect("connector section room");
        let semantic = match row.delegate {
            Some((_, forge_change::Delegate::Repair(forge_change::Repair::Semantic))) => true,
            Some((
                _,
                forge_change::Delegate::Repair(forge_change::Repair::Ci | forge_change::Repair::Gate(_))
                | forge_change::Delegate::Resolve { .. }
                | forge_change::Delegate::Gate { .. }
                | forge_change::Delegate::Produce,
            ))
            | None => false,
        };
        if semantic && wanted.room() > 0 {
            let source = forge::BriefSource::Pull { item, head: forge::BriefCommit(head) };
            let kind = forge_brief_kind(source);
            let token = domain
                .brief_connectors
                .insert(BriefConnector { task, kind, cutting: false })
                .expect("reserved connector section room")
                .token();
            domain.work.push(Work::Forge(forge::Event::PlanBrief { section: token, source }));
            wanted
                .push(brief::Planned::Connector {
                    connector: 0,
                    kind: forge_kind_number(kind),
                    token,
                    size: 0,
                    limit: forge_brief_budget(kind, &domain.limits.brief.budgets),
                    priority: 6,
                    required: true,
                })
                .expect("semantic update section room");
        }
    }
    domain.work.push(Work::Brief(brief::GatherEvent::Plan {
        brief: Token::new(task),
        budget: domain.limits.brief.brief_bytes,
        deadline: env.now.saturating_add(domain.limits.brief.gather),
        sections: wanted.into_boxed(),
    }));
}

pub(crate) fn forge_brief_kind(source: forge::BriefSource) -> ForgeBriefKind {
    match source {
        forge::BriefSource::Ci { .. } => ForgeBriefKind::Ci,
        forge::BriefSource::Reviews { .. } => ForgeBriefKind::Reviews,
        forge::BriefSource::Pull { .. } => ForgeBriefKind::Pull,
    }
}

pub(crate) fn forge_kind_number(kind: ForgeBriefKind) -> u16 {
    match kind {
        ForgeBriefKind::Ci => 1,
        ForgeBriefKind::Reviews => 2,
        ForgeBriefKind::Pull => 3,
    }
}

pub(crate) fn forge_brief_budget(kind: ForgeBriefKind, budgets: &BriefBudgets) -> u32 {
    match kind {
        ForgeBriefKind::Ci => budgets.ci,
        ForgeBriefKind::Reviews => budgets.reviews,
        ForgeBriefKind::Pull => budgets.pull,
    }
}

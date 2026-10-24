//! Store and forge translations for jig's outside referee.
use jig_conformance::referee as r;
use jig_domain_core as core;
use jig_domain_people as people;
use jig_domain_tasks as tasks;
use jig_test_system::SystemKey;
use std::collections::{BTreeMap, BTreeSet};
use temper_engine_domain as root;
use temper_engine_domain_forge as forge;

#[must_use]
pub fn value(bytes: &[u8]) -> u64 {
    // Whole byte strings are reduced consistently at both independent boundaries.
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |n, b| (n ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3))
}
#[must_use]
pub fn repository() -> r::Name {
    r::Name {
        connector: 0,
        path: vec![b"forge".to_vec(), b"forge.example".to_vec(), b"org".to_vec(), b"repo".to_vec()],
    }
}
#[expect(clippy::wildcard_enum_match_arm, reason = "unexpected native writes fail this scenario binding explicitly")]
pub fn parts(write: &skein_forge::api::Write) -> (u16, Vec<r::Name>, Option<u64>, u64, u64) {
    let mut name = repository();
    let (kind, condition, target, state) = match write {
        skein_forge::api::Write::Status { commit, context, check } => {
            (6, None, value(format!("{context:?}:{check:?}").as_bytes()), value(commit))
        }
        skein_forge::api::Write::CreateIssue { title, body, .. } => {
            (8, None, value(format!("{title:?}:{body:?}").as_bytes()), 0)
        }
        skein_forge::api::Write::Post { number, body, .. } => {
            name.path.extend([b"issue".to_vec(), number.to_string().into_bytes()]);
            (7, None, value(body), 0)
        }
        skein_forge::api::Write::Edit { number, title, body } => {
            name.path.extend([b"issue".to_vec(), number.to_string().into_bytes()]);
            (13, None, value(format!("{title:?}:{body:?}").as_bytes()), 0)
        }
        skein_forge::api::Write::Close { number } => {
            name.path.extend([b"issue".to_vec(), number.to_string().into_bytes()]);
            (13, None, 1, 0)
        }
        skein_forge::api::Write::Merge { number, head } => {
            name.path.extend([b"branch".to_vec(), b"main".to_vec()]);
            (4, Some(value(head)), *number, value(head))
        }
        skein_forge::api::Write::OpenPull { title, body, head, base } => {
            (3, None, value(format!("{title:?}:{body:?}:{head:?}:{base:?}").as_bytes()), 0)
        }
        skein_forge::api::Write::CreateBranch { branch, commit } => {
            name.path.extend([b"branch".to_vec(), branch.to_vec()]);
            (9, None, value(commit), 0)
        }
        skein_forge::api::Write::DeleteBranch { branch } => {
            name.path.push(b"branch".to_vec());
            name.path.extend(branch.split(|byte| *byte == b'/').map(<[u8]>::to_vec));
            (15, None, 0, 0)
        }
        other => panic!("unbound forge effect {other:?}"),
    };
    (kind, vec![name], condition, target, state)
}
#[must_use]
#[expect(clippy::wildcard_enum_match_arm, reason = "only named effect call rows identify this outbox entry")]
pub fn key(store: &Store, entry: &skein_forge_client::Entry) -> SystemKey {
    let call = store.rows.values().find_map(|row| match row {
        root::Record::Core(core::Record::Core(core::CoreRecord::Call(row))) if matches!(row.part, core::CallPart::Effect { entry: n, .. } if n == entry.number) => Some(row.key),
        _ => None,
    });
    SystemKey {
        deployment: [31; 16],
        task: entry.task,
        purpose: entry.number,
        attempt: call.map_or(0, |x| x.attempt),
        completion: call.map_or(0, |x| x.completion),
        position: call.map_or(0, |x| x.position),
    }
}
#[must_use]
pub fn policy(
    seed: u64,
    scenario: Option<jig_conformance::scenarios::Scenario>,
    story: Option<super::stories::Story>,
) -> r::Policy {
    let (config, _) = super::config::root(seed, scenario, story);
    let rules = &config.core.defaults.rules;
    let project = &config.core.seed_projects[0].policy.policy;
    r::Policy {
        deployment: jig_core_world::observations::scope(&rules.ceiling),
        projects: BTreeMap::from([(1, jig_core_world::observations::scope(&project.ceiling))]),
        projections: BTreeMap::from([(
            1,
            project
                .projections
                .iter()
                .map(|g| r::Grant { connector: g.connector, kind: g.kind, pattern: pattern(&g.pattern) })
                .collect(),
        )]),
        people: BTreeMap::from([((1, 1), jig_core_world::observations::scope(&project.roles[0].authority))]),
        effect_accepters: BTreeSet::from([(1, 1)]),
        requirements: Vec::new(),
        implications: BTreeSet::new(),
        kinds: BTreeMap::from(
            [
                (2, r::Recovery::Unrecoverable),
                (3, r::Recovery::Conditional),
                (4, r::Recovery::Conditional),
                (5, r::Recovery::Unrecoverable),
                (6, r::Recovery::Idempotent),
                (7, r::Recovery::Unrecoverable),
                (8, r::Recovery::Unrecoverable),
                (9, r::Recovery::Unrecoverable),
                (10, r::Recovery::Idempotent),
                (11, r::Recovery::Unrecoverable),
                (12, r::Recovery::Idempotent),
                (13, r::Recovery::Idempotent),
                (14, r::Recovery::Idempotent),
                (15, r::Recovery::Idempotent),
            ]
            .map(|(kind, recovery)| ((0, kind), r::Kind { recovery, price: 0 })),
        ),
        owned: vec![(0, r::Pattern { base: repository().path, terminal: Vec::new(), open: true })],
        participating: BTreeSet::from([repository()]),
        mechanics: BTreeSet::new(),
        delivery_bound: 60_000_000_000,
        uncertainty_bound: 60_000_000_000,
        story_steps: 1_000,
    }
}
#[must_use]
pub fn pattern(p: &jig_domain_authority::Pattern) -> r::Pattern {
    let (terminal, open) = match &p.last {
        jig_domain_authority::Last::Exact(x) => (x.to_vec(), false),
        jig_domain_authority::Last::Open(x) => (x.to_vec(), true),
    };
    r::Pattern { base: p.segments.iter().map(|x| x.to_vec()).collect(), terminal, open }
}
pub type Record = root::Record;
pub type Store = temper_engine_domain_world::store::Store;

#[expect(clippy::too_many_lines, reason = "one outside translation covers the durable row vocabulary")]
#[must_use]
pub fn snapshot(store: &Store) -> r::Snapshot {
    let mut snapshot = r::Snapshot::default();
    let mut decisions = Vec::new();
    for record in store.rows.values() {
        match record {
            root::Record::Core(core::Record::Tasks(tasks::Stored::Live(row) | tasks::Stored::Ended(row))) => {
                snapshot.tasks.insert(row.number, jig_core_world::observations::task(row));
                snapshot.receipts.insert(r::Receipt::Task(row.number));
                if let tasks::Escalation::Waiting { holder: tasks::EscalationHolder::Task(holder), entry, .. } =
                    row.escalation
                {
                    snapshot.receipts.insert(r::Receipt::Word(holder, entry));
                }
                if matches!(row.phase, tasks::Phase::Ended(_)) {
                    snapshot.receipts.insert(r::Receipt::Ended(row.number));
                }
                for word in &row.inbox {
                    snapshot.receipts.insert(r::Receipt::Word(row.number, word.number));
                }
            }
            root::Record::Core(core::Record::Tasks(tasks::Stored::Writer(row))) => {
                let (task, attempt) = match row.writer {
                    tasks::Writer::Run { task, attempt } => (task, attempt),
                    tasks::Writer::Effect { entry } => (0, entry),
                };
                snapshot.writers.push((
                    r::Name {
                        connector: row.resource.connector,
                        path: row.resource.path.iter().map(|x| x.to_vec()).collect(),
                    },
                    task,
                    attempt,
                ));
            }
            root::Record::Core(core::Record::Tasks(tasks::Stored::Ledger(row))) => {
                snapshot.balances.push((
                    row.numbers.budget,
                    row.numbers.spent,
                    row.numbers.spent_below,
                    row.numbers.reserved,
                ));
                if let tasks::Funder::Pool { person, .. } = row.funder {
                    *snapshot.person_spend.entry(person).or_default() += row.numbers.spent;
                }
            }
            root::Record::Core(core::Record::People(people::Stored::SignIn { number, .. })) => {
                snapshot.receipts.insert(r::Receipt::SignIn(*number));
            }
            root::Record::Core(core::Record::People(people::Stored::Answer { key, .. })) => {
                snapshot.receipts.insert(r::Receipt::Party(key.person, key.key));
            }
            root::Record::Core(core::Record::Core(core::CoreRecord::RunProof(row))) => {
                snapshot.receipts.insert(r::Receipt::Claim(row.task, row.attempt));
            }
            root::Record::Core(core::Record::Core(core::CoreRecord::Turn(row))) => {
                snapshot.receipts.insert(r::Receipt::Turn(row.task, row.attempt, row.turn));
            }
            root::Record::Core(core::Record::Core(core::CoreRecord::Terminal(row))) => {
                snapshot.receipts.insert(r::Receipt::Terminal(row.task, row.attempt));
            }
            root::Record::Core(core::Record::Core(core::CoreRecord::Call(row))) => {
                snapshot.receipts.insert(r::Receipt::Call(
                    row.key.task,
                    row.key.attempt,
                    row.key.completion,
                    row.key.position,
                ));
                if let core::CallPart::Delegated(members) = &row.part {
                    snapshot.batches.push(members.iter().map(|made| made.task).collect());
                }
            }
            root::Record::Core(core::Record::Core(core::CoreRecord::ProposalDecision(row)))
                if row.choice == people::ProposalChoice::Accepted =>
            {
                decisions.push(*row);
            }
            root::Record::Forge { row } => {
                let forge::Stored::Entry { entry: row, held: false } = row.as_ref() else { continue };
                let key = key(store, row);
                let (kind, resources, condition, target, state) = parts(&row.effect.write);
                if let Some(attempt) = row.attempt {
                    snapshot.receipts.insert(r::Receipt::Outbox(
                        0,
                        key,
                        u32::try_from(attempt.sent.as_nanos() / 1_000_000).expect("bounded attempt timestamp"),
                    ));
                    snapshot.attempts.insert(
                        (0, key),
                        (
                            u32::try_from(attempt.sent.as_nanos() / 1_000_000).expect("bounded attempt timestamp"),
                            attempt.expires.as_nanos(),
                        ),
                    );
                }
                snapshot.decisions.push(r::Decision {
                    projection: key.attempt == 0 && store.rows.values().any(|record| matches!(record,root::Record::Core(core::Record::Tasks(tasks::Stored::Live(task)|tasks::Stored::Ended(task))) if task.number==row.task && task.tracked.is_some())),
                    connector: 0,
                    key,
                    kind,
                    resources,
                    condition,
                    target,
                    judged_state: state,
                    accepted_by: None,
                });
            }
            root::Record::Core(_) => {}
        }
    }
    for accepted in decisions {
        if let Some((first, last)) = accepted.created {
            for task in first..=last {
                snapshot
                    .accepted_tasks
                    .insert(task, r::Acceptance { proposal: accepted.proposal, by: r::Source::Person(accepted.by) });
            }
        }
        for decision in &mut snapshot.decisions {
            if let people::Proposer::Task(task) = accepted.proposer
                && decision.key.task == task
            {
                decision.accepted_by = Some(r::Source::Person(accepted.by));
                snapshot.acceptances.insert(
                    (0, decision.key),
                    r::Acceptance { proposal: accepted.proposal, by: r::Source::Person(accepted.by) },
                );
            }
        }
    }
    snapshot
}

/// Entry identity and projection ownership retained from actual durable rows.
#[derive(Clone, Copy)]
pub struct EffectEvidence {
    pub kind: u16,
    pub projection: bool,
}

/// Follow each durable entry to its named answer or connector erasure. A later
/// hold cannot relabel a key already observed as settled.
#[expect(clippy::wildcard_enum_match_arm, reason = "only durable named effect calls supply a connector outcome")]
pub fn resolved(
    store: &Store,
    snapshot: &mut r::Snapshot,
    seen: &mut BTreeMap<SystemKey, EffectEvidence>,
    held: &mut BTreeMap<SystemKey, u16>,
    settled: &mut BTreeSet<SystemKey>,
) {
    let mut live = BTreeSet::new();
    for record in store.rows.values() {
        if let root::Record::Forge { row } = record
            && let forge::Stored::Entry { entry, held: false } = row.as_ref()
        {
            let key = key(store, entry);
            live.insert(key);
            let evidence = EffectEvidence {
                kind: parts(&entry.effect.write).0,
                projection: store.rows.values().any(|record| {
                    matches!(record,
                    root::Record::Forge { row } if matches!(row.as_ref(), forge::Stored::Issue(issue)
                        if issue.goal == key.task && issue.pending == Some(key.purpose)))
                }),
            };
            seen.entry(key)
                .and_modify(|old| {
                    assert_eq!(old.kind, evidence.kind, "durable entry changed kind");
                    old.projection |= evidence.projection;
                })
                .or_insert(evidence);
        }
    }
    for (&key, evidence) in seen.iter() {
        if held.contains_key(&key) || settled.contains(&key) {
            continue;
        }
        let answer = store.rows.values().find_map(|record| match record {
            root::Record::Core(core::Record::Core(core::CoreRecord::Call(call)))
                if call.key.task == key.task
                    && call.key.attempt == key.attempt
                    && call.key.completion == key.completion
                    && call.key.position == key.position =>
            {
                match call.part {
                    core::CallPart::Effect { entry, outcome, .. } if entry == key.purpose => Some(outcome),
                    _ => None,
                }
            }
            _ => None,
        });
        match answer {
            Some(Some(core::connector::OutboxOutcome::Held { .. })) => {
                held.insert(key, evidence.kind);
            }
            // A write past its lifetime found not made is settled too (D14).
            Some(Some(
                core::connector::OutboxOutcome::Made
                | core::connector::OutboxOutcome::NotMade
                | core::connector::OutboxOutcome::Failed
                | core::connector::OutboxOutcome::Withdrawn,
            )) => {
                settled.insert(key);
            }
            Some(None | Some(core::connector::OutboxOutcome::Uncertain)) => {}
            None if live.contains(&key) => {}
            None => {
                let projection_failed = evidence.projection
                    && store.rows.values().any(|record| {
                        matches!(record,
                    root::Record::Forge { row } if matches!(row.as_ref(), forge::Stored::Issue(issue)
                        if issue.goal == key.task && issue.failed))
                    });
                if projection_failed {
                    held.insert(key, evidence.kind);
                } else {
                    // A non-named connector entry has reached its terminal
                    // before its root-owned projection/procedure erases it.
                    settled.insert(key);
                }
            }
        }
    }
    snapshot.held_effects.extend(held.keys().map(|key| (0, *key)));
    snapshot.settled_effects.extend(settled.iter().map(|key| (0, *key)));
}

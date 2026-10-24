//! Store adapters for the core-selected restart script (jig's domain/root.md, 8).
use crate::archive::{
    Waiting, input_failed, input_loaded, note_failed, note_loaded, transcript_failed, transcript_loaded, waiting,
};
use crate::boundary::{Request, Work};
use crate::domain::{Domain, close, emit};
use crate::limits::{Limits, core_limits, environment_core, route_decision};
use crate::route::{escalation, route_core_requests, route_into};
use crate::{Decision, Delivery, Key, Range, Record, loads};
use alloc::boxed::Box;
use jig_domain_people as people;
use jig_domain_tasks as tasks;
use skein_lib::{Env, Queue, Token};
use temper_engine_domain_forge as forge;

/// Store transport terminals are read-only; cold rows reserve their full restore route.
pub(crate) fn load_decision(domain: &mut Domain, limits: &Limits) -> Option<Decision> {
    if domain.startup_range.is_some() {
        route_decision(domain, limits)
    } else {
        Decision::reserve_room(&mut domain.journal, &limits.journal, skein_lib::JournalRoom { writes: 0, held: 0 })
    }
}

pub(crate) fn accept_load(domain: &mut Domain, env: &Env<Limits>, decision: Option<Decision>) {
    if let Some(decision) = decision {
        crate::decision::accept_routed(
            &mut domain.journal,
            &mut domain.core.counters,
            &env.limits.journal,
            decision,
            false,
        )
        .expect("read-only store terminal was admitted before changing its owner");
    }
}

pub(crate) fn request_load(
    domain: &mut Domain,
    waiter: Token,
    range: Range,
    after: Option<Key>,
    out: &mut Queue<Request>,
) {
    let mut load_out = Queue::with_capacity(1);
    let most = match &range {
        Range::Core(
            crate::CoreRange::Deployment
            | crate::CoreRange::TaskResult { .. }
            | crate::CoreRange::EndedRead { .. }
            | crate::CoreRange::EscalationDecision { .. }
            | crate::CoreRange::ProposalDecision { .. }
            | crate::CoreRange::TaskProposal { .. }
            | crate::CoreRange::Notes(jig_domain_notes::Range::Entry { .. }),
        ) => 1,
        Range::Core(
            crate::CoreRange::Calls
            | crate::CoreRange::Tasks
            | crate::CoreRange::Settings
            | crate::CoreRange::Accounts
            | crate::CoreRange::People
            | crate::CoreRange::RunProofs
            | crate::CoreRange::Turns { .. }
            | crate::CoreRange::TaskTranscript { .. },
        )
        | Range::Census(_)
        | Range::Forge => domain.limits.loads.rows,
        Range::Core(crate::CoreRange::Notes(jig_domain_notes::Range::Lines { .. })) => {
            domain.limits.loads.rows.min(domain.limits.notes.load_rows)
        }
    };
    if loads::begin(&mut domain.loads, waiter, range.clone(), after, most, &mut load_out).is_none() {
        match range {
            Range::Core(crate::CoreRange::TaskTranscript { .. }) => {
                transcript_failed(domain, waiter);
                return;
            }
            Range::Core(
                crate::CoreRange::ProposalDecision { .. }
                | crate::CoreRange::TaskProposal { .. }
                | crate::CoreRange::EscalationDecision { .. }
                | crate::CoreRange::Calls
                | crate::CoreRange::Deployment
                | crate::CoreRange::Tasks
                | crate::CoreRange::RunProofs
                | crate::CoreRange::Settings
                | crate::CoreRange::Accounts
                | crate::CoreRange::People
                | crate::CoreRange::TaskResult { .. }
                | crate::CoreRange::EndedRead { .. }
                | crate::CoreRange::Turns { .. },
            )
            | Range::Census(_)
            | Range::Forge => {}
            Range::Core(crate::CoreRange::Notes(_)) => {
                note_failed(domain, waiter);
                return;
            }
        }
        match range {
            Range::Core(crate::CoreRange::ProposalDecision { .. } | crate::CoreRange::TaskProposal { .. }) => {
                domain.work.push(Work::ProposalFailed { waiter });
                return;
            }
            Range::Core(crate::CoreRange::EscalationDecision { .. }) => {
                domain.work.push(Work::EscalationFailed { waiter });
                return;
            }
            Range::Core(
                crate::CoreRange::Calls
                | crate::CoreRange::Deployment
                | crate::CoreRange::Tasks
                | crate::CoreRange::Settings
                | crate::CoreRange::Accounts
                | crate::CoreRange::People
                | crate::CoreRange::RunProofs
                | crate::CoreRange::Turns { .. }
                | crate::CoreRange::TaskTranscript { .. }
                | crate::CoreRange::TaskResult { .. }
                | crate::CoreRange::EndedRead { .. },
            )
            | Range::Census(_)
            | Range::Forge => {
                unreachable!("startup/result load room reserved")
            }
            Range::Core(crate::CoreRange::Notes(_)) => unreachable!("notes load failure handled above"),
        }
    }
    match load_out.pop().expect("load issued") {
        loads::Request::Load { owner, range, after, most, bytes } => {
            out.push(Request::Store(crate::StoreRequest::Load { owner, range, after, most, bytes }));
        }
        loads::Request::Loaded { .. } | loads::Request::Unloaded { .. } => unreachable!("begin only issues IO"),
    }
}

pub(crate) fn load_outputs(
    domain: &mut Domain,
    env: &Env<Limits>,
    load_out: &mut Queue<loads::Request>,
    mut texts: Option<crate::TranscriptTexts>,
    decision: &mut Option<Decision>,
    out: &mut Queue<Request>,
) {
    for _ in 0..load_out.len() {
        match load_out.pop().expect("load terminal count") {
            loads::Request::Loaded { waiter, rows, next, cut } => {
                if waiter == Token::new(u64::MAX) {
                    if cut.is_some() {
                        let _refused = domain.core.restart_refuse();
                        out.push(Request::Stop);
                    } else {
                        startup_page(domain, env, rows, next, decision.take().expect("admitted restore page"), out);
                    }
                    return;
                }
                match waiting(domain, waiter) {
                    Some(Waiting::Notes) => note_loaded(domain, waiter, rows, next, cut),
                    Some(Waiting::Input) if cut.is_none() => input_loaded(domain, waiter, rows, next, out),
                    Some(Waiting::Input) => input_failed(domain, waiter),
                    Some(Waiting::Transcript) => transcript_loaded(domain, waiter, rows, texts.take(), next, cut, out),
                    Some(Waiting::Ended) if cut.is_none() && next.is_none() => {
                        domain.work.push(Work::EndedLoaded { waiter, rows });
                    }
                    Some(Waiting::Ended) => domain.work.push(Work::EndedFailed { waiter }),
                    Some(Waiting::TaskProposal) => domain.work.push(Work::TaskProposalLoaded {
                        waiter,
                        rows: if cut.is_none() && next.is_none() { Some(rows) } else { None },
                    }),
                    Some(Waiting::Proposal) if cut.is_none() && next.is_none() => {
                        domain.work.push(Work::ProposalLoaded { waiter, rows });
                    }
                    Some(Waiting::Proposal) => domain.work.push(Work::ProposalFailed { waiter }),
                    Some(Waiting::Escalation) if cut.is_none() => {
                        domain.work.push(Work::EscalationLoaded { waiter, rows });
                    }
                    Some(Waiting::Escalation) => domain.work.push(Work::EscalationFailed { waiter }),
                    None => {}
                }
            }
            loads::Request::Unloaded { waiter, .. } => {
                if waiter == Token::new(u64::MAX) {
                    let _refused = domain.core.restart_refuse();
                    out.push(Request::Stop);
                    return;
                }
                match waiting(domain, waiter) {
                    Some(Waiting::Notes) => note_failed(domain, waiter),
                    Some(Waiting::Input) => input_failed(domain, waiter),
                    Some(Waiting::Transcript) => transcript_failed(domain, waiter),
                    Some(Waiting::Ended) => domain.work.push(Work::EndedFailed { waiter }),
                    Some(Waiting::Proposal) => domain.work.push(Work::ProposalFailed { waiter }),
                    Some(Waiting::TaskProposal) => domain.work.push(Work::TaskProposalLoaded { waiter, rows: None }),
                    Some(Waiting::Escalation) => domain.work.push(Work::EscalationFailed { waiter }),
                    None => {}
                }
            }
            loads::Request::Load { .. } => unreachable!("terminal methods never issue IO"),
        }
    }
}

pub(crate) fn startup_page(
    domain: &mut Domain,
    env: &Env<Limits>,
    rows: Box<[Record]>,
    next: Option<Key>,
    mut decision: Decision,
    out: &mut Queue<Request>,
) {
    let Some(range) = domain.startup_range.clone() else { return };
    if let Range::Census(_) = range {
        census_page(domain, env, rows, next, decision, out);
        return;
    }
    for row in rows {
        restore_page_row(domain, env, row);
    }
    route_into(domain, env, &mut decision);
    if domain.core.restart_failure().is_some() {
        out.push(Request::Stop);
        return;
    }
    if range == Range::Core(crate::CoreRange::Deployment) {
        domain.core_header_loaded = true;
        close(domain, env, decision, out);
        let following = match next {
            Some(after) => (range, Some(after)),
            None => (Range::Core(crate::CoreRange::Settings), None),
        };
        domain.startup_range = Some(following.0.clone());
        request_load(domain, Token::new(u64::MAX), following.0, following.1, out);
        return;
    }
    if let Some(after) = next {
        emit(&mut decision, &env.limits, Delivery::Load { waiter: Token::new(u64::MAX), range, after: Some(after) });
        close(domain, env, decision, out);
        return;
    }
    if range == Range::Core(crate::CoreRange::Settings) && !domain.core.settings_restored(&environment_core(env)) {
        out.push(Request::Stop);
        return;
    }
    let following = match range {
        Range::Core(crate::CoreRange::Deployment) => Some(Range::Core(crate::CoreRange::Settings)),
        Range::Core(crate::CoreRange::Settings) => Some(Range::Core(crate::CoreRange::Accounts)),
        Range::Core(crate::CoreRange::Accounts) => Some(Range::Core(crate::CoreRange::People)),
        Range::Core(crate::CoreRange::People) => Some(Range::Core(crate::CoreRange::Tasks)),
        Range::Core(crate::CoreRange::Tasks) => Some(Range::Core(crate::CoreRange::RunProofs)),
        Range::Forge | Range::Census(_) | Range::Core(crate::CoreRange::Calls) => None,
        Range::Core(crate::CoreRange::RunProofs) => Some(Range::Core(crate::CoreRange::Calls)),
        Range::Core(
            crate::CoreRange::EscalationDecision { .. }
            | crate::CoreRange::ProposalDecision { .. }
            | crate::CoreRange::TaskProposal { .. }
            | crate::CoreRange::Turns { .. }
            | crate::CoreRange::TaskTranscript { .. }
            | crate::CoreRange::TaskResult { .. }
            | crate::CoreRange::EndedRead { .. },
        ) => {
            unreachable!("startup range")
        }
        Range::Core(crate::CoreRange::Notes(_)) => unreachable!("notes load is outside startup"),
    };
    if range == Range::Core(crate::CoreRange::Tasks) {
        // People's restoration terminal; at a first start the core seeds the
        // configured projects and owners (jig's `protocol/configuration.md`, 3).
        domain.work.push(Work::People(people::Event::Restored));
        route_into(domain, env, &mut decision);
        if domain.core.restart_failure().is_some() {
            out.push(Request::Stop);
            return;
        }
    }
    if let Some(range) = following {
        domain.startup_range = Some(range.clone());
        emit(&mut decision, &env.limits, Delivery::Load { waiter: Token::new(u64::MAX), range, after: None });
        close(domain, env, decision, out);
        return;
    }
    domain.startup_range = None;
    if range != Range::Forge {
        domain.work.push(Work::Core(jig_domain_core::Event::SettingsCommit {
            // Leave the rest of the root route's room for restart callbacks.
            room: jig_domain_core::settings::writes_worst_case(&core_limits(&env.limits))
                .expect("validated settings write room"),
        }));
    }
    let request = if range == Range::Forge {
        // The connector confirms that its restored rows have been reconciled.
        domain.work.push(Work::Forge(forge::Event::Restored { clock: skein_forge_client::RecoveryClock::Wall }));
        jig_domain_core::RestartRequest::Idle
    } else {
        domain.core.restart_done(jig_domain_core::RestartStep::LoadCore)
    };
    domain.work.push(Work::Restart(request));
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
}

/// Perform only the step requested by the core; page cursors are transport
/// state, while restart order and admission belong to jig-core.
pub(crate) fn restart_request(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    request: jig_domain_core::RestartRequest,
) {
    match request {
        jig_domain_core::RestartRequest::Idle => {}
        jig_domain_core::RestartRequest::Settings { .. } | jig_domain_core::RestartRequest::Refused { .. } => {
            domain.stop_pending = true;
        }
        jig_domain_core::RestartRequest::Step(step) => match step {
            jig_domain_core::RestartStep::LoadCore => {
                domain.startup_range = Some(Range::Core(crate::CoreRange::Deployment));
                emit(
                    decision,
                    &env.limits,
                    Delivery::Load {
                        waiter: Token::new(u64::MAX),
                        range: Range::Core(crate::CoreRange::Deployment),
                        after: None,
                    },
                );
            }
            jig_domain_core::RestartStep::LoadInbox => {
                // The accounts' records came with the core's ranges: want each
                // configured account once they are restored (jig's
                // `examples/ops/domain/src/restart.rs`).
                let routed = jig_domain_core::want_accounts(&mut domain.core, &environment_core(env));
                route_core_requests(domain, env, decision, routed);
                let census = domain.core.census(&core_limits(&env.limits));
                let range = Range::Census(census);
                domain.startup_range = Some(range.clone());
                emit(decision, &env.limits, Delivery::Load { waiter: Token::new(u64::MAX), range, after: None });
            }
            jig_domain_core::RestartStep::RestoreConnector { connector } => {
                assert_eq!(connector, domain.config.forge_connector, "configured forge connector");
                domain.startup_range = Some(Range::Forge);
                emit(
                    decision,
                    &env.limits,
                    Delivery::Load { waiter: Token::new(u64::MAX), range: Range::Forge, after: None },
                );
            }
            jig_domain_core::RestartStep::AdoptRuns => {
                domain.work.push(Work::Tasks(tasks::Event::Restored));
                // Finish task restoration before draining every emitted claim.
                domain.work.push(Work::AdoptRestored);
            }
            jig_domain_core::RestartStep::ReadAfresh { connector } => {
                assert_eq!(connector, domain.config.forge_connector, "configured forge connector");
                domain.work.push(Work::Forge(forge::Event::ReadAfresh));
            }
            jig_domain_core::RestartStep::SettleOutbox { connector } => {
                assert_eq!(connector, domain.config.forge_connector, "configured forge connector");
                domain.work.push(Work::Forge(forge::Event::SettleOutbox));
            }
            jig_domain_core::RestartStep::Open => {
                let request = domain.core.restart_done(step);
                assert_eq!(request, jig_domain_core::RestartRequest::Idle, "decisions open once");
                domain.work.push(Work::Forge(forge::Event::SendDemoActs));
                for _ in 0..domain.limits.tasks.tasks {
                    let Some(context) = domain.core.take_due() else { break };
                    domain.work.push(Work::Activate(context));
                }
            }
        },
    }
}

pub(crate) fn connector_restart_done(domain: &mut Domain, stage: jig_domain_core::connector::RestartStage) {
    let connector = domain.config.forge_connector;
    let step = match stage {
        jig_domain_core::connector::RestartStage::Restored => {
            jig_domain_core::RestartStep::RestoreConnector { connector }
        }
        jig_domain_core::connector::RestartStage::ReadAfresh => jig_domain_core::RestartStep::ReadAfresh { connector },
        jig_domain_core::connector::RestartStage::Settled => jig_domain_core::RestartStep::SettleOutbox { connector },
    };
    let request = domain.core.restart_done(step);
    domain.work.push(Work::Restart(request));
}

pub(crate) fn connector_restarting(domain: &Domain) -> bool {
    match domain.core.restart_step() {
        Some(
            jig_domain_core::RestartStep::RestoreConnector { .. }
            | jig_domain_core::RestartStep::ReadAfresh { .. }
            | jig_domain_core::RestartStep::SettleOutbox { .. },
        ) => true,
        Some(
            jig_domain_core::RestartStep::LoadCore
            | jig_domain_core::RestartStep::LoadInbox
            | jig_domain_core::RestartStep::AdoptRuns
            | jig_domain_core::RestartStep::Open,
        )
        | None => false,
    }
}

/// A record breaking the accounts child's rules is counted there and dropped
/// (jig's `examples/ops/domain/src/restart.rs`).
fn restore_account(domain: &mut Domain, env: &Env<Limits>, record: jig_domain_accounts::Stored) {
    let limits = core_limits(&env.limits).accounts;
    let mut out = Queue::with_capacity(jig_domain_accounts::max_out(&limits));
    jig_domain_accounts::step(
        &mut domain.core.accounts,
        &Env { now: env.now, wall: env.wall, limits },
        jig_domain_accounts::Event::Restore { record },
        &mut out,
    );
    if !out.is_empty() {
        let _refused = domain.core.restart_refuse();
    }
}

/// Reject unsupported root shapes and identities above durable high-water marks before child
/// restoration. Proof rows consume exact transient live-row correlations and never load archive
/// history into the live map.
pub(crate) fn restore_page_row(domain: &mut Domain, env: &Env<Limits>, row: Record) {
    match row {
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Settings(row))) => {
            if domain.core.restore_core(jig_domain_core::CoreRecord::Settings(row), &core_limits(&env.limits))
                != jig_domain_core::Restored::Live
            {
                let _refused = domain.core.restart_refuse();
            }
        }
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Call(record))) => {
            if domain.core.restore_core(jig_domain_core::CoreRecord::Call(record), &core_limits(&env.limits))
                != jig_domain_core::Restored::Live
            {
                let _refused = domain.core.restart_refuse();
            }
        }
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Deployment(deployment))) => {
            if !domain.forge.bind_deployment(deployment.id, &env.limits.forge) {
                let _refused = domain.core.restart_refuse();
                return;
            }
            match domain
                .core
                .restore_core(jig_domain_core::CoreRecord::Deployment(deployment), &core_limits(&env.limits))
            {
                jig_domain_core::Restored::Deployment { commits } => {
                    domain.core_header_loaded = true;
                    domain.restored_commits = Some(commits);
                }
                jig_domain_core::Restored::Live
                | jig_domain_core::Restored::Archive
                | jig_domain_core::Restored::Rejected => {
                    unreachable!("deployment restore result")
                }
            }
        }
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Projection(row))) => {
            if domain.core.restore_core(jig_domain_core::CoreRecord::Projection(row), &core_limits(&env.limits))
                == jig_domain_core::Restored::Rejected
            {
                let _refused = domain.core.restart_refuse();
            }
        }
        Record::Core(jig_domain_core::Record::People(people::Stored::Policy { project, name, value })) => {
            if !domain.core.restore_policy(&core_limits(&domain.limits), project, name, value) {
                let _refused = domain.core.restart_refuse();
            }
        }
        Record::Core(jig_domain_core::Record::People(record)) => {
            domain.work.push(Work::People(people::Event::Restore { record }));
        }
        Record::Core(jig_domain_core::Record::Accounts(record)) => restore_account(domain, env, record),
        Record::Forge { row } => {
            domain.work.push(Work::Forge(forge::Event::Restore { record: *row }));
        }
        Record::Core(jig_domain_core::Record::Tasks(record)) => {
            let supported = match &record {
                tasks::Stored::Live(task) => escalation::supported(domain, task),
                tasks::Stored::Update(_)
                | tasks::Stored::Result(_)
                | tasks::Stored::EndedByParty(_)
                | tasks::Stored::EndedByProject(_)
                | tasks::Stored::EndedDelegate(_)
                | tasks::Stored::Delegate(_)
                | tasks::Stored::Activity(_)
                | tasks::Stored::Milestone(_)
                | tasks::Stored::PersonProposal(_)
                | tasks::Stored::DecidedProposal(_)
                | tasks::Stored::ClosedLedger(_)
                | tasks::Stored::History(_)
                | tasks::Stored::Ended(_)
                | tasks::Stored::Stub(_)
                | tasks::Stored::Ledger(_)
                | tasks::Stored::Writer(_)
                | tasks::Stored::Pool(_) => true,
            };
            if !supported || !domain.core.restore_task_row(&record) {
                let _refused = domain.core.restart_refuse();
                return;
            }
            domain.work.push(Work::Tasks(tasks::Event::Restore { record }));
        }
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::RunProof(proof))) => {
            if domain.core.restore_core(jig_domain_core::CoreRecord::RunProof(proof), &core_limits(&env.limits))
                != jig_domain_core::Restored::Live
            {
                let _refused = domain.core.restart_refuse();
            }
        }
        Record::Core(jig_domain_core::Record::Notes(_)) => unreachable!("notes are loaded on demand, outside startup"),
        Record::Core(jig_domain_core::Record::Core(
            jig_domain_core::CoreRecord::Turn(_)
            | jig_domain_core::CoreRecord::Terminal(_)
            | jig_domain_core::CoreRecord::EscalationDecision(_)
            | jig_domain_core::CoreRecord::ProposalDecision(_),
        )) => {
            unreachable!("startup excludes archive families")
        }
    }
}

/// Rebuild the derived inbox from the census's ended owners, after people's
/// read positions restored (jig's `protocol/store.md`, 7.2).
fn census_page(
    domain: &mut Domain,
    env: &Env<Limits>,
    rows: Box<[Record]>,
    next: Option<Key>,
    mut decision: Decision,
    out: &mut Queue<Request>,
) {
    for row in rows {
        let restored = match row {
            Record::Core(row) => domain.core.restore_inbox_row(&core_limits(&env.limits), &row),
            Record::Forge { .. } => false,
        };
        if !restored {
            let _refused = domain.core.restart_refuse();
            out.push(Request::Stop);
            return;
        }
    }
    let range = domain.startup_range.clone().expect("census range in flight");
    if let Some(after) = next {
        emit(&mut decision, &env.limits, Delivery::Load { waiter: Token::new(u64::MAX), range, after: Some(after) });
        close(domain, env, decision, out);
        return;
    }
    domain.startup_range = None;
    let request = domain.core.restart_done(jig_domain_core::RestartStep::LoadInbox);
    domain.work.push(Work::Restart(request));
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
}

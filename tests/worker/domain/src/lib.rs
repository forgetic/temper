//! The worker root's version-two runtime world at its typed domain boundary.
//! A scripted engine, checkout and agent drive the worker through deterministic
//! turns, reconnects, merges and retained answers (domain/worker.md, section 9).
//! It owns injected time and seeds; it does not know a wire protocol or store.
//! `next::World` runs and replays those stories.

use skein_lib::Duration;
use temper_worker_domain::{Limits, agent, checkout, host};

pub mod next;

/// The calm worker's limits: room for three runs of one repository each,
/// with the engine's charters, and events and outcomes of a few hundred
/// bytes.
pub const LIMITS: Limits = Limits {
    host: host::Limits {
        accounts: 4,
        slots: 3,
        charter_bytes: 8_192,
        snapshot_bytes: 64,
        transcript_bytes: 0,
        delivery_evidence_bytes: 0,
        turn_bytes: 0,
        outcome_bytes: 4_096,
        detail_bytes: 32,
        held: 2,
        event_bytes: 96,
        run_calls: 2,
        facts: 256,
        told: 16,
        fact_bytes: 32,
        turns: 0,
        turn_queue_bytes: 0,
    },
    checkout: checkout::Limits {
        workspaces: 4,
        repositories: 2,
        name_bytes: 32,
        message_bytes: 512,
        conflicts: 0,
        path_bytes: 0,
        remote_timeout: Duration::from_secs(60),
        local_timeout: Duration::from_secs(10),
        facts: 256,
    },
    agent: agent::Limits {
        accounts: 4,
        repositories: 8,
        name_bytes: 256,
        agents: 3,
        charter_bytes: 8_192,
        snapshot_bytes: 64,
        transcript_bytes: 0,
        turn_bytes: 0,
        conflicts: 0,
        path_bytes: 0,
        event_bytes: 96,
        events: 2,
        calls: 2,
        call_bytes: 512,
        answer_bytes: 128,
        fact_bytes: 32,
        outcome_bytes: 4_096,
        detail_bytes: 32,
        spawn_timeout: Duration::from_secs(1),
        no_progress: Duration::from_secs(10),
        long_span: Duration::from_secs(120),
        wall_time: Duration::from_secs(900),
        grace: Duration::from_secs(5),
        kill_after: Duration::from_secs(2),
        facts: 256,
    },
    grace: Duration::from_secs(60),
    redial: Duration::from_secs(1),
    redial_max: Duration::from_secs(8),
    stalled: 8,
    turn_backoff: Duration::from_secs(1),
};

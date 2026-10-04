//! Bounds derived from the limits of the domains, without depending on them.
use crate::machine::{Endpoint, Machine};
use crate::wire::Term;
use core::mem::size_of;
use skein_lib::{Duration, Queue};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub chunk: u32,
    pub name_bytes: u32,
    pub secret_bytes: u32,
    pub handshake: Duration,
    pub hello: Duration,
    pub ping: Duration,
    pub silence: Duration,
    pub stall: Duration,
}
impl Limits {
    pub const STARTING: Limits = Limits {
        chunk: 65_536,
        name_bytes: 64,
        secret_bytes: 64,
        handshake: Duration::from_secs(10),
        hello: Duration::from_secs(10),
        ping: Duration::from_secs(15),
        silence: Duration::from_secs(45),
        stall: Duration::from_secs(60),
    };
    #[must_use]
    pub fn valid(&self) -> bool {
        self.handshake > Duration::ZERO
            && self.hello > Duration::ZERO
            && self.ping > Duration::ZERO
            && self.silence > Duration::ZERO
            && self.stall > Duration::ZERO
            && self.chunk > 0
            && self.name_bytes <= 64
            && self.secret_bytes <= 64
            && self.silence >= self.ping.saturating_mul(3)
    }
}

/// Every count and payload byte bound is supplied by the owning protocol layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sizes {
    pub charter: u32,
    pub snapshot: u32,
    pub inbound: u32,
    pub call: u32,
    pub answer: u32,
    pub outcome: u32,
    pub fact: u32,
    pub detail: u32,
    pub name_bytes: u32,
    pub worker_name_bytes: u32,
    pub secret_bytes: u32,
    pub token_bytes: u32,
    pub repositories: u32,
    pub slots: u32,
    pub workstreams: u32,
    pub grants: u32,
    pub endpoints: u32,
    pub entries: u32,
    pub inbox: u32,
    pub run_calls: u32,
    pub stalled: u32,
    pub bounces: u32,
    pub accounts: u32,
    pub facts: u32,
    pub agent_outbox: u32,
    /// Frozen bounds; callers must retain these values.
    pub terms: u32,
    pub refuse_bytes: u32,
    pub diagnostic_bytes: u32,
}
impl Sizes {
    /// Small complete configuration, useful as a base for deployment limits.
    pub const STARTING: Sizes = Sizes {
        charter: 65_536,
        snapshot: 65_536,
        inbound: 16_384,
        call: 16_384,
        answer: 65_536,
        outcome: 65_536,
        fact: 4096,
        detail: 4096,
        name_bytes: 64,
        worker_name_bytes: 64,
        secret_bytes: 64,
        token_bytes: 4096,
        repositories: 8,
        slots: 4,
        workstreams: 32,
        grants: 8,
        endpoints: 8,
        entries: 128,
        inbox: 16,
        run_calls: 8,
        stalled: 32,
        bounces: 32,
        accounts: 8,
        facts: 32,
        agent_outbox: 16,
        terms: 32,
        refuse_bytes: 506,
        diagnostic_bytes: 512,
    };
    #[must_use]
    pub fn valid(&self) -> bool {
        self.worker_name_bytes <= 64
            && self.secret_bytes <= 64
            && self.terms == 32
            && self.refuse_bytes == 506
            && self.diagnostic_bytes == 512
    }
}
#[path = "bounds.rs"]
mod bounds;
pub use bounds::largest;

#[must_use]
pub fn frame(kind: u16, sizes: &Sizes) -> Option<u32> {
    largest(kind, sizes)?.checked_add(8)
}

/// Output bytes include headers and the opening/refusal reserve.
#[must_use]
pub fn output_cap(endpoint: Endpoint, sizes: &Sizes) -> Option<u32> {
    let reserve = 264_u32.checked_add(10)?.checked_add(520)?.checked_add(frame(16, sizes)?)?;
    let body = match endpoint {
        Endpoint::Engine => {
            let per = frame(0x181, sizes)?
                .checked_add(sizes.inbox.checked_mul(frame(0x182, sizes)?)?)?
                .checked_add(sizes.run_calls.checked_mul(frame(0x184, sizes)?)?)?
                .checked_add(frame(0x183, sizes)?)?
                .checked_add(frame(0x185, sizes)?)?
                .checked_add(sizes.accounts.checked_mul(frame(0x186, sizes)?)?)?;
            sizes.slots.checked_mul(per)?
        }
        Endpoint::WorkerLink => frame(0x101, sizes)?
            .checked_add(sizes.slots.checked_mul(frame(0x102, sizes)?)?)?
            .checked_add(sizes.stalled.checked_mul(frame(0x103, sizes)?)?)?
            .checked_add(sizes.bounces.checked_mul(frame(0x104, sizes)?)?)?
            .checked_add(
                sizes
                    .slots
                    .checked_mul(sizes.run_calls)?
                    .checked_mul(frame(0x106, sizes)?.checked_add(frame(0x107, sizes)?)?)?,
            )?
            .checked_add(sizes.facts.checked_mul(frame(0x105, sizes)?)?)?,
        Endpoint::WorkerAgent => {
            let mut largest = 0_u32;
            for kind in 0x281..=0x285 {
                largest = largest.max(frame(kind, sizes)?);
            }
            sizes.agent_outbox.checked_mul(largest)?
        }
        Endpoint::Agent => sizes
            .run_calls
            .checked_mul(frame(0x201, sizes)?.max(frame(0x202, sizes)?))?
            .checked_add(frame(0x207, sizes)?)?
            .checked_add(sizes.facts.checked_mul(frame(0x203, sizes)?)?)?
            .checked_add(sizes.accounts.checked_mul(frame(0x208, sizes)?.checked_add(frame(0x209, sizes)?)?)?)?
            .checked_add(frame(0x204, sizes)?)?
            .checked_add(frame(0x205, sizes)?)?
            .checked_add(frame(0x206, sizes)?)?,
    };
    reserve.checked_add(body)
}
/// Terms advertise exactly the kinds this endpoint receives after opening.
#[must_use]
pub fn terms(endpoint: Endpoint, sizes: &Sizes) -> Option<Box<[Term]>> {
    let mut out = skein_lib::List::with_capacity(sizes.terms);
    for kind in crate::kinds::KINDS {
        if endpoint.receives(*kind) && *kind > 16 {
            let term = Term { kind: *kind, largest: largest(*kind, sizes)? };
            out.push(term).expect("v1 has fewer than 32 terms");
        }
    }
    Some(out.into_boxed())
}
/// A peer must advertise each kind we send once, without foreign/duplicate kinds.
#[must_use]
pub fn check_terms(endpoint: Endpoint, peer: &[Term], sizes: &Sizes) -> Option<()> {
    for (index, term) in peer.iter().enumerate() {
        if term.kind <= 16 || !endpoint.sends(term.kind) {
            return None;
        }
        for prior in peer.get(..index)? {
            if prior.kind == term.kind {
                return None;
            }
        }
    }
    for kind in crate::kinds::KINDS {
        if endpoint.sends(*kind) && *kind > 16 {
            let mut found = false;
            for term in peer {
                if term.kind == *kind && term.largest >= largest(*kind, sizes)? {
                    found = true;
                }
            }
            if !found {
                return None;
            }
        }
    }
    Some(())
}
/// Intake, one body, its exact decoded arrays and owned bytes, bounded output, and state.
#[must_use]
pub fn worst_case(endpoint: Endpoint, limits: &Limits, sizes: &Sizes) -> Option<u64> {
    let cap = output_cap(endpoint, sizes)?;
    let mut body = 512_u32;
    for kind in crate::kinds::KINDS {
        body = body.max(largest(*kind, sizes)?);
    }
    let mut decoded = 0_u64;
    for kind in crate::kinds::KINDS {
        decoded = decoded.max(crate::memory::decoded_heap(*kind, sizes)?);
    }
    let state = u64::try_from(size_of::<Machine>()).ok()?;
    state
        .checked_add(u64::from(limits.chunk).checked_add(8)?)?
        .checked_add(u64::from(body))?
        .checked_add(decoded)?
        .checked_add(u64::from(cap))?
        .checked_add(Queue::<Box<[u8]>>::worst_case(output_slots(endpoint, sizes)?)?)
}
use alloc::boxed::Box;

/// Bounded frame records, derived from outstanding domain records plus opening.
#[must_use]
pub fn output_slots(endpoint: Endpoint, sizes: &Sizes) -> Option<u32> {
    let count = match endpoint {
        Endpoint::Engine => sizes
            .slots
            .checked_mul(3_u32.checked_add(sizes.inbox)?.checked_add(sizes.run_calls)?.checked_add(sizes.accounts)?)?,
        Endpoint::WorkerLink => 1_u32
            .checked_add(sizes.slots)?
            .checked_add(sizes.stalled)?
            .checked_add(sizes.bounces)?
            .checked_add(sizes.slots.checked_mul(sizes.run_calls)?.checked_mul(2)?)?
            .checked_add(sizes.facts)?,
        Endpoint::WorkerAgent => sizes.agent_outbox,
        Endpoint::Agent => sizes
            .run_calls
            .checked_mul(2)?
            .checked_add(sizes.facts)?
            .checked_add(sizes.accounts.checked_mul(2)?)?
            .checked_add(4)?,
    };
    count.checked_add(8)
}

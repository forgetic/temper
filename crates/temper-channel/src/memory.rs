//! Exact decoded-wire heap bounds: array records plus their owned fields.
use crate::Sizes;
use crate::wire::{AgentRepository, EndpointDescriptor, Grant, Hosting, Landed, Landing, Repository, Term};
use alloc::boxed::Box;
use core::mem::size_of;
fn heap_access(sizes: &Sizes) -> u64 {
    u64::from(sizes.name_bytes)
}
fn heap_agent_repository(sizes: &Sizes) -> u64 {
    u64::from(sizes.name_bytes)
}
fn heap_ask(sizes: &Sizes) -> u64 {
    u64::from(sizes.detail).max(u64::from(sizes.call))
}
fn heap_endpoint_descriptor(sizes: &Sizes) -> Option<u64> {
    u64::from(sizes.name_bytes).checked_add(u64::from(sizes.name_bytes))?.checked_add(u64::from(sizes.name_bytes))
}
fn heap_finish(sizes: &Sizes) -> u64 {
    u64::from(sizes.outcome).max(u64::from(sizes.snapshot))
}
fn heap_grant(sizes: &Sizes) -> Option<u64> {
    u64::from(sizes.token_bytes).checked_add(u64::from(sizes.token_bytes))
}
fn heap_landing(sizes: &Sizes) -> u64 {
    heap_push_failure(sizes)
}
fn heap_link_answer(sizes: &Sizes) -> Option<u64> {
    Some(
        u64::from(sizes.outcome)
            .checked_add(heap_work(sizes)?)?
            .max(u64::from(sizes.snapshot).checked_add(heap_work(sizes)?)?)
            .max(u64::from(sizes.detail).checked_add(heap_work(sizes)?)?),
    )
}
fn heap_open(sizes: &Sizes) -> Option<u64> {
    u64::from(sizes.worker_name_bytes).checked_add(u64::from(sizes.secret_bytes))
}
fn heap_push(sizes: &Sizes) -> u64 {
    heap_push_failure(sizes)
}
fn heap_push_failure(sizes: &Sizes) -> u64 {
    u64::from(sizes.diagnostic_bytes)
}
fn heap_refuse(sizes: &Sizes) -> u64 {
    u64::from(sizes.refuse_bytes)
}
fn heap_reply(sizes: &Sizes) -> u64 {
    u64::from(sizes.answer).max(heap_push(sizes))
}
fn heap_repository(sizes: &Sizes) -> Option<u64> {
    u64::from(sizes.name_bytes)
        .checked_add(u64::from(sizes.name_bytes))?
        .checked_add(heap_start(sizes))?
        .checked_add(heap_access(sizes))
}
fn heap_start(sizes: &Sizes) -> u64 {
    u64::from(sizes.name_bytes).max(u64::from(sizes.name_bytes)).max(u64::from(sizes.name_bytes))
}
fn heap_work(sizes: &Sizes) -> Option<u64> {
    u64::from(sizes.repositories)
        .checked_mul(u64::try_from(size_of::<Landed>()).ok()?.checked_add(0_u64)?)?
        .checked_add(
            u64::from(sizes.repositories)
                .checked_mul(u64::try_from(size_of::<Landing>()).ok()?.checked_add(heap_landing(sizes))?)?,
        )
}
fn heap_workspace(sizes: &Sizes) -> Option<u64> {
    u64::from(sizes.name_bytes).checked_add(
        u64::from(sizes.repositories)
            .checked_mul(u64::try_from(size_of::<Repository>()).ok()?.checked_add(heap_repository(sizes)?)?)?,
    )
}
pub(crate) fn decoded_heap(kind: u16, sizes: &Sizes) -> Option<u64> {
    Some(match kind {
        1 => heap_open(sizes)?,
        2 | 4 | 260 | 262 | 263 | 387 | 389 | 514 | 516 | 517 | 518 | 520 | 521 | 644 => 0_u64,
        3 => heap_refuse(sizes),
        16 => u64::from(sizes.terms).checked_mul(u64::try_from(size_of::<Term>()).ok()?.checked_add(0_u64)?)?,
        257 => u64::from(sizes.workstreams)
            .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?.checked_add(u64::from(sizes.name_bytes))?)?
            .checked_add(
                u64::from(sizes.slots).checked_mul(u64::try_from(size_of::<Hosting>()).ok()?.checked_add(0_u64)?)?,
            )?,
        258 => heap_link_answer(sizes)?,
        259 => u64::from(sizes.call),
        261 | 515 => u64::from(sizes.fact),
        385 => heap_workspace(sizes)?
            .checked_add(u64::from(sizes.name_bytes))?
            .checked_add(u64::from(sizes.charter))?
            .checked_add(u64::from(sizes.snapshot))?
            .checked_add(
                u64::from(sizes.grants)
                    .checked_mul(u64::try_from(size_of::<Grant>()).ok()?.checked_add(heap_grant(sizes)?)?)?,
            )?,
        386 | 642 => u64::from(sizes.inbound),
        388 => u64::from(sizes.answer),
        390 | 645 => heap_grant(sizes)?,
        513 => heap_ask(sizes),
        519 => heap_finish(sizes),
        641 => u64::from(sizes.charter)
            .checked_add(u64::from(sizes.snapshot))?
            .checked_add(u64::from(sizes.repositories).checked_mul(
                u64::try_from(size_of::<AgentRepository>()).ok()?.checked_add(heap_agent_repository(sizes))?,
            )?)?
            .checked_add(u64::from(sizes.endpoints).checked_mul(
                u64::try_from(size_of::<EndpointDescriptor>()).ok()?.checked_add(heap_endpoint_descriptor(sizes)?)?,
            )?)?
            .checked_add(
                u64::from(sizes.grants)
                    .checked_mul(u64::try_from(size_of::<Grant>()).ok()?.checked_add(heap_grant(sizes)?)?)?,
            )?,
        643 => heap_reply(sizes),
        _ => return None,
    })
}

//! Conversion scratch includes array storage on both typed sides, which
//! does not follow directly from a byte cap (empty names still own boxes).
use alloc::boxed::Box;
use temper_channel::{Sizes, payload as wire, wire as frame};
use temper_legacy_engine_domain::{self as engine, brief, forge, notes, plan};

fn array(count: u32, fixed: usize) -> Option<u64> {
    u64::from(count).checked_mul(u64::try_from(fixed).ok()?)
}
fn pair(count: u32, left: usize, right: usize) -> Option<u64> {
    array(count, left.checked_add(right)?)
}

/// One translation at a time: held source, its owned conversion (including
/// the overlap while arrays move), its encoded input/output, and validation
/// encoding. The borrowed repository/value tables belong to the caller and
/// are counted by the connection owner's retained-state bound.
#[must_use]
pub fn worst_case(sizes: &Sizes) -> Option<u64> {
    if !sizes.valid() {
        return None;
    }
    let entries = sizes.entries;
    let nested = entries.checked_mul(entries)?;
    let charter = pair(entries, size_of::<brief::Section>(), size_of::<wire::Section>())?.checked_add(pair(
        sizes.endpoints,
        size_of::<engine::Model>(),
        size_of::<wire::Model>(),
    )?)?;
    let outcome = pair(entries, size_of::<plan::Step>(), size_of::<wire::Step>())?
        .checked_add(pair(nested, size_of::<plan::Gate>(), size_of::<wire::Gate>())?)?
        .checked_add(array(nested, size_of::<Box<[u8]>>())?.checked_mul(2)?)?
        .checked_add(pair(entries, size_of::<plan::Target>(), size_of::<wire::Target>())?)?
        .checked_add(pair(sizes.repositories, size_of::<plan::Repository>(), size_of::<u32>())?)?;
    let served = pair(entries, size_of::<notes::Entry>(), size_of::<wire::Entry>())?
        .checked_add(pair(nested, size_of::<notes::Reference>(), size_of::<wire::Item>())?)?
        .checked_add(pair(entries, size_of::<forge::api::Summary>(), size_of::<wire::Summary>())?)?
        .checked_add(array(nested, size_of::<Box<[u8]>>())?.checked_mul(2)?)?
        .checked_add(pair(entries, size_of::<forge::api::Comment>(), size_of::<wire::Comment>())?)?
        .checked_add(pair(entries, size_of::<forge::api::Review>(), size_of::<wire::ForgeReview>())?)?
        .checked_add(pair(entries, size_of::<forge::api::Status>(), size_of::<wire::Status>())?)?
        .checked_add(pair(entries, size_of::<forge::api::Remark>(), size_of::<wire::Remark>())?)?
        .checked_add(pair(entries, size_of::<forge::api::PageName>(), size_of::<wire::PageName>())?)?;
    let payload = sizes
        .charter
        .max(sizes.outcome)
        .max(sizes.call)
        .max(sizes.answer)
        .max(sizes.inbound)
        .max(sizes.fact)
        .max(sizes.snapshot);
    let arrays = charter.max(outcome).max(served);
    let conversions = u64::from(payload).checked_mul(5)?.checked_add(arrays.checked_mul(3)?)?;
    let workspace = pair(sizes.repositories, size_of::<engine::Checkout>(), size_of::<frame::Repository>())?
        .checked_add(u64::from(sizes.repositories).checked_mul(u64::from(sizes.name_bytes).checked_mul(4)?)?)?;
    let grants = pair(sizes.grants, size_of::<engine::accounts::Grant>(), size_of::<frame::Grant>())?
        .checked_add(u64::from(sizes.grants).checked_mul(u64::from(sizes.token_bytes).checked_mul(2)?)?)?;
    let hello = pair(sizes.slots, size_of::<engine::Hosted>(), size_of::<frame::Hosting>())?
        .checked_add(array(sizes.workstreams, size_of::<Box<[u8]>>())?.checked_mul(2)?)?
        .checked_add(u64::from(sizes.workstreams).checked_mul(u64::from(sizes.name_bytes))?)?;
    let work = pair(sizes.repositories, size_of::<engine::Landed>(), size_of::<frame::Landed>())?
        .checked_add(array(sizes.repositories, size_of::<frame::Landing>())?)?
        .checked_add(u64::from(sizes.repositories).checked_mul(u64::from(sizes.diagnostic_bytes))?)?;
    let mut largest = 0_u32;
    for kind in [0x101_u16, 0x102, 0x103, 0x104, 0x105, 0x106, 0x107, 0x181, 0x182, 0x183, 0x184, 0x185, 0x186] {
        largest = largest.max(temper_channel::sizes::frame(kind, sizes)?);
    }
    conversions
        .checked_add(workspace)?
        .checked_add(grants)?
        .checked_add(hello)?
        .checked_add(work)?
        .checked_add(u64::from(largest).checked_mul(2)?)
}

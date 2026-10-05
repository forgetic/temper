use super::Sizes;
fn largest_provider(_sizes: &Sizes) -> u32 {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    length
}
fn largest_hosting_phase(_sizes: &Sizes) -> u32 {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    length
}
fn largest_cancel_reason(_sizes: &Sizes) -> u32 {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    length
}
fn largest_ask(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32.checked_add(sizes.detail)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32.checked_add(sizes.call)?)?;
    length = length.max(variant);
    Some(length)
}
fn largest_hosting(sizes: &Sizes) -> Option<u32> {
    0_u32.checked_add(8_u32)?.checked_add(8_u32)?.checked_add(largest_hosting_phase(sizes))
}
pub(crate) fn largest_failure(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_preparation(sizes)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_run_failure(sizes))?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_agent_failure(sizes))?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_cancel_reason(sizes))?;
    length = length.max(variant);
    Some(length)
}
pub(crate) fn largest_endpoint_descriptor(sizes: &Sizes) -> Option<u32> {
    0_u32
        .checked_add(4_u32)?
        .checked_add(largest_provider(sizes))?
        .checked_add(4_u32.checked_add(sizes.name_bytes)?)?
        .checked_add(largest_address(sizes)?)?
        .checked_add(2_u32)?
        .checked_add(4_u32.checked_add(sizes.name_bytes)?)?
        .checked_add(4_u32)?
        .checked_add(4_u32.checked_add(sizes.name_bytes)?)?
        .checked_add(1_u32.checked_add(4_u32)?)
}
fn largest_push(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_push_failure(sizes)?)?;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    Some(length)
}
fn largest_grant(sizes: &Sizes) -> Option<u32> {
    0_u32
        .checked_add(4_u32)?
        .checked_add(8_u32)?
        .checked_add(8_u32)?
        .checked_add(4_u32.checked_add(sizes.token_bytes)?)?
        .checked_add(4_u32.checked_add(sizes.token_bytes)?)
}
fn largest_push_reason(_sizes: &Sizes) -> u32 {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    length
}
fn largest_reply(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32.checked_add(sizes.answer)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_push(sizes)?)?;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    Some(length)
}
fn largest_preparation(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32)?;
    variant = variant.checked_add(largest_missing(sizes))?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32)?;
    length = length.max(variant);
    Some(length)
}
fn largest_finish(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32.checked_add(sizes.outcome)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(1_u32.checked_add(4_u32.checked_add(sizes.snapshot)?)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_run_failure(sizes))?;
    length = length.max(variant);
    Some(length)
}
fn largest_missing(_sizes: &Sizes) -> u32 {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    length
}
fn largest_workspace(sizes: &Sizes) -> Option<u32> {
    0_u32
        .checked_add(4_u32.checked_add(sizes.name_bytes)?)?
        .checked_add(4_u32.checked_add(sizes.repositories.checked_mul(largest_repository(sizes)?)?)?)
}
fn largest_link_answer(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_assignment_refusal(sizes)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32.checked_add(sizes.outcome)?)?;
    variant = variant.checked_add(largest_work(sizes)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(1_u32.checked_add(4_u32.checked_add(sizes.snapshot)?)?)?;
    variant = variant.checked_add(largest_work(sizes)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_failure(sizes)?)?;
    variant = variant.checked_add(4_u32.checked_add(sizes.detail)?)?;
    variant = variant.checked_add(largest_work(sizes)?)?;
    length = length.max(variant);
    Some(length)
}
fn largest_address(_sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(16_u32)?;
    length = length.max(variant);
    Some(length)
}
fn largest_start(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(32_u32)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.max(variant);
    Some(length)
}
fn largest_invalid(_sizes: &Sizes) -> u32 {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    length
}
fn largest_term(_sizes: &Sizes) -> Option<u32> {
    0_u32.checked_add(2_u32)?.checked_add(4_u32)
}
fn largest_agent_failure(_sizes: &Sizes) -> u32 {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    length
}
fn largest_access(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.max(variant);
    Some(length)
}
fn largest_landing(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_push_failure(sizes)?)?;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(32_u32)?;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    Some(length)
}
pub(crate) fn largest_assignment_refusal(sizes: &Sizes) -> Option<u32> {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let mut variant = 1_u32;
    variant = variant.checked_add(largest_invalid(sizes))?;
    length = length.max(variant);
    Some(length)
}
pub(crate) fn largest_work(sizes: &Sizes) -> Option<u32> {
    0_u32
        .checked_add(4_u32.checked_add(sizes.repositories.checked_mul(largest_landed(sizes)?)?)?)?
        .checked_add(1_u32.checked_add(4_u32.checked_add(sizes.repositories.checked_mul(largest_landing(sizes)?)?)?)?)
}
fn largest_bounce(_sizes: &Sizes) -> u32 {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    length
}
fn largest_push_failure(sizes: &Sizes) -> Option<u32> {
    0_u32
        .checked_add(1_u32.checked_add(4_u32)?)?
        .checked_add(largest_push_reason(sizes))?
        .checked_add(4_u32.checked_add(sizes.diagnostic_bytes)?)?
        .checked_add(8_u32)
}
fn largest_landed(_sizes: &Sizes) -> Option<u32> {
    0_u32.checked_add(4_u32)?.checked_add(32_u32)
}
fn largest_agent_repository(sizes: &Sizes) -> Option<u32> {
    0_u32.checked_add(4_u32.checked_add(sizes.name_bytes)?)?.checked_add(1_u32)
}
fn largest_repository(sizes: &Sizes) -> Option<u32> {
    0_u32
        .checked_add(4_u32)?
        .checked_add(4_u32.checked_add(sizes.name_bytes)?)?
        .checked_add(4_u32.checked_add(sizes.name_bytes)?)?
        .checked_add(largest_start(sizes)?)?
        .checked_add(largest_access(sizes)?)?
        .checked_add(4_u32)
}
fn largest_run_failure(_sizes: &Sizes) -> u32 {
    let mut length = 1_u32;
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    let variant = 1_u32;
    length = length.max(variant);
    length
}
/// Largest permitted body, checked for u32 overflow.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one exhaustive table mirrors every v1 wire kind")]
pub fn largest(kind: u16, sizes: &Sizes) -> Option<u32> {
    match kind {
        1 => Some(256),
        2 | 17 => Some(2),
        3 => Some(512),
        4 => Some(0),
        16 => {
            let mut length = 0_u32;
            length = length.checked_add(4_u32.checked_add(sizes.terms.checked_mul(largest_term(sizes)?)?)?)?;
            Some(length)
        }
        257 => {
            let mut length = 0_u32;
            length = length.checked_add(4_u32)?;
            length = length.checked_add(
                4_u32.checked_add(sizes.workstreams.checked_mul(4_u32.checked_add(sizes.name_bytes)?)?)?,
            )?;
            length = length.checked_add(4_u32.checked_add(sizes.slots.checked_mul(largest_hosting(sizes)?)?)?)?;
            Some(length)
        }
        258 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(largest_link_answer(sizes)?)?;
            Some(length)
        }
        259 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(4_u32.checked_add(sizes.call)?)?;
            Some(length)
        }
        260 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(largest_bounce(sizes))?;
            Some(length)
        }
        261 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(4_u32.checked_add(sizes.fact)?)?;
            Some(length)
        }
        262 | 263 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(4_u32)?;
            length = length.checked_add(8_u32)?;
            Some(length)
        }
        385 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(largest_workspace(sizes)?)?;
            length = length.checked_add(1_u32.checked_add(4_u32.checked_add(sizes.name_bytes)?)?)?;
            length = length.checked_add(4_u32.checked_add(sizes.charter)?)?;
            length = length.checked_add(1_u32.checked_add(4_u32.checked_add(sizes.snapshot)?)?)?;
            length = length.checked_add(4_u32.checked_add(sizes.grants.checked_mul(largest_grant(sizes)?)?)?)?;
            Some(length)
        }
        386 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(4_u32.checked_add(sizes.inbound)?)?;
            Some(length)
        }
        387 | 389 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            Some(length)
        }
        388 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(4_u32.checked_add(sizes.answer)?)?;
            Some(length)
        }
        390 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(largest_grant(sizes)?)?;
            Some(length)
        }
        513 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(largest_ask(sizes)?)?;
            Some(length)
        }
        514 | 516 | 518 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            Some(length)
        }
        515 => {
            let mut length = 0_u32;
            length = length.checked_add(4_u32.checked_add(sizes.fact)?)?;
            Some(length)
        }
        517 | 644 => {
            let length = 0_u32;
            Some(length)
        }
        519 => {
            let mut length = 0_u32;
            length = length.checked_add(largest_finish(sizes)?)?;
            Some(length)
        }
        520 | 521 => {
            let mut length = 0_u32;
            length = length.checked_add(4_u32)?;
            length = length.checked_add(8_u32)?;
            Some(length)
        }
        641 => {
            let mut length = 0_u32;
            length = length.checked_add(4_u32.checked_add(sizes.charter)?)?;
            length = length.checked_add(1_u32.checked_add(4_u32.checked_add(sizes.snapshot)?)?)?;
            length = length
                .checked_add(4_u32.checked_add(sizes.repositories.checked_mul(largest_agent_repository(sizes)?)?)?)?;
            length = length
                .checked_add(4_u32.checked_add(sizes.endpoints.checked_mul(largest_endpoint_descriptor(sizes)?)?)?)?;
            length = length.checked_add(4_u32.checked_add(sizes.grants.checked_mul(largest_grant(sizes)?)?)?)?;
            Some(length)
        }
        642 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(4_u32.checked_add(sizes.inbound)?)?;
            Some(length)
        }
        643 => {
            let mut length = 0_u32;
            length = length.checked_add(8_u32)?;
            length = length.checked_add(largest_reply(sizes)?)?;
            Some(length)
        }
        645 => {
            let mut length = 0_u32;
            length = length.checked_add(largest_grant(sizes)?)?;
            Some(length)
        }
        _ => None,
    }
}

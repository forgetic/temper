/// Exponential retry delay capped at thirty seconds.
pub fn retry_delay_ms(attempt: u32) -> u64 {
    250_u64.saturating_mul(1_u64 << attempt.min(7)).min(30_000)
}

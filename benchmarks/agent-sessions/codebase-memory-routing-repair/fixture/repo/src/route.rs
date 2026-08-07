use crate::DeliveryAttempt;

/// Selects a stable ordered-delivery worker. The mixer is intentionally local:
/// callers should reason about affinity inputs rather than hash details.
pub(crate) fn worker_slot(attempt: &DeliveryAttempt<'_>, workers: usize) -> usize {
    assert!(workers > 0, "at least one delivery worker is required");

    let routing_topic = if attempt.attempt == 0 {
        attempt.affinity_topic()
    } else {
        attempt.topic
    };
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in attempt
        .tenant
        .bytes()
        .chain([0xff])
        .chain(routing_topic.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash as usize % workers
}

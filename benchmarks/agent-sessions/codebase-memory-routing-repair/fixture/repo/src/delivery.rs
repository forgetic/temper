use crate::{DeliveryAttempt, route};

/// Production-facing facade for assigning ordered delivery work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryRouter {
    workers: usize,
}

impl DeliveryRouter {
    pub fn new(workers: usize) -> Self {
        assert!(workers > 0, "at least one delivery worker is required");
        Self { workers }
    }

    pub fn worker_for(&self, attempt: &DeliveryAttempt<'_>) -> usize {
        route::worker_slot(attempt, self.workers)
    }
}

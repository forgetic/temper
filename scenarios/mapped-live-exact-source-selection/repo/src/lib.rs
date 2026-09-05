mod model;
mod route;

pub struct DeliveryAttempt<'a> {
    pub tenant: &'a str,
    pub topic: &'a str,
    pub canonical_topic: Option<&'a str>,
    pub attempt: u32,
}

pub fn worker_for(attempt: &DeliveryAttempt<'_>, workers: usize) -> usize {
    route::worker_slot(attempt, workers)
}

/// One attempt to deliver a topic event for a tenant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryAttempt<'a> {
    pub tenant: &'a str,
    pub topic: &'a str,
    /// Original topic used to keep aliases on the same ordered worker stream.
    pub canonical_topic: Option<&'a str>,
    pub attempt: u32,
}

impl<'a> DeliveryAttempt<'a> {
    pub fn new(tenant: &'a str, topic: &'a str) -> Self {
        Self {
            tenant,
            topic,
            canonical_topic: None,
            attempt: 0,
        }
    }

    pub fn aliased_to(mut self, canonical_topic: &'a str) -> Self {
        self.canonical_topic = Some(canonical_topic);
        self
    }

    pub fn retry(mut self, attempt: u32) -> Self {
        self.attempt = attempt;
        self
    }

    pub(crate) fn affinity_topic(&self) -> &'a str {
        self.canonical_topic.unwrap_or(self.topic)
    }
}

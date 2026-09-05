use crate::DeliveryAttempt;

impl DeliveryAttempt<'_> {
    pub(crate) fn affinity_topic(&self) -> &str {
        self.canonical_topic.unwrap_or(self.topic)
    }
}

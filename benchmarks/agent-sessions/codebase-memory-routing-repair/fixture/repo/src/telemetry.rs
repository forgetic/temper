use crate::DeliveryAttempt;

/// Low-cardinality route label used by delivery metrics.
pub fn route_label(attempt: &DeliveryAttempt<'_>) -> String {
    format!("{}/{}", attempt.tenant, attempt.affinity_topic())
}

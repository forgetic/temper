/// Chooses the ordered worker key for an event attempt.
pub fn retry_worker_topic<'a>(
    topic: &'a str,
    canonical_topic: Option<&'a str>,
    attempt: u32,
) -> &'a str {
    if attempt == 0 {
        canonical_topic.unwrap_or(topic)
    } else {
        topic
    }
}

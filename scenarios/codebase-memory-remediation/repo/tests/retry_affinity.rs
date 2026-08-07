use codebase_memory_retry_fixture::retry_worker_topic;

#[test]
fn alias_retries_keep_the_original_ordered_worker() {
    assert_eq!(
        retry_worker_topic("billing-events", Some("invoices"), 2),
        "invoices"
    );
}

#[test]
fn unaliased_retries_keep_their_topic() {
    assert_eq!(
        retry_worker_topic("billing-events", None, 2),
        "billing-events"
    );
}

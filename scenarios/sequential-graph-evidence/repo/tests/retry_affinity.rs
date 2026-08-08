use sequential_graph_evidence_fixture::delivery_worker_topic;

#[test]
fn delivery_worker_topic_preserves_canonical_topic() {
    assert_eq!(
        delivery_worker_topic("billing-events", Some("invoices"), 2),
        "invoices"
    );
}

#[test]
fn unaliased_retries_keep_their_topic() {
    assert_eq!(
        delivery_worker_topic("billing-events", None, 2),
        "billing-events"
    );
}

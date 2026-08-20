use mapped_live_graph_convergence_fixture::{DeliveryAttempt, worker_for};

#[test]
fn alias_retry_stays_on_worker() {
    let initial = DeliveryAttempt {
        tenant: "tenant-a",
        topic: "orders-v2",
        canonical_topic: Some("orders"),
        attempt: 0,
    };
    let retry = DeliveryAttempt {
        attempt: 2,
        ..initial
    };

    assert_eq!(worker_for(&initial, 97), worker_for(&retry, 97));
}

#[test]
fn ordinary_retry_keeps_topic_affinity() {
    let initial = DeliveryAttempt {
        tenant: "tenant-a",
        topic: "orders",
        canonical_topic: None,
        attempt: 0,
    };
    let retry = DeliveryAttempt {
        attempt: 2,
        ..initial
    };

    assert_eq!(worker_for(&initial, 97), worker_for(&retry, 97));
}

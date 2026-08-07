use delivery_router::{DeliveryAttempt, DeliveryRouter};

#[test]
fn alias_retries_stay_on_the_original_ordered_worker() {
    let router = DeliveryRouter::new(17);
    let first = DeliveryAttempt::new("tenant-west", "billing-events").aliased_to("invoices");
    let retry = first.retry(2);

    assert_eq!(router.worker_for(&first), router.worker_for(&retry));
}

#[test]
fn tenant_remains_part_of_alias_affinity() {
    let router = DeliveryRouter::new(17);
    let west = DeliveryAttempt::new("tenant-west", "billing-events").aliased_to("invoices");
    let east = DeliveryAttempt::new("tenant-east", "billing-events").aliased_to("invoices");

    assert_ne!(router.worker_for(&west), router.worker_for(&east));
}

#[test]
fn ordinary_topic_retries_remain_stable() {
    let router = DeliveryRouter::new(17);
    let first = DeliveryAttempt::new("tenant-west", "notifications");

    assert_eq!(
        router.worker_for(&first),
        router.worker_for(&first.retry(4))
    );
}

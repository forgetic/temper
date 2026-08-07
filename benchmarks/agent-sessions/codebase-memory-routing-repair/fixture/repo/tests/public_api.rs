use delivery_router::{DeliveryAttempt, DeliveryRouter, retry_delay_ms, route_label};

#[test]
fn public_facade_keeps_operational_helpers_cohesive() {
    let attempt = DeliveryAttempt::new("acme", "orders");
    assert!(DeliveryRouter::new(4).worker_for(&attempt) < 4);
    assert_eq!(retry_delay_ms(2), 1_000);
    assert_eq!(route_label(&attempt), "acme/orders");
}

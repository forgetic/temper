use skein_lib::Duration;
use temper_engine_protocol::oauth::Event;
use temper_engine_protocol_world::oauth_socket::World;
use temper_fake_llm_protocol::oauth::{Body, Plan};
#[test]
fn seeded_short_transfers_and_raced_close_preserve_save_order_and_single_rotation() {
    for seed in 0..12 {
        let mut world = World::new(seed, true);
        world.queue(Plan {
            status: 200,
            body: Body::Token(temper_oauth::TokenResponse {
                access_token: b"access-5".as_slice().into(),
                refresh_token: Some(b"refresh-5".as_slice().into()),
                expires_in: 30,
            }),
            retry_after: None,
            head_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
        });
        world.hold_closed = true;
        world.refresh(5);
        world.drive();
        assert_eq!(world.records.len(), 1, "seed {seed}");
        assert!(world.table().values().is_empty());
        assert_eq!(world.issuer.posts(), 1);
        let operation = world.records[0].0;
        world.event(Event::Kept { owner: operation });
        world.settle();
        assert_eq!(world.table().values()[0].generation, 5);
        assert_eq!(world.owner.bindings(), 1);
        world.release_closed();
        assert_eq!(world.owner.bindings(), 0);
        world.close();
    }
}

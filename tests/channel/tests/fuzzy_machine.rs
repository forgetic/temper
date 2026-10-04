use skein_lib::Rng;
use temper_channel::{Sizes, codec};
use temper_channel_world::replay;
#[test]
fn fragmented_seed_sweep_and_arbitrary_bytes_do_not_panic() {
    for seed in 0..128 {
        let trace = replay(seed);
        assert!(!trace.is_empty());
    }
    let mut rng = Rng::new(7);
    for _ in 0..4096 {
        let length = usize::try_from(rng.below(256)).expect("short arbitrary input");
        let mut bytes = vec![0; length];
        for byte in &mut bytes {
            *byte = u8::try_from(rng.below(256)).expect("one byte");
        }
        let decoded = codec::decode(&bytes, &Sizes::STARTING);
        if let Some(message) = decoded {
            assert!(codec::encode(&message, &Sizes::STARTING).is_some());
        }
    }
}

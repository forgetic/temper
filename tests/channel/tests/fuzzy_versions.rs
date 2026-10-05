use skein_lib::Rng;
use temper_channel::{Sizes, codec, payload::v2};
use temper_channel_world::replay_v2;
#[test]
fn second_version_seed_sweep_and_bounded_payload_fuzz() {
    for seed in 0..64 {
        assert_eq!(replay_v2(seed), replay_v2(seed));
    }
    let sizes = Sizes { entries: 2, ..Sizes::STARTING };
    let mut rng = Rng::new(71);
    for _ in 0..4096 {
        let length = usize::try_from(rng.below(256)).expect("short arbitrary input");
        let mut bytes = vec![0; length];
        for byte in &mut bytes {
            *byte = u8::try_from(rng.below(256)).expect("one byte");
        }
        if let Some(message) = codec::decode_version(&bytes, &sizes, 2) {
            assert!(codec::encode_version(&message, &sizes, 2).is_some());
        }
        if let Some(call) = v2::decode_call(&bytes, &sizes) {
            assert!(v2::encode_call(&call, &sizes).is_some());
        }
        if let Some(message) = v2::decode_inbound(&bytes, &sizes) {
            assert!(v2::encode_inbound(&message, &sizes).is_some());
        }
        if let Some(charter) = v2::decode_charter(&bytes, &sizes) {
            assert!(v2::encode_charter(&charter, &sizes).is_some());
        }
        if let Some(outcome) = v2::decode_outcome(&bytes, &sizes) {
            assert!(v2::encode_outcome(&outcome, &sizes).is_some());
        }
        if let Some(served) = v2::decode_served(&bytes, &sizes) {
            assert!(v2::encode_served(&served, &sizes).is_some());
        }
        if let Some(transcript) = v2::decode_transcript(&bytes, &sizes) {
            assert!(v2::encode_transcript(&transcript, &sizes).is_some());
        }
    }
}

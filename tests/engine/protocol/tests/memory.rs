use temper_engine_protocol::{payload, worst_case};
use temper_engine_protocol_world::{SIZES, charter, outcome};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

#[test]
fn maximal_charter_conversion_counts_source_nested_arrays_and_output_together() {
    let meter = Meter::new();
    meter.start();
    let mut original = charter();
    let fixed = payload::encode_charter(&original, &SIZES).expect("the fixture fits").len();
    original.instructions = vec![b'i'; SIZES.charter as usize - fixed].into();
    let bytes = payload::encode_charter(&original, &SIZES).expect("exactly full encoded charter");
    assert_eq!(bytes.len(), SIZES.charter as usize);
    let decoded = payload::decode_charter(&bytes, &SIZES).expect("a maximal charter decodes");
    let measured = meter.end();
    meter.check(measured, worst_case(&SIZES).expect("bounded conversion"), 1);
    assert_eq!(decoded, original);
}

#[test]
fn maximal_outcome_conversion_counts_nested_plan_arrays_and_owned_text() {
    let meter = Meter::new();
    meter.start();
    let mut original = outcome();
    let fixed = payload::encode_outcome(&original, &SIZES).expect("the fixture fits").len();
    let temper_engine_domain::Outcome::Plan { text, .. } = &mut original else { panic!("a plan fixture") };
    *text = vec![b'x'; SIZES.outcome as usize - fixed].into();
    let bytes = payload::encode_outcome(&original, &SIZES).expect("exactly full encoded outcome");
    assert_eq!(bytes.len(), SIZES.outcome as usize);
    let decoded = payload::decode_outcome(&bytes, &SIZES).expect("a maximal plan decodes");
    let measured = meter.end();
    meter.check(measured, worst_case(&SIZES).expect("bounded conversion"), 2);
    assert_eq!(decoded, original);
}

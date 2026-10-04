use temper_engine_protocol::{payload, translation_worst_case};
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
    meter.check(measured, translation_worst_case(&SIZES).expect("bounded conversion"), 1);
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
    meter.check(measured, translation_worst_case(&SIZES).expect("bounded conversion"), 2);
    assert_eq!(decoded, original);
}

#[test]
fn occupied_connections_and_maximal_link_conversions_fit_the_owner_bound() {
    use skein_lib::Token;
    use temper_channel::wire;
    use temper_engine_domain as engine;
    use temper_engine_protocol::{connection::Transport, names, worst_case};
    use temper_engine_protocol_world::link::{LIMITS, SIZES, World};
    let meter = Meter::new();
    meter.start();
    let mut world = World::new(Transport::Loopback);
    let (alpha, a) = world.open(b"alpha", b"one");
    let (beta, b) = world.open(b"beta", b"two");
    for (socket, owner, number) in [(alpha, a, 1), (beta, b, 2)] {
        world.peers.get_mut(&socket).expect("peer").blocked = true;
        let mut charter = charter();
        let fixed = payload::encode_charter(&charter, &SIZES).expect("fits").len();
        charter.instructions = vec![b'i'; SIZES.charter as usize - fixed].into();
        world.down(
            engine::Request::Assign {
                channel: owner,
                assignment: engine::Assignment {
                    item: engine::Item { repository: 0, number },
                    attempt: 1,
                    workspace: engine::Workspace {
                        key: vec![b'k'; SIZES.name_bytes as usize].into(),
                        repositories: (0..SIZES.repositories)
                            .map(|repository| engine::Checkout {
                                repository,
                                start: engine::Start::Base { branch: vec![b'b'; SIZES.name_bytes as usize].into() },
                                push: Some(vec![b'p'; SIZES.name_bytes as usize].into()),
                            })
                            .collect(),
                    },
                    save: Some(vec![b's'; SIZES.name_bytes as usize].into()),
                    charter,
                    snapshot: Some(vec![0x99; SIZES.snapshot as usize].into()),
                    grants: Box::new([]),
                },
            },
            &[],
        );
        let mut outcome = outcome();
        let fixed = payload::encode_outcome(&outcome, &SIZES).expect("fits").len();
        let engine::Outcome::Plan { text, .. } = &mut outcome else { panic!("a plan") };
        *text = vec![b'x'; SIZES.outcome as usize - fixed].into();
        world.append(
            socket,
            wire::Message::Answer {
                run: names::run(engine::Item { repository: 0, number }).expect("fits").raw(),
                attempt: names::attempt(engine::Item { repository: 0, number }, 1).expect("fits").raw(),
                answer: wire::LinkAnswer::Ended {
                    outcome: payload::encode_outcome(&outcome, &SIZES).expect("full outcome"),
                    work: wire::Work { landed: Box::new([]), saved: None },
                },
            },
        );
        world.settle();
    }
    let (_, waiting) = world.open(b"alpha", b"one");
    assert_eq!(world.listener.connections(), LIMITS.connections);
    assert_eq!(world.listener.phase(waiting), Some(temper_engine_protocol::connection::Phase::Pending));
    let (_, refused) = world.accept();
    assert_eq!(refused, None::<Token>);
    let measured = meter.end();
    meter.check(measured, worst_case(&LIMITS, &SIZES).expect("bounded owner"), "full listener");
}

//! Domain-tier fixtures use the production record and payload translators.
//! Referee expectations stay outside opaque payloads; socket ownership,
//! authentication and frame bytes are exercised by worker/protocol worlds.
use skein_lib::{Time, Token};
use std::collections::BTreeMap;
use temper_engine_domain::{self as engine, Call, Charter, Inbound, Item, Served};
use temper_engine_domain_world::deployment;
use temper_engine_protocol::{
    payload,
    translate::{self as engine_link, Repository, Value},
};
use temper_worker_domain::{self as worker, host};
use temper_worker_protocol::translate::link as worker_link;

pub const IDENTITY: u32 = 1;
pub const SIZES: temper_channel::Sizes =
    temper_channel::Sizes { slots: 32, workstreams: 64, ..temper_channel::Sizes::STARTING };
pub use temper_engine_domain_world::names::{attempt_of, item, run};
pub type Names = (Token, Token);
pub type Places = BTreeMap<Names, u64>;
#[must_use]
pub fn names(item: Item, attempt: u64) -> Names {
    (run(item), temper_engine_domain_world::names::attempt(item, attempt))
}
#[must_use]
pub fn directory(repository: u32) -> Box<[u8]> {
    let name = deployment::name(repository);
    let at = name.iter().position(|byte| *byte == b'/').map_or(0, |at| at + 1);
    name[at..].into()
}
fn repositories() -> Box<[Repository]> {
    deployment::REPOSITORIES
        .iter()
        .enumerate()
        .map(|(index, remote)| Repository {
            name: directory(u32::try_from(index).expect("small fixture")),
            remote: (*remote).into(),
            identity: IDENTITY,
        })
        .collect()
}
/// Values here are fixture inputs, beside the domain's names. Live retention
/// and receiver skew are exercised by the production connection byte world.
fn values(grants: &[engine::accounts::Grant]) -> Vec<Value> {
    grants
        .iter()
        .map(|grant| Value {
            account: grant.account,
            generation: grant.generation,
            expires: Time::from_nanos(u64::MAX),
            token: b"fixture".as_slice().into(),
            account_id: Box::new([]),
        })
        .collect()
}
#[must_use]
pub fn down(request: engine::Request, grants: &[engine::accounts::Grant]) -> worker::Event {
    let (_, frame) = engine_link::down(request, &repositories(), &values(grants), Time::ZERO, &SIZES)
        .expect("fixture projection supported")
        .expect("worker request");
    worker_link::up(frame, &SIZES).expect("worker projection").event.expect("versioned worker event")
}
#[must_use]
pub fn assignment(assignment: &engine::Assignment) -> (host::Assignment, Vec<u32>) {
    let indexes = assignment.workspace.repositories.iter().map(|checkout| checkout.repository).collect();
    let worker::Event::Assign { assignment } =
        down(engine::Request::Assign { channel: Token::new(0), assignment: assignment.clone() }, &assignment.grants)
    else {
        panic!("Assign projection");
    };
    (assignment, indexes)
}
#[must_use]
pub fn charter_of(bytes: &[u8]) -> Charter {
    payload::decode_charter(bytes, &SIZES).expect("production charter")
}
#[must_use]
pub fn event_comment(bytes: &[u8]) -> Option<u64> {
    match payload::decode_inbound(bytes, &SIZES).expect("production inbound") {
        Inbound::News(engine::forge::News::Comment { id, .. }) => Some(id),
        Inbound::News(engine::forge::News::Reviews { .. } | engine::forge::News::Pull { .. })
        | Inbound::Finished { .. }
        | Inbound::Held { .. }
        | Inbound::Decided { .. } => None,
    }
}
#[must_use]
pub fn up(request: worker::Request, channel: Token) -> engine::Event {
    let frame = worker_link::down(request, &SIZES).expect("fixture projection supported").expect("engine request");
    engine_link::up(
        channel,
        frame,
        u32::try_from(deployment::REPOSITORIES.len()).expect("fixture repositories"),
        &SIZES,
    )
    .expect("production engine projection")
    .expect("versioned event")
}
#[must_use]
pub fn told(channel: Token, told: worker::Told) -> engine::Event {
    let frame = worker_link::told(told, &SIZES).expect("fixture fact bounded");
    engine_link::up(
        channel,
        frame,
        u32::try_from(deployment::REPOSITORIES.len()).expect("fixture repositories"),
        &SIZES,
    )
    .expect("production fact projection")
    .expect("fact event")
}
#[must_use]
pub fn call(call: Call) -> Vec<u8> {
    payload::encode_call(call, &SIZES).expect("production call").into_vec()
}
#[must_use]
pub fn call_of(bytes: &[u8]) -> Option<Call> {
    payload::decode_call(bytes, &SIZES)
}
#[must_use]
pub fn served(served: Served) -> Box<[u8]> {
    payload::encode_served(served, &SIZES).expect("production served")
}
#[must_use]
pub fn undecoded() -> Box<[u8]> {
    served(Served::Unserved(engine::Unserved::Invalid))
}

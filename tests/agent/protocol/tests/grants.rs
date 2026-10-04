use skein_lib::{Duration, Time};
use temper_agent_domain::GrantName;
use temper_agent_protocol::{
    Error,
    grants::{self, Table},
};
use temper_agent_protocol_world::fixture;
use temper_channel::wire;

fn grant(account: u32, generation: u64, valid: u64) -> wire::Grant {
    wire::Grant {
        account,
        generation,
        valid: Duration::from_secs(valid),
        token: b"value".as_slice().into(),
        account_id: b"user".as_slice().into(),
    }
}
fn name(generation: u64) -> GrantName {
    GrantName { account: 0, generation }
}
#[test]
fn scope_header_admission_and_skew_are_checked() {
    let limits = fixture::limits();
    assert!(Table::new(&[0, 0], &limits).is_err());
    assert!(Table::new(&[0, 1, 2], &limits).is_err());
    let mut table = Table::new(&[0], &limits).expect("scope");
    assert_eq!(table.insert(grant(1, 0, 10), Time::ZERO, &limits), Err(Error::Grant));
    for (token, id) in
        [(b"bad\r\n".as_slice(), b"user".as_slice()), (b"value", b"bad\0"), (b"", b""), (b"=bad", b"user")]
    {
        let mut value = grant(0, 0, 10);
        value.token = token.into();
        value.account_id = id.into();
        assert_eq!(table.insert(value, Time::ZERO, &limits), Err(Error::Grant));
    }
    table.insert(grant(0, 0, 10), Time::ZERO, &limits).expect("admitted");
    assert!(table.get(name(0), Time::from_nanos(8_999_999_999), Duration::from_secs(1)).is_some());
    assert!(table.get(name(0), Time::from_nanos(9_000_000_000), Duration::from_secs(1)).is_none());
    assert!(grants::bearer(b"a/b+_-.~=="));
    assert!(!grants::bearer(b"a=b"));
}
#[test]
fn repeated_grants_clip_validity_and_conflicting_values_fail() {
    let limits = fixture::limits();
    let mut table = Table::new(&[0], &limits).expect("scope");
    table.insert(grant(0, 1, 10), Time::ZERO, &limits).expect("first");
    let now = Time::from_nanos(2_000_000_000);
    assert_eq!(
        table.insert(grant(0, 1, 30), now, &limits).expect("repeat").expect("metadata").valid,
        Duration::from_secs(8)
    );
    assert_eq!(
        table.insert(grant(0, 1, 3), now, &limits).expect("shorter").expect("metadata").valid,
        Duration::from_secs(3)
    );
    let mut conflict = grant(0, 1, 20);
    conflict.token = b"another".as_slice().into();
    assert_eq!(table.insert(conflict, now, &limits), Err(Error::Grant));
    assert_eq!(table.next_deadline(), Some(Time::from_nanos(5_000_000_000)));
}
#[test]
fn two_newest_generation_names_fence_evicted_expired_and_rejected_values() {
    let limits = fixture::limits();
    let mut table = Table::new(&[0], &limits).expect("scope");
    for generation in [1, 3, 2] {
        table.insert(grant(0, generation, 10), Time::ZERO, &limits).expect("generation");
    }
    assert!(table.get(name(1), Time::ZERO, Duration::ZERO).is_none());
    assert!(table.get(name(2), Time::ZERO, Duration::ZERO).is_some());
    assert!(table.reject(name(3)));
    assert!(!table.reject(name(3)));
    assert!(table.insert(grant(0, 3, 30), Time::ZERO, &limits).expect("retired repeat").is_none());
    assert!(table.names(Time::ZERO).is_empty());
    table.expire(Time::from_nanos(10_000_000_000));
    assert_eq!(table.next_deadline(), None);
    assert!(
        table.insert(grant(0, 2, 30), Time::from_nanos(10_000_000_000), &limits).expect("expired repeat").is_none()
    );
    assert!(table.insert(grant(0, 1, 30), Time::from_nanos(10_000_000_000), &limits).expect("old repeat").is_none());
    table.insert(grant(0, 4, 30), Time::from_nanos(10_000_000_000), &limits).expect("fresh");
    assert_eq!(table.names(Time::from_nanos(15_000_000_000))[0].valid, Duration::from_secs(25));
}
#[test]
fn time_saturation_clips_metadata_before_the_domain_applies_skew() {
    let limits = fixture::limits();
    let mut table = Table::new(&[0], &limits).expect("scope");
    let mut value = grant(0, 1, 0);
    value.valid = Duration::from_nanos(20);
    let now = Time::from_nanos(u64::MAX - 10);
    assert_eq!(table.insert(value, now, &limits).expect("bounded").expect("metadata").valid, Duration::from_nanos(10));
    assert!(table.get(name(1), Time::from_nanos(u64::MAX - 2), Duration::from_nanos(1)).is_some());
    assert!(table.get(name(1), Time::from_nanos(u64::MAX - 1), Duration::from_nanos(1)).is_none());
}

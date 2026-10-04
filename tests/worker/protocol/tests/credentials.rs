use skein_lib::{Duration, Time};
use temper_worker_protocol::credentials::{Error, Table};
use temper_worker_protocol_world::agent::grant;
#[test]
fn receiver_skew_boundary_and_retired_names_never_revive() {
    let mut table = Table::new(&[1, 2], 2, 32).expect("table");
    table.insert(grant(1, 7, 2), Time::ZERO, Duration::from_secs(2)).expect("short grant");
    assert!(table.grant(1, 7, Time::ZERO).is_none());
    table.expire(Time::ZERO);
    table.insert(grant(1, 7, 10), Time::ZERO, Duration::ZERO).expect("delayed same name");
    assert!(table.grant(1, 7, Time::ZERO).is_none());
    table.insert(grant(2, 0, 10), Time::ZERO, Duration::from_secs(2)).expect("static name");
    assert_eq!(table.grant(2, 0, Time::ZERO).expect("static usable").valid, Duration::from_secs(8));
}
#[test]
fn exactly_two_newest_generations_and_conflicts_are_bounded() {
    let mut table = Table::new(&[1], 1, 32).expect("table");
    for generation in [3, 5, 4, 2] {
        table.insert(grant(1, generation, 10), Time::ZERO, Duration::ZERO).expect("grant");
    }
    assert!(table.grant(1, 5, Time::ZERO).is_some());
    assert!(table.grant(1, 4, Time::ZERO).is_some());
    assert!(table.grant(1, 3, Time::ZERO).is_none());
    let mut conflict = grant(1, 5, 10);
    conflict.token = b"changed".as_slice().into();
    assert_eq!(table.insert(conflict, Time::ZERO, Duration::ZERO), Err(Error::Conflict));
    assert_eq!(table.insert(grant(2, 5, 10), Time::ZERO, Duration::ZERO), Err(Error::Unknown));
}

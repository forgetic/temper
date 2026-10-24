use super::*;
use crate::fixtures::{self, durable as fixture};
use fixture::{POWER_LOSS_SEED, READ, SYNC, USER, WRITE};
use skein_fake_machine::{How, Item, Machine};

#[test]
fn a_torn_log_write_recovers_the_durable_prefix() {
    let bytes = fixture::bytes(false);
    let first = &bytes.frames[0];
    let second = &bytes.frames[1];
    let mut machine = Machine::for_user(USER);
    let root = machine.lay(&[Item::file(b"log", b"")]);
    let file = machine.open(root, b"log", How::Write).expect("log");
    machine.write(file, 0, first).expect("first write");
    machine.sync(file);
    machine.write(file, first.len() as u64, second).expect("second write");
    machine.crash(POWER_LOSS_SEED);
    let root = machine.reopen_root(0);
    let file = machine.open(root, b"log", How::Read).expect("recovered log");
    let bytes = machine.read(file, 0, 4096).expect("recovery read");
    assert!(bytes.len() > first.len() && bytes.len() < first.len() + second.len(), "second write torn");
    let mut store = Store::new();
    let mut events =
        vec![Observed::Bytes { path: fixture::LOG.to_vec(), at: 0, bytes }, Observed::Recovered(fixture::LOG.to_vec())];
    assert_eq!(store.observe(&mut events, &fixtures::codec(), fixtures::store_names(), &fixture::bounds()).len(), 1);
    assert_eq!(store.applied, 1);
    assert_eq!(store.rows.get(&fixture::row().key()), Some(&fixture::row()));
}

#[test]
fn a_repair_snapshot_write_yields_no_commit_frames() {
    let bytes = fixture::bytes(true).snapshot;
    let mut capture = Capture::default();
    capture.names.insert(1, b"generations/1/store/snapshot.tmp".to_vec());
    let write = Op::Write { fd: Fd::new(1), bytes: bytes.clone().into(), from: 0, at: 0 };
    capture.pending(&write);
    assert!(capture.events.is_empty(), "snapshot bytes are not pending commit frames");
    capture.complete(&Complete {
        op: WRITE,
        kind: write,
        result: Ok(Done::Count(u32::try_from(bytes.len()).expect("snapshot length"))),
    });
    capture.complete(&Complete { op: SYNC, kind: Op::Sync { fd: Fd::new(1) }, result: Ok(Done::Nothing) });
    let mut store = Store::new();
    assert!(
        store.observe(&mut capture.events, &fixtures::codec(), fixtures::store_names(), &fixture::bounds()).is_empty()
    );
    assert_eq!(store.applied, 0);
}

#[test]
fn a_repair_snapshot_read_restores_the_durable_rows() {
    let bytes = fixture::bytes(true).snapshot;
    let mut store = Store::new();
    store.recovering();
    let mut capture = Capture::default();
    capture.names.insert(1, b"generations/1/store/snapshot".to_vec());
    capture.complete(&Complete {
        op: READ,
        kind: Op::Read { fd: Fd::new(1), buf: bytes.clone().into(), at: 0 },
        result: Ok(Done::Count(u32::try_from(bytes.len()).expect("snapshot length"))),
    });
    assert_eq!(
        store.observe(&mut capture.events, &fixtures::codec(), fixtures::store_names(), &fixture::bounds()).len(),
        1
    );
    assert_eq!(store.applied, 1);
    assert_eq!(store.recovered, [1].into());
    assert_eq!(store.rows.get(&fixture::row().key()), Some(&fixture::row()));
}

#[test]
fn the_durable_observer_retains_a_settings_copy_before_its_erase() {
    use jig_domain_core::settings as s;
    let revision = s::Revision {
        head: s::Head {
            number: 1,
            against: 0,
            at: skein_lib::Wall::EPOCH,
            by: s::Decider::FirstStart,
            proposal: None,
            moved: false,
        },
        items: Box::new([]),
    };
    let held = core::Record::Core(core::CoreRecord::Settings(s::Record::Revision {
        history: false,
        revision: revision.clone(),
    }));
    let history = core::Record::Core(core::CoreRecord::Settings(s::Record::Revision { history: true, revision }));
    let limits = fixtures::codec();
    let mut store = Store::new();
    store.rows.insert(root::Key::Core(held.key()), root::Record::Core(held.clone()));
    let ops = Box::new([
        skein_kv::Op::Put {
            key: keys::encode(&history.key(), u32::MAX).expect("history key"),
            value: codec::encode(&history, &limits, fixtures::store_names())
                .expect("settings revision fits fixture bounds"),
        },
        skein_kv::Op::Erase { key: keys::encode(&held.key(), u32::MAX).expect("held key") },
    ]);
    let commit = store.commit(1, ops, &limits, fixtures::store_names(), &fixture::bounds());
    assert_eq!(
        commit.writes,
        vec![root::Write::CopySettingsRevision(1), root::Write::Erase(root::Key::Core(held.key()))]
    );
    store.apply(&commit, &limits);
    assert!(!store.rows.contains_key(&root::Key::Core(held.key())));
    assert_eq!(store.rows.get(&root::Key::Core(history.key())), Some(&root::Record::Core(history)));
}

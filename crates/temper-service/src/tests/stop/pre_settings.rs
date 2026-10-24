//! Refused pre-settings starts leave the whole deployment directory unchanged.
#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    reason = "this real-directory step test inventories its bounded scratch tree with std"
)]
use super::fixtures;
use crate::{End, Stop, store};
use alloc::collections::BTreeMap;
use jig_protocol_store::{codec, component_v2, keys};
use skein_io::{
    file,
    file_layer::{self, FileIo},
    kernel::Complete,
};
use skein_lib::{Duration, Env, Queue, Token, Writer};
use skein_shell::{Clock, Config, Kernel, Wait};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

// Use skein-kv itself to frame, write and sync the planted rows, as the restart
// fixtures do. The old component family is retained by jig for this purpose.
fn plant(directory: &Path, ops: Box<[skein_kv::Op]>) {
    let (_, _, profile) = fixtures::store_limits();
    let limits = profile.store;
    let mut kv = skein_kv::Store::new(&limits);
    let mut io = FileIo::new(8, limits.chunk, 32, limits.deadline);
    io.seed_randomness(0);
    let root = skein_shell::open_root(directory).expect("plant root");
    let root_token = io.adopt_root(root).expect("root slot");
    let mut kernel = Kernel::open(Config { operations: 32 }).expect("file kernel");
    let mut below = Queue::with_capacity(128);
    let mut above = Queue::with_capacity(128);
    let mut events = Queue::with_capacity(128);
    let mut submitted = Queue::with_capacity(128);
    let mut completed = Queue::with_capacity(128);
    let clock = Clock::new();
    let mut requests = vec![skein_kv::Request::Open { owner: Token::new(1), root }];
    if !ops.is_empty() {
        requests.push(skein_kv::Request::Commit { owner: Token::new(2), ops });
    }
    requests.push(skein_kv::Request::Close { owner: Token::new(3) });
    for request in requests {
        let now = clock.now();
        let env = Env { now: now.now, wall: now.wall, limits };
        skein_kv::down(&mut kv, &env, request, &mut above, &mut below);
        let until = now.now.checked_add(Duration::from_secs(5)).unwrap();
        loop {
            if let Some(event) = above.pop() {
                let (skein_kv::Event::Opened { .. }
                | skein_kv::Event::Committed { .. }
                | skein_kv::Event::Closed { .. }) = event
                else {
                    panic!("plant refused: {event:?}");
                };
                break;
            }
            assert!(clock.now().now < until, "plant completes within five seconds");
            if io.takes()
                && let Some(request) = below.pop()
            {
                file_layer::down(&mut io, clock.now().now, request, &mut events, &mut submitted);
            }
            kernel.submit(&mut submitted, Wait::No);
            kernel.reap(&mut completed);
            if completed.is_empty() && events.is_empty() && !io.takes() {
                kernel.submit(&mut submitted, Wait::Until(until));
                kernel.reap(&mut completed);
            }
            while let Some(complete) = completed.pop() {
                file_layer::up(&mut io, complete, &mut events, &mut submitted);
            }
            while let Some(event) = events.pop() {
                let now = clock.now();
                skein_kv::up(&mut kv, &Env { now: now.now, wall: now.wall, limits }, event, &mut above, &mut below);
            }
            if below.is_empty() && skein_kv::is_ready(&kv, &limits) {
                let now = clock.now();
                skein_kv::fire(&mut kv, &Env { now: now.now, wall: now.wall, limits }, &mut above, &mut below);
            }
        }
    }
    file_layer::down(
        &mut io,
        clock.now().now,
        file::Request::Close { owner: Token::new(4), file: root_token },
        &mut events,
        &mut submitted,
    );
    kernel.submit(&mut submitted, Wait::Forever);
    kernel.reap(&mut completed);
    while let Some(complete) = completed.pop() {
        file_layer::up(&mut io, complete, &mut events, &mut submitted);
    }
    let Some(file::Event::Closed { .. }) = events.pop() else { panic!("plant root closed") };
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut tree = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).expect("planted directory") {
            let entry = entry.expect("planted entry");
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            let bytes = if kind.is_dir() {
                pending.push(path.clone());
                None
            } else {
                assert!(kind.is_file(), "the planted tree contains only directories and files");
                Some(fs::read(&path).unwrap())
            };
            tree.insert(path.strip_prefix(root).unwrap().to_path_buf(), bytes);
            assert!(tree.len() <= 16, "the walk stays within the planted tree");
        }
    }
    tree
}

#[test]
/// Existing data is untouched; skein-kv open starts a new empty segment before
/// the first version refusal. A second start leaves that whole inventory unchanged.
fn pre_settings_refusals_leave_the_whole_directory_unchanged() {
    let directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").join(format!("pre-settings-{}", std::process::id()));
    fs::create_dir(&directory).expect("new deployment directory");
    for name in ["store", "secrets", "transcripts"] {
        let path = directory.join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let (root, core, profile) = fixtures::store_limits();
    let limits = store::limits(&root, &core, &root.forge, &profile).unwrap();
    let deployment = fixtures::seeds().deployment;
    let old = component_v2::Format::new(
        &component_v2::CEILINGS,
        component_v2::FormatParts {
            component: 2,
            core: 6,
            tasks: 9,
            people: 13,
            notes: 1,
            accounts: 2,
            application: temper_protocol_store::STORE_VERSION,
            deployment,
        },
    );
    let mut bytes = Writer::new(usize::try_from(old.measure()).unwrap());
    old.encode(&mut bytes).unwrap();
    let header = jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Deployment(jig_domain_core::Deployment {
        id: deployment,
        projects: 0,
        tasks: 0,
        people: 0,
        sign_ins: 0,
        messages: 0,
        runs: 0,
        calls: 0,
        connector_rows: 0,
        applications: 0,
        commits: 1,
    }));
    plant(
        &directory.join("store"),
        Box::new([
            skein_kv::Op::Put {
                key: keys::encode_own(&keys::Own::Format, limits.store.key).unwrap(),
                value: bytes.finish(),
            },
            skein_kv::Op::Put {
                key: keys::encode(&header.key(), limits.store.key).unwrap(),
                value: codec::encode(&header, &codec::limits(&core).unwrap(), &[]).unwrap(),
            },
        ]),
    );
    // An old deployment already has its secrets log, even with no secrets rows.
    plant(&directory.join("secrets"), Box::new([]));
    let before = inventory(&directory);
    refuse(&directory, &root, &limits);
    assert_eq!(inventory(&directory), before, "first refusal leaves the whole inventory unchanged");
    refuse(&directory, &root, &limits);
    assert_eq!(inventory(&directory), before, "second refusal leaves the whole inventory unchanged");
    fs::remove_dir_all(directory).unwrap();
}

fn refuse(directory: &Path, root: &temper_engine_domain::Limits, limits: &jig_protocol_store::Limits) {
    let models = jig_protocol_config::core(&fixtures::engine().jig, [7; 16], 7).settings.models;
    let mut store = store::Store::new(root, limits, &models, fixtures::seeds().installation);
    store.adopt(skein_shell::open_root(directory).unwrap(), skein_shell::effective_user()).unwrap();
    let mut service = fixtures::start_with_store(&[(1, 100)], Some(store));
    let mut kernel = Kernel::open(Config { operations: 32 }).expect("file kernel");
    let clock = Clock::new();
    let until = clock.now().now.checked_add(Duration::from_secs(5)).unwrap();
    let mut completed: Queue<Complete> = Queue::with_capacity(128);
    for _ in 0_u32..10_000 {
        let now = clock.now();
        assert!(now.now < until, "refused startup drains within five seconds");
        let engine = service.engine.as_mut().unwrap();
        crate::engine::iterate(engine, now.now, now.wall);
        if engine.end.is_some() {
            engine.store.close();
        }
        kernel.submit(&mut engine.store.submitted, Wait::No);
        kernel.reap(&mut completed);
        if completed.is_empty() && kernel.in_flight() > 0 {
            kernel.submit(&mut engine.store.submitted, Wait::Until(until));
            kernel.reap(&mut completed);
        }
        while let Some(complete) = completed.pop() {
            engine.store.complete(complete);
        }
        if engine.store.closed() {
            break;
        }
    }
    assert_eq!(
        service.end(),
        Some(&End::Stopped(Stop::StoreOpen {
            why: jig_protocol_store::OpenRefusal::OlderBuild { why: jig_protocol_store::Older::Directory }
        }))
    );
    assert!(service.engine.as_ref().unwrap().store.closed(), "file drivers drained");
}

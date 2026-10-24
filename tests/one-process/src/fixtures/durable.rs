//! Real KV writes for the independent durability observer's focused tests.
use jig_protocol_store::{codec, keys};
use skein_io::file;
use skein_lib::{Env, Queue, Time, Token, Wall};
use std::collections::BTreeMap;

pub(crate) const TASK: u64 = 1;
pub(crate) const USER: u32 = 1000;
pub(crate) const POWER_LOSS_SEED: u64 = 6;
const OPEN: Token = Token::new(1);
const COMMIT: Token = Token::new(2);
pub(crate) const WRITE: Token = Token::new(3);
pub(crate) const SYNC: Token = Token::new(4);
pub(crate) const READ: Token = Token::new(5);
pub(crate) const LOG: &[u8] = b"generations/1/store/log-00000000000000000001";

pub(crate) fn row() -> temper_engine_domain::Record {
    temper_engine_domain::Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Terminal(
        jig_domain_core::TerminalRecord {
            task: TASK,
            attempt: 1,
            cumulative: 0,
            end: jig_domain_tasks::End::Finished {
                result: jig_domain_tasks::TaskResult::Report { words: super::ANSWER.into() },
                cancel_delegates: false,
            },
        },
    )))
}
pub(crate) fn bounds() -> temper_protocol_store::Bounds {
    temper_protocol_store::bounds(&temper_service::limits::profile(temper_service::limits::Profile::Small).forge)
        .expect("small forge bounds")
}
pub(crate) struct Bytes {
    pub(crate) frames: Vec<Vec<u8>>,
    pub(crate) snapshot: Vec<u8>,
}
pub(crate) fn bytes(snapshot: bool) -> Bytes {
    let mut limits = jig_protocol_config::profile(jig_protocol_config::Profile::Small).store.store;
    limits.snapshot_after = if snapshot { 1 } else { u64::MAX };
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut files = jig_test_store_files::Files::new();
    let mut store = skein_kv::Store::new(&limits);
    let mut above = Queue::with_capacity(128);
    let mut below = Queue::with_capacity(128);
    let mut names = BTreeMap::new();
    let mut bytes = Bytes { frames: Vec::new(), snapshot: Vec::new() };
    let root = files.root().fd;
    let temper_engine_domain::Record::Core(row) = row() else { unreachable!() };
    let key = keys::encode(&row.key(), limits.key).expect("fixture key");
    let value = codec::encode(&row, &super::codec(), super::store_names()).expect("fixture row");
    let ops = Box::new([skein_kv::Op::Put { key, value }]);
    let mut requests = vec![
        skein_kv::Request::Open { owner: OPEN, root },
        skein_kv::Request::Commit { owner: COMMIT, ops: ops.clone() },
    ];
    if !snapshot {
        requests.push(skein_kv::Request::Commit { owner: COMMIT, ops });
    }
    for request in requests {
        skein_kv::down(&mut store, &env, request, &mut above, &mut below);
        while let Some(request) = below.pop() {
            if let file::Request::Create { owner, name, .. } | file::Request::OpenRead { owner, name, .. } = &request {
                names.insert(*owner, name.clone());
            } else if let file::Request::WriteAt { file, bytes: written, .. } = &request {
                let name = names.get(file).expect("opened fixture file");
                if name.starts_with(b"log-") {
                    bytes.frames.push(written.to_vec());
                } else if name.as_ref() == b"snapshot.tmp" {
                    bytes.snapshot.extend_from_slice(written);
                }
            }
            let arrived = if let file::Request::List { owner, .. } = request {
                jig_protocol_store::Arrived { event: file::Event::Listed { owner, entries: Box::new([]) }, fd: None }
            } else {
                files.answer(request)
            };
            if let file::Event::Opened { owner, file, .. } = &arrived.event {
                let name = names.remove(owner).expect("opening fixture file");
                names.insert(*file, name);
            }
            skein_kv::up(&mut store, &env, arrived.event, &mut above, &mut below);
            if below.is_empty() && skein_kv::is_ready(&store, &limits) {
                skein_kv::fire(&mut store, &env, &mut above, &mut below);
            }
        }
    }
    bytes
}

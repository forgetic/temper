//! Durability observations from real file completions, independent of the service.
mod snapshots;
#[cfg(test)]
mod tests;
use jig_domain_core as core;
use jig_protocol_store::{codec, keys, transcripts};
use skein_io::kernel::{Complete, Done, Fd, Op};
use skein_lib::Reader;
use std::collections::{BTreeMap, VecDeque};
use temper_engine_domain as root;

#[derive(Debug)]
pub struct Commit {
    pub number: u64,
    pub writes: Vec<root::Write>,
}
#[derive(Default)]
pub struct Store {
    pub rows: BTreeMap<root::Key, root::Record>,
    pub(crate) agent_records: Vec<super::agent::Record>,
    pub pending: VecDeque<Commit>,
    pub applied: u64,
    pub recovered: std::collections::BTreeSet<u64>,
    pub(crate) restored: Option<BTreeMap<root::Key, root::Record>>,
    restoring: bool,
    files: BTreeMap<Vec<u8>, Vec<u8>>,
    validated: BTreeMap<Vec<u8>, usize>,
    lengths: BTreeMap<u64, u64>,
}
#[derive(Debug)]
pub enum Observed {
    Bytes { path: Vec<u8>, at: usize, bytes: Vec<u8> },
    Sync(Vec<u8>),
    Recovered(Vec<u8>),
    Snapshot(Vec<u8>),
    Pending(Vec<u8>),
}
#[derive(Default)]
pub struct Capture {
    names: BTreeMap<i32, Vec<u8>>,
    pub events: Vec<Observed>,
}
impl Capture {
    pub fn root(&mut self, fd: Fd) {
        self.names.insert(fd.raw(), Vec::new());
    }
    fn path(&self, fd: Fd) -> Option<&[u8]> {
        self.names.get(&fd.raw()).map(Vec::as_slice)
    }
    pub fn complete(&mut self, complete: &Complete) {
        match (&complete.kind, &complete.result) {
            (Op::Open { root, path, .. }, Ok(Done::Fd(fd))) => {
                let Some(parent) = self.path(*root) else { return };
                let mut name = parent.to_vec();
                if !name.is_empty() {
                    name.push(b'/');
                }
                if path.as_ref() != b"." {
                    name.extend_from_slice(path);
                }
                self.names.insert(fd.raw(), name);
            }
            (Op::Write { fd, bytes, from, at }, Ok(Done::Count(count))) => {
                if let Some(path) = self.path(*fd) {
                    let from = *from as usize;
                    self.events.push(Observed::Bytes {
                        path: path.to_vec(),
                        at: usize::try_from(*at).expect("fixture file offset"),
                        bytes: bytes[from..from + *count as usize].to_vec(),
                    });
                }
            }
            (Op::Read { fd, buf, at }, Ok(Done::Count(count))) => {
                if let Some(path) = self.path(*fd).map(<[u8]>::to_vec) {
                    self.events.push(Observed::Bytes {
                        path: path.clone(),
                        at: usize::try_from(*at).expect("fixture file offset"),
                        bytes: buf[..*count as usize].to_vec(),
                    });
                    if is_log(&path) {
                        self.events.push(Observed::Recovered(path.clone()));
                    } else if path == b"generations/1/store/snapshot" {
                        self.events.push(Observed::Snapshot(path.clone()));
                    }
                }
            }
            (Op::Sync { fd }, Ok(Done::Nothing)) => {
                if let Some(path) = self.path(*fd) {
                    self.events.push(Observed::Sync(path.to_vec()));
                }
            }
            (Op::Close { fd }, _) => {
                self.names.remove(&fd.raw());
            }
            _ => {}
        }
    }
    pub fn pending(&mut self, op: &Op) {
        if let Op::Write { fd, bytes, from: 0, .. } = op
            && self.path(*fd).is_some_and(is_log)
        {
            self.events.push(Observed::Pending(bytes.to_vec()));
        }
    }
}
impl Store {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    pub(crate) fn recovering(&mut self) {
        self.rows.clear();
        self.applied = 0;
        self.lengths.clear();
        self.files.retain(|path, _| !path.starts_with(b"generations/1/store/"));
        self.validated.clear();
        self.pending.clear();
        self.recovered.clear();
        self.restored = None;
        self.restoring = true;
    }
    pub fn observe(
        &mut self,
        events: &mut Vec<Observed>,
        limits: &codec::Limits,
        names: &[core::settings::ModelName],
        connector: &temper_protocol_store::Bounds,
    ) -> Vec<Commit> {
        let mut committed = Vec::new();
        for event in events.drain(..) {
            match event {
                Observed::Bytes { path, at, bytes } => {
                    // The secrets' store keeps a signed-in or handed-in source's
                    // record; the store, its snapshots and its logs hold none
                    // (jig's store.md, section 9).
                    if !path.starts_with(b"generations/1/secrets/") {
                        super::secrets::require(&bytes, "store bytes");
                    }
                    if let Some(validated) = self.validated.get_mut(&path)
                        && at < *validated
                    {
                        *validated = 0;
                    }
                    let file = self.files.entry(path).or_default();
                    file.resize(file.len().max(at + bytes.len()), 0);
                    file[at..at + bytes.len()].copy_from_slice(&bytes);
                }
                Observed::Pending(bytes) => {
                    self.pending = frames(&bytes, self.applied)
                        .0
                        .into_iter()
                        .map(|(number, ops)| self.commit(number, ops, limits, names, connector))
                        .collect();
                }
                Observed::Recovered(path) if is_log(&path) => {
                    if let Some(bytes) = self.files.get(&path) {
                        self.recovered.extend(frames(bytes, 0).0.into_iter().map(|(number, _)| number));
                    }
                    committed.extend(self.durable(&path, limits, names, connector));
                }
                Observed::Snapshot(path) => {
                    if let Some(bytes) = self.files.get(&path)
                        && let Some((number, ops)) = snapshots::read(bytes)
                        && self.applied < number
                    {
                        let commit = self.commit(number, ops, limits, names, connector);
                        self.apply(&commit, limits);
                        self.recovered.extend(1..=number);
                        committed.push(commit);
                    }
                }
                Observed::Sync(path) if is_log(&path) => {
                    committed.extend(self.durable(&path, limits, names, connector));
                }
                Observed::Sync(path) if path == b"generations/1/store" && self.restoring => {
                    // KV announces its new segment only after replaying every old log.
                    // Capture this boundary before any new startup commit can join it.
                    self.restored = Some(self.rows.clone());
                    self.restoring = false;
                }
                Observed::Sync(_) | Observed::Recovered(_) => {}
            }
        }
        committed
    }
    fn durable(
        &mut self,
        path: &[u8],
        limits: &codec::Limits,
        names: &[core::settings::ModelName],
        connector: &temper_protocol_store::Bounds,
    ) -> Vec<Commit> {
        let mut committed = Vec::new();
        let Some(bytes) = self.files.get(path) else { return committed };
        let start = self.validated.get(path).copied().unwrap_or(0);
        let (frames, used) = frames(&bytes[start..], self.applied);
        self.validated.insert(path.to_vec(), start + used);
        for (number, ops) in frames {
            if number <= self.applied {
                continue;
            }
            let commit = self.commit(number, ops, limits, names, connector);
            self.apply(&commit, limits);
            self.pending.retain(|pending| pending.number > number);
            committed.push(commit);
        }
        committed
    }
    fn apply(&mut self, commit: &Commit, limits: &codec::Limits) {
        for write in &commit.writes {
            match write {
                root::Write::CopySettingsRevision(_) => super::restart::apply(&mut self.rows, write),
                root::Write::Save(row) => {
                    self.rows.insert(row.key(), row.clone());
                    if let root::Record::Core(core::Record::Core(core::CoreRecord::Turn(turn))) = row {
                        let len = transcripts::frame::encode(turn, limits).expect("observed frame bounds").len() as u64;
                        *self.lengths.entry(turn.task).or_default() += len;
                    }
                }
                root::Write::Erase(key) => {
                    self.rows.remove(key);
                }
            }
        }
        self.applied = commit.number;
    }
    fn commit(
        &self,
        number: u64,
        ops: Box<[skein_kv::Op]>,
        limits: &codec::Limits,
        names: &[core::settings::ModelName],
        connector: &temper_protocol_store::Bounds,
    ) -> Commit {
        let mut writes = Vec::new();
        for op in ops {
            match op {
                skein_kv::Op::Put { key, value } => {
                    if let Some(key) = keys::decode(&key) {
                        let row = codec::decode(&key, &value, limits, names).expect("real stored row decodes");
                        if let core::Record::Core(core::CoreRecord::Settings(core::settings::Record::Revision {
                            history: true,
                            revision,
                        })) = &row
                        {
                            // Jig materializes a held-to-history copy as a KV Put. Retain the
                            // operation in the oracle, so a dropped copy cannot go unnoticed.
                            writes.push(root::Write::CopySettingsRevision(revision.head.number));
                        } else {
                            writes.push(root::Write::Save(root::Record::Core(row)));
                        }
                    } else if let Some(keys::Own::Transcript { task }) = keys::decode_own(&key) {
                        let place = codec::component::Transcript::decode(&value).expect("real transcript place");
                        let name = format!("transcripts/{:016x}/{task:016x}", task >> 12).into_bytes();
                        let bytes = self.files.get(&name).expect("transcript write precedes map commit");
                        let mut at = usize::try_from(self.lengths.get(&task).copied().unwrap_or(0))
                            .expect("fixture transcript offset");
                        while at < usize::try_from(place.length).expect("fixture transcript length") {
                            let len = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("frame length")) as usize;
                            let turn = transcripts::frame::decode(&bytes[at..at + len], limits)
                                .expect("whole real transcript frame");
                            writes.push(root::Write::Save(root::Record::Core(core::Record::Core(
                                core::CoreRecord::Turn(turn),
                            ))));
                            at += len;
                        }
                    } else if key.first() == Some(&keys::APPLICATION) {
                        let row =
                            temper_protocol_store::records::decode(&value, connector).expect("real application row");
                        writes.push(root::Write::Save(root::Record::Forge { row: Box::new(row) }));
                    }
                }
                skein_kv::Op::Erase { key } => {
                    if let Some(key) = keys::decode(&key) {
                        writes.push(root::Write::Erase(root::Key::Core(key)));
                    } else if key.first() == Some(&keys::APPLICATION) {
                        let key = temper_protocol_store::keys::decode(&key[1..], connector.forge())
                            .expect("real application key");
                        writes.push(root::Write::Erase(root::Key::Forge(key)));
                    }
                }
            }
        }
        Commit { number, writes }
    }
}
type Frames = Vec<(u64, Box<[skein_kv::Op]>)>;
fn frames(bytes: &[u8], after: u64) -> (Frames, usize) {
    let mut reader = Reader::new(bytes);
    let mut frames = Vec::new();
    let mut used = 0;
    while reader.remaining() >= 26 {
        let start = bytes.len() - reader.remaining() as usize;
        assert_eq!(reader.bytes(4), Some(b"SKVC".as_slice()), "real KV frame magic");
        assert_eq!(reader.u16(), Some(1), "real KV frame version");
        let number = reader.u64().expect("frame number");
        let count = reader.u32().expect("frame writes");
        let body = reader.u32().expect("frame body length");
        if reader.remaining() < body + 4 {
            break;
        }
        let end = (bytes.len() - reader.remaining() as usize) + body as usize;
        let mut ops = Vec::new();
        for _ in 0..count {
            let kind = reader.u8().expect("write kind");
            let len = reader.u32().expect("key length");
            let key = reader.bytes(len).expect("key");
            match kind {
                1 => {
                    let len = reader.u32().expect("value length");
                    let value = reader.bytes(len).expect("value");
                    if number > after {
                        ops.push(skein_kv::Op::Put { key: key.into(), value: value.into() });
                    }
                }
                2 => {
                    if number > after {
                        ops.push(skein_kv::Op::Erase { key: key.into() });
                    }
                }
                _ => panic!("real KV write kind"),
            }
        }
        assert_eq!((bytes.len() - reader.remaining() as usize), end, "whole frame operations");
        let expected = transcripts::frame::crc32c(&bytes[start..end]);
        assert_eq!(reader.u32(), Some(expected), "real KV frame checksum");
        used = bytes.len() - reader.remaining() as usize;
        if number > after {
            frames.push((number, ops.into_boxed_slice()));
        }
    }
    (frames, used)
}

fn is_log(path: &[u8]) -> bool {
    path.strip_prefix(b"generations/1/store/log-")
        .is_some_and(|number| number.len() == 20 && number.iter().all(u8::is_ascii_digit))
}

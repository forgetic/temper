//! Root store requests and terminals, with temper's decoded families.
use super::ranges;
use ::core::mem::size_of;
use alloc::boxed::Box;
use jig_domain_core as core;
use jig_protocol_store as store;
use skein_lib::{List, Map, Queue, Token};
use temper_engine_domain as engine;
use temper_protocol_store as families;

/// A bounded load table and transcript continuations, never decoded rows.
#[derive(Debug)]
pub struct Routes {
    pending: Map<Token, Flight>,
    reads: Map<Token, Token>,
    next: u64,
    commit: u64,
    limits: engine::loads::Limits,
    families: families::Bounds,
}
#[derive(Debug)]
pub(super) struct Flight {
    root: Token,
    pub(super) range: engine::Range,
    pub(super) after: Option<engine::Key>,
    pub(super) most: u32,
    pub(super) bytes: u32,
    pub(super) offset: Option<u64>,
    waiting: bool,
    pub(super) cursor: Option<store::Cursor>,
}
impl Routes {
    /// Size the continuation table from the root's admitted loads.
    #[must_use]
    pub fn new(table: u32, limits: &engine::Limits, families: families::Bounds) -> Routes {
        Routes {
            pending: Map::with_capacity(table),
            reads: Map::with_capacity(table),
            next: 0,
            commit: 0,
            limits: limits.loads,
            families,
        }
    }
    /// Whether every routed load has had its terminal.
    #[must_use]
    pub fn idle(&self) -> bool {
        if !self.reads.is_empty() {
            return false;
        }
        for (_, flight) in &self.pending {
            if flight.waiting {
                return false;
            }
        }
        true
    }
    fn token(&mut self) -> Option<Token> {
        self.next = self.next.checked_add(1)?;
        Some(Token::new(self.next))
    }
    fn continuation(&mut self, range: &engine::Range, after: Option<&engine::Key>) -> Option<u64> {
        let mut found = None;
        let mut retired = None;
        for (owner, flight) in &self.pending {
            if !flight.waiting {
                retired = Some(*owner);
                if flight.range == *range && flight.after.as_ref() == after {
                    found = Some(*owner);
                    break;
                }
            }
        }
        if let Some(owner) = found {
            return self.pending.remove(&owner)?.offset;
        }
        // A continuation that is never asked again must not consume an IO slot.
        if let Some(owner) = retired {
            self.pending.remove(&owner);
        }
        None
    }
}

/// A root terminal or the next component page of a filtered load.
#[derive(Debug)]
pub enum Up {
    Root(Box<engine::Event>),
    Ask(store::Request),
}

/// Route one root request; table saturation refuses before issuing a load.
#[must_use]
pub fn down(routes: &mut Routes, request: engine::StoreRequest) -> Option<store::Request> {
    Some(match request {
        engine::StoreRequest::Commit { number, writes } => {
            let mut out = List::with_capacity(u32::try_from(writes.len()).ok()?);
            for write in writes {
                let translated = match write {
                    engine::Write::CopySettingsRevision(number) => store::Write::CopySettingsRevision(number),
                    engine::Write::Save(engine::Record::Core(row)) => store::Write::Save(row),
                    engine::Write::Erase(engine::Key::Core(key)) => store::Write::Erase(key),
                    engine::Write::Save(engine::Record::Forge { row }) => store::Write::Put {
                        key: families::keys::encode(&temper_engine_domain_forge::stored_key(&row), u32::MAX)?,
                        value: families::records::encode(&row),
                    },
                    engine::Write::Erase(engine::Key::Forge(key)) => {
                        store::Write::Remove { key: families::keys::encode(&key, u32::MAX)? }
                    }
                };
                out.push(translated).expect("same write count");
            }
            routes.commit = number;
            store::Request::Commit { number, writes: out.into_boxed() }
        }
        engine::StoreRequest::Read { load, range } => {
            let owner = routes.token()?;
            routes.reads.insert(owner, load).ok()?;
            store::Request::Read { owner, range: *range }
        }
        engine::StoreRequest::Load { owner: root, range, after, most, bytes } => {
            let offset = routes.continuation(&range, after.as_ref());
            let owner = routes.token()?;
            let flight = Flight { root, range, after, most, bytes, offset, waiting: true, cursor: None };
            let request = ranges::request(owner, &flight)?;
            routes.pending.insert(owner, flight).ok()?;
            request
        }
    })
}

/// Every store terminal is accepted independently of journal room.
pub fn up(routes: &mut Routes, event: store::Event, into: &mut Queue<Up>) {
    match event {
        store::Event::Committed { number, fill } => root(into, engine::StoreInput::Committed { number, fill }),
        store::Event::Failed { number } => root(into, engine::StoreInput::Uncommitted { number }),
        store::Event::Restored { owner, rows, next } | store::Event::Loaded { owner, rows, next } => {
            page(routes, owner, rows, next, None, into);
        }
        store::Event::Got { owner, mut row } => {
            if let Some(Flight {
                range: engine::Range::Core(engine::CoreRange::TaskProposal { task, proposal }), ..
            }) = routes.pending.get(&owner)
                && let Some(store::Row::Jig(core::Record::Tasks(jig_domain_tasks::Stored::Live(live)))) = &row
            {
                let mut pointer = None;
                for kept in &live.proposal_rows {
                    if kept.proposal == *proposal {
                        pointer = Some(kept.revision);
                        break;
                    }
                }
                if live.number == *task
                    && let Some(revision) = pointer
                {
                    into.push(Up::Ask(store::Request::Get {
                        owner,
                        key: store::Get::Jig(core::Key::Tasks(jig_domain_tasks::Key::History {
                            task: *task,
                            revision,
                        })),
                    }));
                    return;
                }
                row = None;
            }
            let rows: Box<[store::Row]> = match row {
                Some(row) => Box::new([row]),
                None => Box::new([]),
            };
            page(routes, owner, rows, None, None, into);
        }
        store::Event::Turns { owner, turns, next } => {
            let mut rows = List::with_capacity(u32::try_from(turns.len()).expect("bounded turns"));
            for turn in turns {
                rows.push(store::Row::Jig(core::Record::Core(core::CoreRecord::Turn(turn)))).expect("same count");
            }
            page(routes, owner, rows.into_boxed(), None, next, into);
        }
        store::Event::Unread { owner } => {
            if let Some(flight) = routes.pending.remove(&owner) {
                root(into, engine::StoreInput::Unloaded { owner: flight.root });
            }
        }
        store::Event::Read { owner, result } => {
            if let Some(load) = routes.reads.remove(&owner) {
                root(into, engine::StoreInput::ReadLoaded { load, result });
            }
        }
        store::Event::Corrupt { owner } => {
            routes.pending.remove(&owner);
            stop(routes, into);
        }
        store::Event::Site { owner, of, site, patcher, .. } => {
            into.push(Up::Ask(dispatch(owner, of, site, patcher)));
        }
        store::Event::OpenRefused { .. } => stop(routes, into),
        store::Event::Patched { .. }
        | store::Event::Opened { .. }
        | store::Event::Kept { .. }
        | store::Event::Carried { .. }
        | store::Event::Stale { .. }
        | store::Event::Erased { .. }
        | store::Event::Secret { .. }
        | store::Event::SecretsLoaded { .. } => {}
    }
}
fn root(into: &mut Queue<Up>, event: engine::StoreInput) {
    into.push(Up::Root(Box::new(engine::Event::Store(event))));
}
fn stop(routes: &Routes, into: &mut Queue<Up>) {
    root(into, engine::StoreInput::Uncommitted { number: routes.commit });
}

fn page(
    routes: &mut Routes,
    owner: Token,
    rows: Box<[store::Row]>,
    cursor: Option<store::Cursor>,
    offset: Option<u64>,
    into: &mut Queue<Up>,
) {
    let more = cursor.is_some() || offset.is_some();
    let Some(mut flight) = routes.pending.remove(&owner) else {
        return;
    };
    let mut decoded = List::with_capacity(flight.most);
    let mut bytes = 0_u64;
    let mut cutting = false;
    let mut last = flight.after.clone();
    let mut scanned = flight.after.clone();
    for row in rows {
        let Some(record) = decode(row, &routes.families) else {
            stop(routes, into);
            return;
        };
        let key = record.key();
        scanned = Some(key.clone());
        if let engine::Range::Census(_) = &flight.range {
        } else if !flight.range.contains(&key) {
            continue;
        }
        if let engine::Range::Census(_) = &flight.range {
        } else if let Some(after) = &flight.after
            && key <= *after
        {
            continue;
        }
        let Some((payload_bytes, total)) = page_bytes(&record, bytes) else {
            stop(routes, into);
            return;
        };
        if cutting
            || decoded.len() >= flight.most
            || total > u64::from(flight.bytes)
            || payload_bytes > u64::from(routes.limits.transcript_bytes)
        {
            cutting = true;
            continue;
        }
        bytes = total;
        last = Some(key);
        decoded.push(record).expect("checked prefix");
    }
    if cutting && decoded.is_empty() {
        stop(routes, into);
        return;
    }
    if let engine::Range::Census(_) = &flight.range {
        if cutting {
            stop(routes, into);
            return;
        }
        let next = match cursor {
            Some(cursor) => match store::keys::decode(&cursor.after) {
                Some(key) => Some(engine::Key::Core(key)),
                None => {
                    stop(routes, into);
                    return;
                }
            },
            None => None,
        };
        root(into, engine::StoreInput::Loaded { texts: None, owner: flight.root, rows: decoded.into_boxed(), next });
        return;
    }
    if decoded.is_empty() && !cutting && more {
        if let Some(next) = &cursor
            && (next.after.is_empty() || flight.cursor.as_ref() == Some(next))
        {
            stop(routes, into);
            return;
        }
        flight.after = scanned;
        flight.cursor = cursor;
        flight.offset = offset;
        if let Some(request) = ranges::request(owner, &flight) {
            routes.pending.insert(owner, flight).expect("removed slot");
            into.push(Up::Ask(request));
        } else {
            stop(routes, into);
        }
        return;
    }
    let next = if cutting || more { last } else { None };
    let texts = ranges::texts(&flight.range, decoded.as_slice());
    // For a cut transcript page restart from its beginning and filter by key;
    // the component's offset names the end of the whole incoming page.
    if offset.is_some() && !cutting {
        flight.after.clone_from(&next);
        flight.offset = offset;
        flight.waiting = false;
        routes.pending.insert(owner, flight).expect("removed slot");
        let root_owner = routes.pending.get(&owner).expect("kept continuation").root;
        root(into, engine::StoreInput::Loaded { texts, owner: root_owner, rows: decoded.into_boxed(), next });
    } else {
        root(into, engine::StoreInput::Loaded { texts, owner: flight.root, rows: decoded.into_boxed(), next });
    }
}

fn decode(row: store::Row, bounds: &families::Bounds) -> Option<engine::Record> {
    Some(match row {
        store::Row::Jig(row) => engine::Record::Core(row),
        store::Row::Application { key, value } => {
            engine::Record::Forge { row: Box::new(families::records::row(&key, &value, bounds).ok()?) }
        }
    })
}

fn page_bytes(record: &engine::Record, kept: u64) -> Option<(u64, u64)> {
    let payload = engine::record_bytes(record)?;
    let slot = u64::try_from(size_of::<engine::Record>()).ok()?.checked_add(payload)?;
    Some((payload, kept.checked_add(slot)?))
}

pub(crate) fn worst_case(limits: &engine::Limits, key: u32, payload: u64) -> Option<u64> {
    Map::<Token, Flight>::worst_case(limits.loads.loads)?
        .checked_add(Map::<Token, Token>::worst_case(limits.loads.loads)?)?
        .checked_add(payload.checked_add(u64::from(key))?.checked_mul(u64::from(limits.loads.loads))?)
}

/// No owner has registered a written patch site in this build. Hand the
/// machine back with the named refusal, so the active generation is preserved.
fn dispatch(
    owner: Token,
    of: store::Owner,
    site: skein_codec::patch::Site,
    patcher: Box<skein_codec::patch::Patcher>,
) -> store::Request {
    let result = match of {
        store::Owner::Skein | store::Owner::Smith | store::Owner::Jig | store::Owner::Application => {
            Err(skein_codec::patch::Halt::NoSite(site))
        }
    };
    store::Request::Site { owner, patcher, result }
}

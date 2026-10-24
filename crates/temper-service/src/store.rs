//! The deployment store and its three file drivers (protocol/store.md, 3–5;
//! jig's protocol/store.md, 13).
mod ranges;
pub mod route;
use jig_protocol_store as protocol;
use route::Up;
use skein_io::{
    file,
    file_layer::{self, FileIo},
    kernel::{Complete, Fd, Op, Submit},
};
use skein_lib::{Duration, Env, Map, Queue, Time, Token, Wall};
use temper_engine_domain as root;
use temper_engine_domain_forge as forge;
const ADOPT: Token = Token::new(u64::MAX - 1);
const CLOSE: Token = Token::new(u64::MAX);
const MASK: u64 = (1_u64 << 60) - 1;

#[derive(Debug)]
struct Driver {
    io: FileIo,
    above: Queue<file::Event>,
    handles: Map<Token, ()>,
}
#[derive(Debug)]
pub struct Store {
    #[cfg(test)]
    pub(crate) forge_loads: skein_lib::List<Option<u64>>,
    #[cfg(test)]
    pub(crate) erases: u32,
    pub(crate) component: protocol::Store,
    env: Env<protocol::Limits>,
    pub(crate) routes: route::Routes,
    pub(crate) above: Queue<protocol::Event>,
    pub(crate) routed: Queue<Up>,
    pub(crate) secrets: Queue<protocol::Event>,
    /// The answers to the accounts' records' loads and writes (jig's store.md 9).
    pub(crate) sources: Queue<protocol::Event>,
    below: protocol::Below,
    drivers: [Driver; 3],
    pub(crate) submitted: Queue<Submit>,
    pub(crate) opened: bool,
    /// The installation's identifier: the drawn one until the store opens,
    /// then the one it keeps (jig's store.md 4.4).
    pub(crate) installation: [u8; 16],
    adopted: bool,
    root: Option<Fd>,
    closing: bool,
}

/// Admit the core, journal and temper families, then copy checked sizes to KV.
#[must_use]
pub fn limits(
    root: &root::Limits,
    core: &jig_domain_core::Limits,
    forge: &forge::Limits,
    profile: &protocol::Limits,
) -> Option<protocol::Limits> {
    let mut kv = profile.store;
    let sizes = protocol::sizes::check(
        core,
        &temper_protocol_store::application(forge)?,
        &root::journal_limits(&root.journal),
        &kv,
        profile.orphan_erases,
    )
    .ok()?;
    kv.key = sizes.key;
    kv.value = sizes.value;
    kv.ops = sizes.ops;
    kv.commit = sizes.commit;
    kv.queued_bytes = u64::from(sizes.commit).checked_mul(u64::from(kv.queued))?;
    kv.segment = u64::from(sizes.commit).checked_mul(2)?;
    kv.snapshot_after = u64::from(sizes.commit);
    kv.chunk = sizes.key.checked_add(sizes.value)?.checked_add(20)?;
    kv.page.bytes = sizes.key.checked_add(sizes.value)?;
    Some(protocol::Limits {
        core: *core,
        store: kv,
        secrets: kv,
        held: profile.held,
        journal: root::journal_limits(&root.journal),
        joins: profile.joins,
        queue: kv.queued,
        files: profile.files,
        reads: core.load_slots.checked_add(core.tasks.tasks)?,
        orphan_erases: profile.orphan_erases,
    })
}
impl Store {
    #[cfg(test)]
    pub(crate) fn opening_limits(&self) -> protocol::Limits {
        self.env.limits
    }

    #[must_use]
    pub fn new(
        application: &root::Limits,
        limits: &protocol::Limits,
        models: &[jig_domain_core::Model],
        installation: [u8; 16],
    ) -> Self {
        let room = root::max_out(application).max(protocol::MAX_OUT_DOWN).checked_mul(2).expect("store route room");
        let routes = route::Routes::new(
            application.loads.loads,
            application,
            temper_protocol_store::bounds(&application.forge).expect("checked family bounds"),
        );
        let mut drivers = [driver(limits, room), driver(limits, room), driver(limits, room)];
        let [store, secrets, transcripts] = &mut drivers;
        let seeded = protocol::seed(
            [&mut store.io, &mut secrets.io, &mut transcripts.io],
            u64::from_le_bytes(*installation.first_chunk::<8>().expect("installation seed")),
        );
        Self {
            #[cfg(test)]
            forge_loads: skein_lib::List::with_capacity(128),
            #[cfg(test)]
            erases: 0,
            component: protocol::Store::on_directory(
                limits,
                temper_protocol_store::application(&application.forge).expect("checked application sizes"),
                models,
                seeded,
            ),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *limits },
            routes,
            above: Queue::with_capacity(room),
            routed: Queue::with_capacity(room),
            secrets: Queue::with_capacity(room),
            sources: Queue::with_capacity(room),
            below: protocol::Below {
                store: Queue::with_capacity(room),
                secrets: Queue::with_capacity(room),
                transcripts: Queue::with_capacity(room),
            },
            drivers,
            submitted: Queue::with_capacity(room),
            opened: false,
            installation,
            adopted: false,
            root: None,
            closing: false,
        }
    }
    /// Whether the component has reported that its deployment opened.
    #[must_use]
    pub const fn opened(&self) -> bool {
        self.opened
    }
    pub fn adopt(&mut self, fd: Fd, user: u32) -> Result<(), Fd> {
        if self.adopted {
            return Err(fd);
        }
        self.adopted = true;
        for index in 0..3 {
            let driver = self.drivers.get_mut(index).expect("driver index");
            driver.io = core::mem::replace(&mut driver.io, FileIo::new(1, 1, 1, Duration::from_secs(1)))
                .with_effective_user(user);
            if index == 0 {
                let file = driver.io.adopt_root(fd).expect("deployment root slot");
                driver.handles.insert(file, ()).expect("owned deployment root");
            }
        }
        self.root = Some(fd);
        let clone =
            file::Request::OpenDirectory { owner: ADOPT, root: fd, name: b".".as_slice().into(), no_follow: true };
        self.below.secrets.push(clone);
        self.below.transcripts.push(file::Request::OpenDirectory {
            owner: ADOPT,
            root: fd,
            name: b".".as_slice().into(),
            no_follow: true,
        });
        Ok(())
    }
    pub fn clock(&mut self, now: Time, wall: Wall) {
        self.env.now = now;
        self.env.wall = wall;
    }
    pub fn ask(&mut self, request: protocol::Request) {
        match request {
            protocol::Request::Secrets { owner, request } => {
                if let Some(event) = self.secrets(owner, request) {
                    self.above.push(event);
                }
            }
            request @ (protocol::Request::Open { .. }
            | protocol::Request::Commit { .. }
            | protocol::Request::Restore { .. }
            | protocol::Request::Load { .. }
            | protocol::Request::Read { .. }
            | protocol::Request::Get { .. }
            | protocol::Request::Transcript { .. }
            | protocol::Request::Site { .. }) => {
                protocol::down(&mut self.component, &self.env, request, &mut self.above, &mut self.below);
            }
        }
    }
    pub fn down(&mut self, out: root::StoreRequest) -> bool {
        if let Some(request) = route::down(&mut self.routes, out) {
            self.ask(request);
            true
        } else {
            false
        }
    }
    /// Get answers stay in this pass; secret writes' terminals are handed to their owner.
    pub fn secrets(&mut self, owner: Token, request: protocol::Secrets) -> Option<protocol::Event> {
        #[cfg(test)]
        if owner == crate::forge::records::LOAD
            && let protocol::Secrets::Load { after, .. } = &request
        {
            self.forge_loads.push(*after).expect("test page trace");
        }
        #[cfg(test)]
        if let protocol::Secrets::Erase { .. } = &request {
            self.erases = self.erases.checked_add(1).expect("test erases");
        }
        let immediate = match &request {
            protocol::Secrets::Get { .. } | protocol::Secrets::Load { .. } => true,
            protocol::Secrets::AdoptValue { .. }
            | protocol::Secrets::Adopt { .. }
            | protocol::Secrets::Keep { .. }
            | protocol::Secrets::Replace { .. }
            | protocol::Secrets::Erase { .. }
            | protocol::Secrets::Hold { .. }
            | protocol::Secrets::Carry { .. } => false,
        };
        protocol::down(
            &mut self.component,
            &self.env,
            protocol::Request::Secrets { owner, request },
            &mut self.above,
            &mut self.below,
        );
        let mut answer = None;
        if immediate {
            for _ in 0..self.above.len() {
                let event = self.above.pop().expect("store terminals");
                if let protocol::Event::Secret { owner: found, .. }
                | protocol::Event::SecretsLoaded { owner: found, .. } = &event
                    && *found == owner
                {
                    answer = Some(event);
                } else {
                    self.above.push(event);
                }
            }
        }
        answer
    }
    #[must_use]
    pub fn room(&self) -> bool {
        self.secrets.room() >= protocol::MAX_OUT_DOWN
            && self.sources.room() >= protocol::MAX_OUT_DOWN
            && self.above.room() >= protocol::MAX_OUT_DOWN
            && self.routed.room() >= protocol::MAX_OUT_DOWN
            && self.below.store.room() >= protocol::MAX_OUT_DOWN
            && self.below.secrets.room() >= protocol::MAX_OUT_DOWN
            && self.below.transcripts.room() >= protocol::MAX_OUT_DOWN
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let mut next: Option<Time> = None;
        for driver in &self.drivers {
            next = match (next, driver.io.next_deadline()) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (left, None) => left,
                (None, right) => right,
            };
        }
        next
    }
    #[must_use]
    pub fn work_pending(&self, now: Time) -> bool {
        if !self.secrets.is_empty()
            || !self.sources.is_empty()
            || !self.above.is_empty()
            || !self.routed.is_empty()
            || !self.submitted.is_empty()
        {
            return true;
        }
        for index in 0..3 {
            let driver = self.drivers.get(index).expect("driver index");
            if driver.io.is_due(now)
                || !self.drivers.get(index).expect("driver index").above.is_empty()
                || driver.io.takes() && (!self.queue(index).is_empty() || self.can_close(index))
            {
                return true;
            }
        }
        false
    }
    fn can_close(&self, index: usize) -> bool {
        self.closing
            && !self.drivers.get(index).expect("driver index").handles.is_empty()
            && (index != 0
                || self.drivers[1].handles.is_empty()
                    && self.drivers[2].handles.is_empty()
                    && self.drivers[1].io.takes()
                    && self.drivers[2].io.takes()
                    && self.drivers[1].above.is_empty()
                    && self.drivers[2].above.is_empty())
    }
    #[must_use]
    pub fn closed(&self) -> bool {
        self.closing && self.idle() && empty_handles(&self.drivers)
    }
    #[must_use]
    pub fn idle(&self) -> bool {
        (self.closing || self.routes.idle())
            && self.secrets.is_empty()
            && self.sources.is_empty()
            && self.above.is_empty()
            && self.routed.is_empty()
            && self.submitted.is_empty()
            && self.below.store.is_empty()
            && self.below.secrets.is_empty()
            && self.below.transcripts.is_empty()
            && idle_drivers(&self.drivers)
    }
    pub fn close(&mut self) {
        self.closing = true;
        self.root = None;
        for _ in 0..self.above.len() {
            let _terminal = self.above.pop();
        }
        for _ in 0..self.routed.len() {
            let _up = self.routed.pop();
        }
        for _ in 0..self.secrets.len() {
            let _terminal = self.secrets.pop();
        }
        for _ in 0..self.sources.len() {
            let _terminal = self.sources.pop();
        }
        for index in 0..3 {
            for _ in 0..self.queue(index).len() {
                let _request = self.pop(index);
            }
        }
    }
    pub fn complete(&mut self, mut complete: Complete) {
        let index = usize::try_from((complete.op.raw() >> 60_u32).checked_sub(8).expect("driver namespace"))
            .expect("three drivers");
        complete.op = Token::new(complete.op.raw() & MASK);
        if let Op::Cancel { target } = &mut complete.kind {
            *target = Token::new(target.raw() & MASK);
        }
        let mut submitted = Queue::with_capacity(4);
        let driver = self.drivers.get_mut(index).expect("driver index");
        file_layer::up(&mut driver.io, complete, &mut driver.above, &mut submitted);
        self.publish(index, &mut submitted);
    }
    fn publish(&mut self, index: usize, queue: &mut Queue<Submit>) {
        let prefix = *crate::tokens::STORE.get(index).expect("driver prefix");
        for _ in 0..queue.len() {
            let mut submit = queue.pop().expect("file submissions");
            submit.op = Token::new(submit.op.raw() | prefix);
            if let Op::Cancel { target } = &mut submit.kind {
                *target = Token::new(target.raw() | prefix);
            }
            self.submitted.push(submit);
        }
    }
    fn queue(&self, index: usize) -> &Queue<file::Request> {
        if index == 0 {
            &self.below.store
        } else if index == 1 {
            &self.below.secrets
        } else {
            &self.below.transcripts
        }
    }
    fn pop(&mut self, index: usize) -> Option<file::Request> {
        if index == 0 {
            self.below.store.pop()
        } else if index == 1 {
            self.below.secrets.pop()
        } else {
            self.below.transcripts.pop()
        }
    }
    fn descriptor(&mut self, index: usize, event: &file::Event) -> Option<Fd> {
        if let file::Event::Opened { file, .. } = event {
            self.drivers
                .get_mut(index)
                .expect("driver index")
                .handles
                .insert(*file, ())
                .expect("bounded open file handles");
            return self.drivers.get(index).expect("driver index").io.descriptor(*file);
        }
        None
    }
    fn close_request(&mut self, index: usize) -> Option<file::Request> {
        if self.can_close(index) {
            let handles = &mut self.drivers.get_mut(index).expect("driver index").handles;
            if let Some((token, ())) = handles.iter().last() {
                let file = *token;
                handles.remove(&file);
                return Some(close(file));
            }
        }
        None
    }
    pub fn iterate(&mut self) {
        if self.adopted && !self.closing && self.room() {
            protocol::fire(&mut self.component, &self.env, &mut self.above, &mut self.below);
        }
        for index in 0..3 {
            let mut submitted = Queue::with_capacity(4);
            file_layer::expire(
                &mut self.drivers.get_mut(index).expect("driver index").io,
                self.env.now,
                &mut submitted,
            );
            for _ in 0..self.drivers.get(index).expect("driver index").above.len() {
                if !self.room() {
                    break;
                }
                let event = self.drivers.get_mut(index).expect("driver index").above.pop().expect("file terminal");
                if event.owner() == CLOSE {
                    continue;
                }
                let fd = self.descriptor(index, &event);
                if event.owner() == ADOPT {
                    if fd.is_none() {
                        self.root = None;
                        self.above.push(protocol::Event::OpenRefused {
                            owner: Token::new(1),
                            why: protocol::OpenRefusal::Failed,
                        });
                    } else if !self.drivers[1].handles.is_empty()
                        && !self.drivers[2].handles.is_empty()
                        && let Some(fd) = self.root.take()
                    {
                        self.ask(protocol::Request::Open {
                            owner: Token::new(1),
                            root: protocol::Root { token: Token::new(1), fd },
                            installation: self.installation,
                        });
                    }
                    continue;
                }
                let arrived = protocol::Arrived { event, fd };
                let event = if index == 0 {
                    protocol::Io::Store(arrived)
                } else if index == 1 {
                    protocol::Io::Secrets(arrived)
                } else {
                    protocol::Io::Transcripts(arrived)
                };
                if !self.closing {
                    protocol::up(&mut self.component, &self.env, event, &mut self.above, &mut self.below);
                }
            }
            if self.drivers.get(index).expect("driver index").io.takes() {
                let request = self.pop(index);
                if let Some(file::Request::Close { file, .. }) = &request {
                    self.drivers.get_mut(index).expect("driver index").handles.remove(file);
                }
                let request = match request {
                    Some(request) => Some(request),
                    None => self.close_request(index),
                };
                if let Some(request) = request {
                    let driver = self.drivers.get_mut(index).expect("driver index");
                    file_layer::down(&mut driver.io, self.env.now, request, &mut driver.above, &mut submitted);
                }
            }
            self.publish(index, &mut submitted);
        }
    }
}
#[must_use]
pub fn owns(token: Token) -> bool {
    crate::tokens::STORE.contains(&(token.raw() & !MASK))
}

fn handles(limits: &protocol::Limits) -> u32 {
    limits.files.checked_add(4).expect("store file handles fit")
}
fn max_read(limits: &protocol::Limits) -> u32 {
    limits
        .store
        .chunk
        .max(limits.core.resume_bytes)
        .max(u32::try_from(limits.core.load_bytes.min(u64::from(u32::MAX))).expect("file read length"))
}
fn driver(limits: &protocol::Limits, capacity: u32) -> Driver {
    Driver {
        io: FileIo::new(handles(limits), max_read(limits), 32, limits.store.deadline),
        above: Queue::with_capacity(capacity),
        handles: Map::with_capacity(handles(limits)),
    }
}
fn close(file: Token) -> file::Request {
    file::Request::Close { owner: CLOSE, file }
}

fn empty_handles(drivers: &[Driver; 3]) -> bool {
    for driver in drivers {
        if !driver.handles.is_empty() {
            return false;
        }
    }
    true
}
fn idle_drivers(drivers: &[Driver; 3]) -> bool {
    for driver in drivers {
        if !driver.io.takes() || !driver.above.is_empty() {
            return false;
        }
    }
    true
}

pub(crate) fn worst_case(application: &root::Limits, limits: &protocol::Limits) -> Option<u64> {
    let room = root::max_out(application).max(protocol::MAX_OUT_DOWN).checked_mul(2)?;
    // Queues and load flights price the same immutable payload envelope.
    let payload = root::payload_worst_case(application)?;
    protocol::worst_case(limits)?
        .checked_add(u64::try_from(size_of::<Store>()).ok()?)?
        .checked_add(FileIo::worst_case(handles(limits), max_read(limits), 32, max_read(limits))?.checked_mul(3)?)?
        .checked_add(Queue::<file::Request>::worst_case(room)?.checked_mul(3)?)?
        .checked_add(Queue::<file::Event>::worst_case(room)?.checked_mul(3)?)?
        .checked_add(Queue::<protocol::Event>::worst_case(room)?.checked_mul(3)?)?
        .checked_add(Queue::<Up>::worst_case(room)?)?
        .checked_add(Queue::<Submit>::worst_case(room)?)?
        .checked_add(Map::<Token, ()>::worst_case(handles(limits))?.checked_mul(3)?)?
        .checked_add(route::worst_case(application, limits.store.key, payload)?)?
        .checked_add(u64::from(max_read(limits)).checked_mul(u64::from(room))?.checked_mul(6)?)?
        .checked_add(payload.checked_mul(u64::from(room))?)
}

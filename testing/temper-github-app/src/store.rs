//! Ordinary test harness; jig owns all store formats and durability.
use jig_protocol_store::{self as protocol, Event, Request, SecretOwner, Secrets};
use skein_io::{
    file,
    file_layer::{self, FileIo},
    kernel::{Complete, Exit, Op, Submit},
};
use skein_lib::{Env, Queue, Secret, Time, Token, Wall};
use std::{
    collections::{BTreeSet, VecDeque},
    path::Path,
};

type Rows = Vec<(u64, Box<[u8]>)>;
const OWNER: Token = Token::new(1);
const CLOSE: Token = Token::new(u64::MAX);
const MASK: u64 = (1 << 60) - 1;
const ROOM: u32 = 128;
const OPERATIONS: u32 = 64;

pub(super) fn run(data: &Path, records: &[(u64, Box<[u8]>)], load: bool) -> Result<Rows, String> {
    let jig = jig_protocol_config::profile(jig_protocol_config::Profile::Small);
    let own = temper_service::limits::profile(temper_service::limits::Profile::Small);
    let root = temper_service::limits::engine(&jig, &own).ok_or("fixture engine limits")?;
    let limits =
        temper_service::store::limits(&root, &jig.core, &root.forge, &jig.store).ok_or("fixture store limits")?;
    if records.iter().any(|(_, value)| value.len() > usize::try_from(limits.secrets.value).expect("store value bound"))
    {
        return Err("fixture record exceeds the secrets store value bound".into());
    }
    let application = temper_protocol_store::application(&root.forge).ok_or("fixture application sizes")?;
    // The caller supplies an existing private data directory, just as start does.
    let fd = skein_shell::open_root(data).map_err(|why| format!("data directory: errno {why}"))?;
    let mut kernel = match skein_shell::Kernel::open(skein_shell::Config { operations: OPERATIONS }) {
        Ok(kernel) => kernel,
        Err(why) => {
            skein_shell::close_keeper_fd(fd);
            return Err(why.to_string());
        }
    };
    let mut drivers: [Driver; 3] = std::array::from_fn(|_| Driver {
        io: FileIo::new(limits.files + 4, limits.store.chunk.max(limits.core.resume_bytes), 32, limits.store.deadline)
            .with_effective_user(skein_shell::effective_user()),
        events: Queue::with_capacity(ROOM),
        handles: BTreeSet::new(),
    });
    let token = drivers[0].io.adopt_root(fd).expect("data root slot");
    drivers[0].handles.insert(token);
    for driver in &mut drivers[1..] {
        let cloned = skein_shell::open_root(data).expect("fixture data root reopens");
        let cloned_token = driver.io.adopt_root(cloned).expect("data root slot");
        assert_eq!(cloned_token, token);
        driver.handles.insert(cloned_token);
    }
    let [store, secrets, transcripts] = &mut drivers;
    let seeded = protocol::seed([&mut store.io, &mut secrets.io, &mut transcripts.io], 0);
    let mut host = StoreHost {
        store: protocol::Store::on_directory(&limits, application, &[], seeded),
        limits,
        drivers,
        above: Queue::with_capacity(ROOM),
        below: protocol::Below {
            store: Queue::with_capacity(ROOM),
            secrets: Queue::with_capacity(ROOM),
            transcripts: Queue::with_capacity(ROOM),
        },
        completions: Queue::with_capacity(OPERATIONS),
        submissions: Queue::with_capacity(OPERATIONS),
        request: Some(Request::Open { owner: OWNER, root: protocol::Root { token, fd }, installation: [0; 16] }),
        records: records.iter().cloned().collect(),
        rows: Vec::new(),
        load,
        closing: false,
        error: None,
    };
    skein_shell::drive(&mut kernel, &skein_shell::Clock::new(), &mut host);
    match host.error {
        Some(why) => Err(why),
        None => Ok(host.rows),
    }
}
struct Driver {
    io: FileIo,
    events: Queue<file::Event>,
    handles: BTreeSet<Token>,
}
struct StoreHost {
    store: protocol::Store,
    limits: protocol::Limits,
    drivers: [Driver; 3],
    above: Queue<Event>,
    below: protocol::Below,
    completions: Queue<Complete>,
    submissions: Queue<Submit>,
    request: Option<Request>,
    records: VecDeque<(u64, Box<[u8]>)>,
    rows: Rows,
    load: bool,
    closing: bool,
    error: Option<String>,
}
impl StoreHost {
    fn next(&mut self) {
        if let Some((number, value)) = self.records.pop_front() {
            self.request = Some(Request::Secrets {
                owner: OWNER,
                request: Secrets::Keep { owner: SecretOwner::Application, number, value: Secret::new(value) },
            });
        } else if self.load {
            self.request = Some(Request::Secrets {
                owner: OWNER,
                request: Secrets::Load { owner: SecretOwner::Application, after: None },
            });
        } else {
            self.closing = true;
        }
    }
    fn answer(&mut self, event: Event) {
        match event {
            Event::Opened { .. } | Event::Kept { .. } => self.next(),
            Event::SecretsLoaded { rows, next, .. } => {
                for row in rows {
                    self.rows.push((row.number, row.value.expose().into()));
                }
                if let Some(after) = next {
                    self.request = Some(Request::Secrets {
                        owner: OWNER,
                        request: Secrets::Load { owner: SecretOwner::Application, after: Some(after) },
                    });
                } else {
                    self.closing = true;
                }
            }
            other => {
                self.error = Some(format!("fixture store: {other:?}"));
                self.closing = true;
            }
        }
    }
    fn publish(&mut self, index: usize, submitted: &mut Queue<Submit>) {
        let prefix = (u64::try_from(index).expect("three drivers") + 8) << 60;
        while let Some(mut submit) = submitted.pop() {
            submit.op = Token::new(submit.op.raw() | prefix);
            if let Op::Cancel { target } = &mut submit.kind {
                *target = Token::new(target.raw() | prefix);
            }
            self.submissions.push(submit);
        }
    }
    fn queue(&mut self, index: usize) -> &mut Queue<file::Request> {
        match index {
            0 => &mut self.below.store,
            1 => &mut self.below.secrets,
            2 => &mut self.below.transcripts,
            _ => unreachable!(),
        }
    }
}
impl skein_shell::Host for StoreHost {
    fn iterate(&mut self, now: Time, wall: Wall) {
        let env = Env { now, wall, limits: self.limits };
        while let Some(mut complete) = self.completions.pop() {
            let index = usize::try_from((complete.op.raw() >> 60) - 8).expect("three driver prefixes");
            complete.op = Token::new(complete.op.raw() & MASK);
            if let Op::Cancel { target } = &mut complete.kind {
                *target = Token::new(target.raw() & MASK);
            }
            let mut submitted = Queue::with_capacity(4);
            let driver = &mut self.drivers[index];
            file_layer::up(&mut driver.io, complete, &mut driver.events, &mut submitted);
            self.publish(index, &mut submitted);
        }
        for index in 0..3 {
            while let Some(event) = self.drivers[index].events.pop() {
                if event.owner() == CLOSE {
                    continue;
                }
                let fd = if let file::Event::Opened { file, .. } = &event {
                    self.drivers[index].handles.insert(*file);
                    self.drivers[index].io.descriptor(*file)
                } else {
                    None
                };
                if !self.closing {
                    let arrived = protocol::Arrived { event, fd };
                    let io = match index {
                        0 => protocol::Io::Store(arrived),
                        1 => protocol::Io::Secrets(arrived),
                        2 => protocol::Io::Transcripts(arrived),
                        _ => unreachable!(),
                    };
                    protocol::up(&mut self.store, &env, io, &mut self.above, &mut self.below);
                }
            }
        }
        while let Some(event) = self.above.pop() {
            self.answer(event);
        }
        if !self.closing {
            if let Some(request) = self.request.take() {
                protocol::down(&mut self.store, &env, request, &mut self.above, &mut self.below);
            }
            protocol::fire(&mut self.store, &env, &mut self.above, &mut self.below);
        }
        for index in 0..3 {
            let mut submitted = Queue::with_capacity(4);
            file_layer::expire(&mut self.drivers[index].io, now, &mut submitted);
            if self.drivers[index].io.takes() {
                let request = if self.closing {
                    while self.queue(index).pop().is_some() {}
                    // The data root closes after its child drivers, as in temper's start.
                    if index == 0 && self.drivers[1..].iter().any(|d| !d.handles.is_empty() || !d.io.takes()) {
                        None
                    } else {
                        self.drivers[index].handles.pop_last().map(|file| file::Request::Close { owner: CLOSE, file })
                    }
                } else {
                    self.queue(index).pop()
                };
                if let Some(request) = request {
                    if let file::Request::Close { file, .. } = &request {
                        self.drivers[index].handles.remove(file);
                    }
                    let driver = &mut self.drivers[index];
                    file_layer::down(&mut driver.io, now, request, &mut driver.events, &mut submitted);
                }
            }
            self.publish(index, &mut submitted);
        }
    }
    fn completions(&mut self) -> &mut Queue<Complete> {
        &mut self.completions
    }
    fn submissions(&mut self) -> &mut Queue<Submit> {
        &mut self.submissions
    }
    fn work_pending(&self, _now: Time) -> bool {
        self.request.is_some()
            || !self.above.is_empty()
            || self.drivers.iter().enumerate().any(|(i, d)| {
                !d.events.is_empty()
                    || d.io.takes()
                        && (if self.closing {
                            !d.handles.is_empty()
                        } else {
                            match i {
                                0 => !self.below.store.is_empty(),
                                1 => !self.below.secrets.is_empty(),
                                _ => !self.below.transcripts.is_empty(),
                            }
                        })
            })
    }
    fn next_deadline(&self) -> Option<Time> {
        self.drivers.iter().filter_map(|d| d.io.next_deadline()).min()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        None
    }
    fn is_empty(&self) -> bool {
        self.closing
            && self.drivers.iter().all(|d| d.handles.is_empty() && d.io.takes() && d.events.is_empty())
            && self.submissions.is_empty()
            && self.completions.is_empty()
    }
    fn worst_case(&self) -> u64 {
        protocol::worst_case(&self.limits).expect("fixture store bound")
    }
    fn operations(&self) -> u32 {
        OPERATIONS
    }
    fn exit(&self) -> Option<Exit> {
        self.is_empty().then_some(Exit::Code(u8::from(self.error.is_some())))
    }
}

use skein_lib::{Duration, Env, Queue, Time, Token, Wall, stream};
use temper_fake_forge_domain::{self as forge, api};
use temper_fake_forge_protocol::{
    config::{Config, Identity, Repository},
    connection::{self, Connection, Event, Limits},
    service::Service,
};
use temper_forge_forgejo::{
    request::{self, Request},
    types::{Label, ObjectFormat},
};
pub fn limits() -> Limits {
    Limits {
        calls: 8,
        http: skein_http::server::Limits { head: 2048, headers: 16, body: 16384, read: 64, response: 1024, send: 31 },
        documents: temper_forge_forgejo::Limits {
            depth: 16,
            tokens: 2048,
            page: 2,
            fields: 8,
            name_bytes: 256,
            title_bytes: 256,
            body_bytes: 1024,
            marker_bytes: 64,
            document_bytes: 16384,
            hook_bytes: 16384,
        },
    }
}
pub fn settings() -> forge::Config {
    forge::Config {
        limits: forge::Limits {
            repositories: 2,
            users: 4,
            labels: 4,
            items: 16,
            comments: 16,
            dependencies: 8,
            reviews: 8,
            branches: 8,
            commits: 32,
            files: 4,
            statuses: 8,
            contexts: 4,
            pages: 8,
            name_bytes: 256,
            title_bytes: 256,
            body_bytes: 1024,
            content_bytes: 1024,
            page_size: 2,
            calls: 8,
            hooks: 32,
            observations: 64,
        },
        latency_min: Duration::ZERO,
        latency_max: Duration::ZERO,
        late: 0,
        late_min: Duration::ZERO,
        late_max: Duration::ZERO,
        unavailable: 0,
        timeouts: 0,
        landing: 0,
        land_min: Duration::ZERO,
        land_max: Duration::ZERO,
        rate_limit: 0,
        rate_window: Duration::ZERO,
        ci: 1,
        hook_min: Duration::ZERO,
        hook_max: Duration::ZERO,
        hooks_late: 0,
        hooks_lost: 0,
        resolution: Duration::from_secs(1),
        skew: forge::Skew::None,
        status_updates: false,
        edit_updates: false,
    }
}
pub fn config() -> Config {
    Config {
        users: Box::from([
            Identity { id: 1, login: Box::from(b"bot".as_slice()), token: Box::from(b"fixture-token".as_slice()) },
            Identity { id: 2, login: Box::from(b"person".as_slice()), token: Box::from(b"person-token".as_slice()) },
        ]),
        repositories: Box::from([Repository {
            id: 7,
            name: Box::from(b"owner/repo".as_slice()),
            object_format: ObjectFormat::Sha1,
            has_wiki: true,
            labels: Box::from([Label { id: 10, name: Box::from(b"track".as_slice()) }]),
        }]),
        default_page: 2,
    }
}
pub fn store() -> forge::Domain {
    let settings = settings();
    let mut forge = forge::Domain::new(&settings, 1);
    forge::repository(
        &mut forge,
        &settings,
        api::Setup {
            name: Box::from(b"owner/repo".as_slice()),
            default: Box::from(b"main".as_slice()),
            tree: Box::from([api::File {
                path: Box::from(b"file".as_slice()),
                content: Box::from(b"first".as_slice()),
            }]),
            labels: Box::from([Box::from(b"track".as_slice())]),
            checks: api::Checks {
                contexts: Box::from([]),
                latency_min: Duration::ZERO,
                latency_max: Duration::ZERO,
                silent: 0,
                passes: 1000,
                reruns: 0,
                cue: None,
            },
            protection: None,
            hooked: true,
        },
    );
    forge::grant(&mut forge, b"owner/repo", 1, api::Permission::Admin);
    forge::grant(&mut forge, b"owner/repo", 2, api::Permission::Read);
    forge
}
/// Demand-aware byte driver, with queues exactly the exported public bounds.
pub struct World {
    pub connection: Connection,
    pub service: Service,
    pub forge: forge::Domain,
    pub env: Env<Limits>,
    pub upper: Queue<Event>,
    pub lower: Queue<stream::Down>,
    pub output: Vec<u8>,
    input: Vec<u8>,
    at: usize,
    pub closed: bool,
    pending: Option<(stream::Read, u32)>,
}
impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}
impl World {
    pub fn new() -> World {
        let limits = limits();
        World {
            connection: Connection::new(Token::new(9), &limits).unwrap(),
            service: Service::new(config(), &limits).unwrap(),
            forge: store(),
            env: Env {
                now: Time::from_nanos(1_700_000_000_000_000_000),
                wall: Wall::from_nanos(1_700_000_000_000_000_000),
                limits,
            },
            upper: Queue::with_capacity(connection::MAX_UP),
            lower: Queue::with_capacity(connection::MAX_DOWN),
            output: Vec::new(),
            input: Vec::new(),
            at: 0,
            closed: false,
            pending: None,
        }
    }
    pub fn wire(&mut self, request: &Request) {
        let encoded = request::encode(request, &self.env.limits.documents).unwrap();
        self.input.extend_from_slice(encoded.method.as_bytes());
        self.input.extend_from_slice(b" ");
        self.input.extend_from_slice(&encoded.target);
        self.input.extend_from_slice(b" HTTP/1.1\r\nHost: fixture\r\nAuthorization: token fixture-token\r\n");
        let body = encoded.body.as_deref().unwrap_or(b"");
        self.input.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        self.input.extend_from_slice(body);
    }
    pub fn raw(&mut self, input: &[u8]) {
        self.input.extend_from_slice(input);
    }
    pub fn drive(&mut self) {
        for _ in 0..65536 {
            if let Some(event) = self.upper.pop() {
                match event {
                    Event::Call(call) => {
                        let env = Env { now: self.env.now, wall: self.env.wall, limits: settings() };
                        let mut out = Queue::with_capacity(forge::MAX_OUT);
                        forge::step(&mut self.forge, &env, call, &mut out);
                        for _ in 0..64 {
                            if let Some(request) = out.pop() {
                                match request {
                                    forge::Request::Reply { to, result } => {
                                        let route = self.service.route(to).expect("live domain terminal");
                                        assert_eq!(route.owner, Token::new(9));
                                        self.connection.reply(
                                            &self.env,
                                            &mut self.service,
                                            &self.forge,
                                            &settings(),
                                            route.call,
                                            result,
                                        );
                                        break;
                                    }
                                    forge::Request::Hook { .. } => {}
                                }
                            } else if let Some(deadline) = self.forge.next_deadline() {
                                let env = Env { now: deadline, wall: self.env.wall, limits: settings() };
                                forge::fire(&mut self.forge, &env, &mut out);
                            } else {
                                panic!("call has a terminal");
                            }
                        }
                        self.forge.reclaim();
                        self.service.reclaim();
                    }
                    Event::Close => {
                        self.connection.closed(&mut self.service, &mut self.upper);
                        self.closed = true;
                    }
                    Event::Closed => {}
                }
                continue;
            }
            if !self.lower.is_empty() {
                for _ in 0..connection::MAX_DOWN {
                    if let Some(request) = self.lower.pop() {
                        match request {
                            stream::Down::Send(bytes) => self.output.extend_from_slice(&bytes),
                            stream::Down::Finish => panic!("HTTP owner closes explicitly"),
                            stream::Down::Demand { read, room } => {
                                if read == stream::Read::Nothing && room == 0 {
                                    self.pending = None;
                                } else {
                                    assert!(self.pending.is_none());
                                    self.pending = Some((read, room));
                                }
                            }
                        }
                    }
                }
                continue;
            }
            if self.connection.is_ready() {
                self.connection.resume(
                    &self.env,
                    &mut self.service,
                    &self.forge,
                    &settings(),
                    &mut self.upper,
                    &mut self.lower,
                );
                continue;
            }
            if let Some((read, room)) = self.pending.take() {
                if room > 0 {
                    self.connection.up(&self.env, stream::Up::Room, &mut self.lower);
                } else {
                    let rest = &self.input[self.at..];
                    let count = match read {
                        stream::Read::Fill(n) => usize::try_from(n).unwrap(),
                        stream::Read::Scan { until, max } => {
                            let cap = usize::try_from(max).unwrap().min(rest.len());
                            let delimiter = until.as_bytes();
                            match rest[..cap].windows(delimiter.len()).position(|piece| piece == delimiter) {
                                Some(at) => at + delimiter.len(),
                                None => usize::try_from(max).unwrap(),
                            }
                        }
                        stream::Read::Nothing => unreachable!(),
                        stream::Read::Line { .. } => unreachable!(),
                    };
                    if count > rest.len() {
                        self.pending = Some((read, room));
                        return;
                    }
                    let bytes = Box::from(&rest[..count]);
                    self.at += count;
                    self.connection.up(&self.env, stream::Up::Bytes(bytes), &mut self.lower);
                }
                continue;
            }

            return;
        }
        panic!("bounded HTTP work settles");
    }
}

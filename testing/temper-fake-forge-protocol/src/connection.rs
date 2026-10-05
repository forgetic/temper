//! One bounded HTTP connection. An owner retains its binding through actual
//! transport Closed; protocol completion only requests that owner close it.
use crate::{
    config::Config,
    service::Service,
    translate::{self, Dispatch},
};
use alloc::boxed::Box;
use skein_http::{self as http, server};
use skein_lib::{Env, List, Queue, ReplyTo, Token, Writer, bytes, stream};
use temper_fake_forge_domain::{self as domain, api};
use temper_forge_forgejo::{
    self as docs,
    request::{self, Operation},
    response,
    types::Document,
};

pub const MAX_UP: u32 = 1;
pub const MAX_DOWN: u32 = server::UP_MAX_OUT.below;
#[derive(Debug)]
pub enum Event {
    Call(domain::Event),
    Close,
    Closed,
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub calls: u32,
    pub http: server::Limits,
    pub documents: docs::Limits,
}
impl Limits {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.calls > 0
            && self.documents.valid()
            && server::worst_case(&self.http).is_some()
            && self.http.body <= u64::from(self.documents.document_bytes)
            && self.http.read <= self.documents.document_bytes
            && self.http.send > 0
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Reading,
    Waiting,
    Responding,
    Retiring,
    Closed,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Next,
    Read,
    Execute,
    Discard,
    Respond,
    Room,
    Send,
    Finish,
    Prepare,
    Close,
    None,
}
#[derive(Debug)]
struct Input {
    method: request::Method,
    target: Box<[u8]>,
    user: u64,
    left: Option<u64>,
}
#[derive(Debug)]
struct Pending {
    request: request::Request,
    user: u64,
    reply_to: Token,
}
#[derive(Debug)]
struct Response {
    status: u16,
    body: Box<[u8]>,
    at: usize,
    thread: Option<crate::thread::Thread>,
}
#[expect(
    missing_debug_implementations,
    reason = "HTTP scratch can contain an authorization header until intake validates it"
)]
pub struct Connection {
    owner: Token,
    server: server::Server,
    events: Queue<server::Event>,
    lower: Queue<stream::Down>,
    deferred: Option<stream::Up>,
    phase: Phase,
    action: Action,
    body: List<u8>,
    input: Option<Input>,
    pending: Option<Pending>,
    answer: Option<Response>,
    calls: u64,
}
impl Connection {
    #[must_use]
    pub fn new(owner: Token, limits: &Limits) -> Option<Connection> {
        if !limits.valid() {
            return None;
        }
        Some(Connection {
            owner,
            server: server::Server::new(&limits.http),
            events: Queue::with_capacity(server::UP_MAX_OUT.above.max(server::DOWN_MAX_OUT.above)),
            lower: Queue::with_capacity(MAX_DOWN),
            deferred: None,
            phase: Phase::Reading,
            action: Action::Next,
            body: List::with_capacity(limits.documents.document_bytes),
            input: None,
            pending: None,
            answer: None,
            calls: 0,
        })
    }
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.events.is_empty() || self.deferred.is_some() || self.action != Action::None
    }
    #[must_use]
    pub const fn calls(&self) -> u64 {
        self.calls
    }
    /// Exactly one server batch or one queued upper event per public step.
    pub fn resume(
        &mut self,
        env: &Env<Limits>,
        service: &mut Service,
        forge: &domain::Domain,
        settings: &domain::Config,
        above: &mut Queue<Event>,
        below: &mut Queue<stream::Down>,
    ) {
        if self.phase == Phase::Closed {
            return;
        }
        if let Some(event) = self.events.pop() {
            self.event(event, env, &service.config, forge, settings, above);
            return;
        }
        if let Some(input) = self.deferred.take() {
            self.up(env, input, below);
            return;
        }
        let action = self.action;
        self.action = Action::None;
        let down = match action {
            Action::Next => Some(server::Request::Next),
            Action::Read => {
                let input = self.input.as_ref().expect("reading a head");
                let count = match input.left {
                    Some(left) => u32::try_from(left.min(u64::from(env.limits.http.read))).expect("read cap").max(1),
                    None => 1,
                };
                Some(server::Request::Body(stream::Down::Demand { read: stream::Read::Fill(count), room: 0 }))
            }
            Action::Execute => {
                self.execute(env, service, forge, settings, above);
                None
            }
            Action::Discard => {
                self.action = Action::Respond;
                Some(server::Request::Discard)
            }
            Action::Respond => {
                let answer = self.answer.as_ref().expect("answer prepared");
                let body = if answer.thread.is_some() {
                    server::Body::Chunked
                } else if answer.body.is_empty() {
                    server::Body::None
                } else {
                    server::Body::Length(u64::try_from(answer.body.len()).expect("bounded body"))
                };
                self.action = if answer.body.is_empty() { Action::None } else { Action::Room };
                Some(server::Request::Respond(server::Response {
                    status: answer.status,
                    headers: Box::from([http::Header {
                        name: bytes::copy_of(b"Content-Type"),
                        value: bytes::copy_of(b"application/json;charset=utf-8"),
                    }]),
                    body,
                    close: false,
                }))
            }
            Action::Room => {
                let answer = self.answer.as_ref().expect("body");
                let count = u32::try_from(answer.body.len().saturating_sub(answer.at))
                    .unwrap_or(u32::MAX)
                    .min(env.limits.http.send);
                Some(server::Request::Reply(stream::Down::Demand { read: stream::Read::Nothing, room: count }))
            }
            Action::Send => {
                let answer = self.answer.as_mut().expect("body");
                let end = answer
                    .at
                    .saturating_add(usize::try_from(env.limits.http.send).expect("u32 fits"))
                    .min(answer.body.len());
                let piece = bytes::copy_of(answer.body.get(answer.at..end).expect("cursor"));
                answer.at = end;
                self.action = if end == answer.body.len() {
                    if answer.thread.is_some() { Action::Prepare } else { Action::Finish }
                } else {
                    Action::Room
                };
                Some(server::Request::Reply(stream::Down::Send(piece)))
            }
            Action::Prepare => {
                self.prepare(env, &service.config, forge, settings);
                None
            }
            Action::Finish => Some(server::Request::Reply(stream::Down::Finish)),
            Action::Close => Some(server::Request::Close),
            Action::None => None,
        };
        if let Some(down) = down {
            let server_env = Env { now: env.now, wall: env.wall, limits: env.limits.http };
            server::down(&mut self.server, &server_env, down, &mut self.events, &mut self.lower);
            self.flush(below);
        }
    }
    pub fn up(&mut self, env: &Env<Limits>, input: stream::Up, below: &mut Queue<stream::Down>) {
        if self.phase == Phase::Retiring || self.phase == Phase::Closed {
            return;
        }
        if !self.events.is_empty() {
            assert!(self.deferred.is_none(), "one lower demand has one answer");
            self.deferred = Some(input);
            return;
        }
        let server_env = Env { now: env.now, wall: env.wall, limits: env.limits.http };
        server::up(&mut self.server, &server_env, input, &mut self.events, &mut self.lower);
        self.flush(below);
    }
    fn flush(&mut self, below: &mut Queue<stream::Down>) {
        for _ in 0..MAX_DOWN {
            if let Some(down) = self.lower.pop() {
                below.push(down);
            } else {
                break;
            }
        }
    }
    fn respond(&mut self, status: u16, document: Document, format: docs::types::ObjectFormat, limits: &Limits) {
        let body = if document == Document::Done {
            Ok(Box::from([]))
        } else {
            response::encode(&document, format, &limits.documents)
        };
        match body {
            Ok(body) => {
                self.answer = Some(Response { status, body, at: 0, thread: None });
                self.phase = Phase::Responding;
                self.action = Action::Respond;
            }
            Err(_) => {
                self.phase = Phase::Retiring;
                self.action = Action::Close;
            }
        }
    }
    fn prepare(&mut self, env: &Env<Limits>, config: &Config, forge: &domain::Domain, settings: &domain::Config) {
        let answer = self.answer.as_mut().expect("streaming answer");
        let thread = answer.thread.as_mut().expect("streaming thread");
        match thread.next(config, forge, settings, &env.limits.documents) {
            Ok(crate::thread::Piece::Bytes(bytes)) => {
                let first = answer.body.is_empty();
                answer.body = bytes;
                answer.at = 0;
                self.action = if first { Action::Respond } else { Action::Room };
            }
            Ok(crate::thread::Piece::Advanced) => self.action = Action::Prepare,
            Ok(crate::thread::Piece::Finished) => self.action = Action::Finish,
            Err(_) => {
                self.phase = Phase::Retiring;
                self.action = Action::Close;
            }
        }
    }
    fn error(&mut self, error: api::Error, limits: &Limits) {
        let (status, message) = error_document(error);
        self.respond(
            status,
            Document::Error { message: bytes::copy_of(message) },
            docs::types::ObjectFormat::Sha1,
            limits,
        );
    }
    #[expect(clippy::manual_let_else, reason = "decode refusal clears owned request body before returning")]
    fn execute(
        &mut self,
        env: &Env<Limits>,
        service: &mut Service,
        forge: &domain::Domain,
        settings: &domain::Config,
        above: &mut Queue<Event>,
    ) {
        let input = self.input.take().expect("body finished");
        let decoded = match request::decode(
            input.method,
            &input.target,
            self.body.as_slice(),
            service.config.default_page,
            &env.limits.documents,
        ) {
            Ok(decoded) => decoded,
            Err(_) => {
                self.error(api::Error::Empty, &env.limits);
                self.body.clear();
                return;
            }
        };
        self.body.clear();
        self.calls = self.calls.saturating_add(1);
        match translate::dispatch(&decoded.request, input.user, &service.config, forge, settings, &env.limits.documents)
        {
            Ok(Dispatch::Immediate(document)) => {
                self.respond(200, document, docs::types::ObjectFormat::Sha1, &env.limits);
            }
            Ok(Dispatch::Call { repository, op }) => {
                let Some(reply_to) = service.allocate(self.owner) else {
                    self.error(api::Error::Unavailable, &env.limits);
                    return;
                };
                self.pending = Some(Pending { request: decoded.request, user: input.user, reply_to });
                self.phase = Phase::Waiting;
                above.push(Event::Call(domain::Event::Call {
                    reply_to: ReplyTo::new(reply_to),
                    user: input.user,
                    repository,
                    op,
                }));
            }
            Err(error) => self.error(error, &env.limits),
        }
    }
    #[expect(clippy::manual_let_else, reason = "authentication refusal prepares response and discards request body")]
    fn event(
        &mut self,
        event: server::Event,
        env: &Env<Limits>,
        config: &Config,
        _forge: &domain::Domain,
        _settings: &domain::Config,
        above: &mut Queue<Event>,
    ) {
        match event {
            server::Event::Call(call) => {
                let user = match config.authenticate(call.header(b"Authorization")) {
                    Some(user) => user,
                    None => {
                        self.error(api::Error::Forbidden, &env.limits);
                        self.action = Action::Discard;
                        return;
                    }
                };
                let method = match call.method {
                    http::Method::Get => request::Method::Get,
                    http::Method::Post => request::Method::Post,
                    http::Method::Patch => request::Method::Patch,
                    http::Method::Delete => request::Method::Delete,
                    http::Method::Head | http::Method::Put | http::Method::Options => {
                        self.error(api::Error::Refused, &env.limits);
                        self.action = Action::Discard;
                        return;
                    }
                };
                let left = match call.body {
                    server::Body::None => Some(0),
                    server::Body::Length(n) => Some(n),
                    server::Body::Chunked => None,
                };
                self.input = Some(Input { method, target: call.target, user, left });
                self.action = Action::Read;
            }
            server::Event::Body(stream::Up::Bytes(bytes)) => {
                if self.phase != Phase::Reading {
                    return;
                }
                let input = self.input.as_mut().expect("body");
                if let Some(left) = &mut input.left {
                    *left = left.saturating_sub(u64::try_from(bytes.len()).expect("body cap"));
                }
                for &byte in &bytes {
                    if self.body.push(byte).is_err() {
                        self.error(api::Error::TooLarge, &env.limits);
                        self.action = Action::Discard;
                        return;
                    }
                }
                self.action = Action::Read;
            }
            server::Event::Body(stream::Up::End) => {
                if self.phase == Phase::Reading {
                    self.action = Action::Execute;
                }
            }
            server::Event::Reply(stream::Up::Room) => self.action = Action::Send,
            server::Event::Body(stream::Up::Failed(_)) | server::Event::Reply(stream::Up::Failed(_)) => {}
            server::Event::Body(stream::Up::Room) | server::Event::Reply(stream::Up::Bytes(_) | stream::Up::End) => {
                unreachable!("server body direction")
            }
            server::Event::Done(server::Reuse::Keep) => {
                self.answer = None;
                self.phase = Phase::Reading;
                self.action = Action::Next;
            }
            server::Event::Done(server::Reuse::Close)
            | server::Event::Ended
            | server::Event::Failed(_)
            | server::Event::Refused(_) => {
                self.phase = Phase::Retiring;
                self.action = Action::Close;
            }
            server::Event::Closed => {
                self.phase = Phase::Retiring;
                self.action = Action::None;
                self.pending = None;
                self.input = None;
                self.answer = None;
                self.body.clear();
                above.push(Event::Close);
            }
        }
    }
    /// The domain terminal is matched to this connection's one outstanding call.
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive answer/operation table separates unpaged threads from finite documents"
    )]
    pub fn reply(
        &mut self,
        env: &Env<Limits>,
        service: &mut Service,
        forge: &domain::Domain,
        settings: &domain::Config,
        to: Token,
        result: Result<api::Answer, api::Error>,
    ) {
        if self.phase != Phase::Waiting {
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        if to != pending.reply_to {
            self.pending = Some(pending);
            return;
        }
        let config = &service.config;
        match result {
            Ok(answer) => {
                let format = match &pending.request.repository {
                    Some(repository) => {
                        let mut out =
                            Writer::new(repository.owner.len().saturating_add(1).saturating_add(repository.name.len()));
                        out.put(&repository.owner).expect("measured");
                        out.put(b"/").expect("measured");
                        out.put(&repository.name).expect("measured");
                        match config.repository(&out.finish()) {
                            Ok(repository) => repository.object_format,
                            Err(error) => {
                                self.error(error, &env.limits);
                                return;
                            }
                        }
                    }
                    None => docs::types::ObjectFormat::Sha1,
                };
                let answer = match answer {
                    api::Answer::Item { item, comments, more } => match &pending.request.operation {
                        Operation::Comments { number, since } => {
                            let repository =
                                translate::full_name(pending.request.repository.as_ref().expect("repository"));
                            match crate::thread::Thread::new(
                                repository,
                                *number,
                                *since,
                                format,
                                comments,
                                more,
                                &env.limits.documents,
                            ) {
                                Ok(thread) => {
                                    self.answer = Some(Response {
                                        status: 200,
                                        body: Box::from([]),
                                        at: 0,
                                        thread: Some(thread),
                                    });
                                    self.phase = Phase::Responding;
                                    self.action = Action::Prepare;
                                }
                                Err(error) => self.error(error, &env.limits),
                            }
                            return;
                        }
                        Operation::Settings
                        | Operation::CurrentUser
                        | Operation::SearchUser { .. }
                        | Operation::Repository
                        | Operation::Labels { .. }
                        | Operation::Items(_)
                        | Operation::Item { .. }
                        | Operation::Comment { .. }
                        | Operation::Pull { .. }
                        | Operation::PullFor { .. }
                        | Operation::Reviews { .. }
                        | Operation::Statuses { .. }
                        | Operation::Remarks { .. }
                        | Operation::Permission { .. }
                        | Operation::Branch { .. }
                        | Operation::Pages { .. }
                        | Operation::Page { .. }
                        | Operation::Dependencies { .. }
                        | Operation::CreateIssue { .. }
                        | Operation::Post { .. }
                        | Operation::EditComment { .. }
                        | Operation::AddLabels { .. }
                        | Operation::RemoveLabel { .. }
                        | Operation::OpenPull { .. }
                        | Operation::Merge { .. }
                        | Operation::Review { .. }
                        | Operation::Reviewers { .. }
                        | Operation::Dependency { .. }
                        | Operation::EditState { .. }
                        | Operation::DeleteBranch { .. }
                        | Operation::PutPage { .. }
                        | Operation::DeletePage { .. } => api::Answer::Item { item, comments, more },
                    },
                    api::Answer::Items { .. }
                    | api::Answer::Pull(_)
                    | api::Answer::Statuses(_)
                    | api::Answer::Permission(_)
                    | api::Answer::Comment { .. }
                    | api::Answer::Dependencies(_)
                    | api::Answer::Labels(_)
                    | api::Answer::Commit(_)
                    | api::Answer::Tree(_)
                    | api::Answer::File(_)
                    | api::Answer::Pages { .. }
                    | api::Answer::Page(_)
                    | api::Answer::Created(_)
                    | api::Answer::Commented(_)
                    | api::Answer::Reviewed(_)
                    | api::Answer::Merged(_)
                    | api::Answer::Revision(_)
                    | api::Answer::Done
                    | api::Answer::Cloned { .. }
                    | api::Answer::Pushed(_)
                    | api::Answer::PullFiles { .. }
                    | api::Answer::Comparison { .. }
                    | api::Answer::Checks(_)
                    | api::Answer::Protection(_)
                    | api::Answer::Settings(_)
                    | api::Answer::Collaborators(_)
                    | api::Answer::Branch(_) => answer,
                };
                match translate::project(
                    &pending.request,
                    pending.user,
                    answer,
                    config,
                    forge,
                    settings,
                    &env.limits.documents,
                ) {
                    Ok(document) => self.respond(success(&pending.request.operation), document, format, &env.limits),
                    Err(error) => self.error(error, &env.limits),
                }
            }
            Err(error) => self.error(error, &env.limits),
        }
    }
    /// Stop protocol work; the server withdraws before its owner closes io.
    pub fn close(&mut self, service: &mut Service) {
        if self.phase == Phase::Closed || self.phase == Phase::Retiring {
            return;
        }
        service.lost(self.owner);
        self.phase = Phase::Retiring;
        self.action = Action::Close;
    }
    /// Actual lower-io Closed: retire the binding only now, dropping stale scratch.
    pub fn closed(&mut self, service: &mut Service, above: &mut Queue<Event>) {
        if self.phase == Phase::Closed {
            return;
        }
        service.lost(self.owner);
        self.deferred = None;
        self.phase = Phase::Closed;
        self.action = Action::None;
        self.pending = None;
        self.input = None;
        self.answer = None;
        self.body.clear();
        for _ in 0..self.events.capacity() {
            if self.events.pop().is_none() {
                break;
            }
        }
        for _ in 0..MAX_DOWN {
            if self.lower.pop().is_none() {
                break;
            }
        }
        above.push(Event::Closed);
    }
}
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !limits.valid() {
        return None;
    }
    server::worst_case(&limits.http)?
        .checked_add(Queue::<server::Event>::worst_case(server::UP_MAX_OUT.above)?)?
        .checked_add(Queue::<stream::Down>::worst_case(MAX_DOWN)?)?
        .checked_add(Queue::<api::Comment>::worst_case(limits.documents.page)?)?
        .checked_add(List::<u8>::worst_case(limits.documents.document_bytes)?)?
        .checked_add(docs::worst_case(&limits.documents)?)?
        .checked_add(u64::from(limits.documents.document_bytes).checked_mul(4)?)?
        .checked_add(u64::from(limits.http.head))?
        .checked_add(u64::from(server::largest_read(&limits.http)))
}
fn success(operation: &Operation) -> u16 {
    match operation {
        Operation::CreateIssue { .. }
        | Operation::Post { .. }
        | Operation::OpenPull { .. }
        | Operation::PutPage { create: true, .. }
        | Operation::Reviewers { remove: false, .. } => 201,
        Operation::RemoveLabel { .. }
        | Operation::DeleteBranch { .. }
        | Operation::DeletePage { .. }
        | Operation::Reviewers { remove: true, .. } => 204,
        Operation::Settings
        | Operation::CurrentUser
        | Operation::SearchUser { .. }
        | Operation::Repository
        | Operation::Labels { .. }
        | Operation::Items(_)
        | Operation::Item { .. }
        | Operation::Comments { .. }
        | Operation::Comment { .. }
        | Operation::Pull { .. }
        | Operation::PullFor { .. }
        | Operation::Reviews { .. }
        | Operation::Statuses { .. }
        | Operation::Remarks { .. }
        | Operation::Permission { .. }
        | Operation::Branch { .. }
        | Operation::Pages { .. }
        | Operation::Page { .. }
        | Operation::Dependencies { .. }
        | Operation::EditComment { .. }
        | Operation::AddLabels { .. }
        | Operation::Merge { .. }
        | Operation::Review { .. }
        | Operation::Dependency { .. }
        | Operation::EditState { .. }
        | Operation::PutPage { create: false, .. } => 200,
    }
}
fn error_document(error: api::Error) -> (u16, &'static [u8]) {
    match error {
        api::Error::Missing(_) => (404, b"The target couldn't be found."),
        api::Error::Forbidden => (403, b"Forbidden"),
        api::Error::RateLimited { .. } => (429, b"Too many requests"),
        api::Error::TooLarge => (413, b"Content Too Large"),
        api::Error::Unavailable | api::Error::Full => (503, b"Service unavailable"),
        api::Error::Timeout => (504, b"Gateway timeout"),
        api::Error::Exists
        | api::Error::Circular
        | api::Error::NothingToMerge
        | api::Error::Empty
        | api::Error::Closed
        | api::Error::Stale
        | api::Error::Conflict
        | api::Error::Protected
        | api::Error::Unreachable
        | api::Error::Refused => (422, b"Fake forge refused this operation"),
    }
}

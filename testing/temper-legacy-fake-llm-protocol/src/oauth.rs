//! A bounded rotating OAuth issuer over the real HTTP server. Fault controls
//! belong to the byte world, independently of the refresh client's decoder.
use alloc::boxed::Box;
use core::mem::size_of;
use skein_http::{Header, Method, server as http};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Duration, Env, List, Queue, Time, bytes};
use temper_oauth as documents;

pub const MAX_UP: u32 = 3;
pub const MAX_DOWN: u32 = 18;
const EVENTS: u32 = 4;
const REQUESTS: u32 = 4;
const ROUTES: u32 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub http: http::Limits,
    pub documents: documents::Limits,
    pub plans: u32,
    /// Spent refresh-token values retained across all served connections.
    pub rotations: u32,
}
#[expect(missing_debug_implementations, reason = "issuer refresh tokens must never occur in traces")]
pub struct Config {
    pub path: Box<[u8]>,
    pub client_id: Box<[u8]>,
    pub refresh_token: Box<[u8]>,
}
#[expect(missing_debug_implementations, reason = "issuer responses contain credential values")]
pub enum Body {
    Token(documents::TokenResponse),
    Error(documents::OAuthError),
    /// Deliberately malformed documents and claim metadata are byte faults.
    Raw(Box<[u8]>),
}
#[expect(missing_debug_implementations, reason = "issuer response controls contain credential values")]
pub struct Plan {
    pub status: u16,
    pub body: Body,
    pub retry_after: Option<Box<[u8]>>,
    pub head_delay: Duration,
    pub body_delay: Duration,
}
struct Queued {
    status: u16,
    body: Box<[u8]>,
    next_refresh: Option<Box<[u8]>>,
    access: Option<(Box<[u8]>, u64)>,
    retry_after: Option<Box<[u8]>>,
    head_delay: Duration,
    body_delay: Duration,
}
struct Reply {
    status: u16,
    body: Box<[u8]>,
    retry_after: Option<Box<[u8]>>,
    head_at: Time,
    body_at: Time,
    offset: usize,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    New,
    Reading,
    Body,
    Waiting,
    Delaying,
    Sending,
    Closing,
    Closed,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// No client id or token crosses this testing control boundary.
    Requested {
        number: u64,
        accepted: bool,
    },
    Close,
    Closed,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Limits,
    Plan,
    Full,
}

#[expect(missing_debug_implementations, reason = "issuer current and response token values are secret")]
pub struct Server {
    state: State,
    http: http::Server,
    events: Queue<http::Event>,
    requests: Queue<http::Request>,
    body: List<u8>,
    reply: Option<Reply>,
    accepted_generation: Option<u64>,
    accepted: bool,
    head_sent: bool,
    body_demanded: bool,
}
#[expect(missing_debug_implementations, reason = "issuer refresh tokens persist across connection loss")]
pub struct Issuer {
    config: Config,
    plans: Queue<Queued>,
    posts: u64,
    generation: u64,
    spent: List<Box<[u8]>>,
    issued: List<Issued>,
}
struct Issued {
    token: Box<[u8]>,
    account_id: Option<Box<[u8]>>,
    expires: Time,
}
impl Issuer {
    pub fn new(config: Config, limits: &Limits) -> Result<Issuer, Error> {
        if worst_case(limits).is_none()
            || config.path.first() != Some(&b'/')
            || config.path.len() > usize::try_from(limits.http.head).expect("u32 fits usize")
        {
            return Err(Error::Limits);
        }
        for &byte in &config.path {
            if !(0x21..=0x7e).contains(&byte) {
                return Err(Error::Limits);
            }
        }
        if config.client_id.len() > usize::try_from(limits.documents.client_bytes).expect("u32 fits usize")
            || config.refresh_token.len() > usize::try_from(limits.documents.token_bytes).expect("u32 fits usize")
        {
            return Err(Error::Limits);
        }
        let request = documents::RefreshRequest {
            client_id: config.client_id.clone(),
            refresh_token: config.refresh_token.clone(),
        };
        if documents::encode_request(&request, &limits.documents).is_err() {
            return Err(Error::Limits);
        }
        Ok(Issuer {
            config,
            plans: Queue::with_capacity(limits.plans),
            posts: 0,
            generation: 0,
            spent: List::with_capacity(limits.rotations),
            issued: List::with_capacity(limits.rotations.checked_add(1).ok_or(Error::Limits)?),
        })
    }
    /// Validates/encodes one bounded response control. A successful rotation
    /// consumes the current refresh token when its request is handled, before
    /// the response is delivered. Losing the response does not undo rotation.
    pub fn queue(&mut self, plan: Plan, limits: &Limits) -> Result<(), Error> {
        if self.plans.room() == 0 {
            return Err(Error::Full);
        }
        if !(200..600).contains(&plan.status) {
            return Err(Error::Plan);
        }
        if let Some(value) = &plan.retry_after {
            if value.len() > usize::try_from(limits.documents.detail_bytes).expect("u32 fits usize") {
                return Err(Error::Plan);
            }
            for &byte in value {
                if !(0x20..=0x7e).contains(&byte) {
                    return Err(Error::Plan);
                }
            }
        }
        let (body, next_refresh, access) = match plan.body {
            Body::Token(response) => {
                let Ok(body) = documents::encode_response(&response, &limits.documents) else {
                    return Err(Error::Plan);
                };
                if (200..300).contains(&plan.status) {
                    (body, response.refresh_token, Some((response.access_token, response.expires_in)))
                } else {
                    (body, None, None)
                }
            }
            Body::Error(error) => {
                let Ok(body) = documents::encode_error(&error, &limits.documents) else {
                    return Err(Error::Plan);
                };
                (body, None, None)
            }
            Body::Raw(body) => {
                if body.len() > usize::try_from(limits.documents.document_bytes).expect("u32 fits usize") {
                    return Err(Error::Plan);
                }
                (body, None, None)
            }
        };
        self.plans.push(Queued {
            status: plan.status,
            body,
            next_refresh,
            access,
            retry_after: plan.retry_after,
            head_delay: plan.head_delay,
            body_delay: plan.body_delay,
        });
        Ok(())
    }
    #[must_use]
    pub const fn posts(&self) -> u64 {
        self.posts
    }
    #[must_use]
    pub fn authorize(&self, token: &[u8], now: Time) -> bool {
        for issued in self.issued.as_slice() {
            if issued.token.as_ref() == token && issued.expires > now {
                return true;
            }
        }
        false
    }
    /// Account metadata is read once at issuing, never in an LLM request's
    /// domain translation. Invalid JWT controls intentionally yield none.
    #[must_use]
    pub fn account_id(&self, token: &[u8], now: Time) -> Option<&[u8]> {
        for issued in self.issued.as_slice() {
            if issued.token.as_ref() == token && issued.expires > now {
                return issued.account_id.as_deref();
            }
        }
        None
    }
}
impl Server {
    pub fn new(limits: &Limits) -> Result<Server, Error> {
        if worst_case(limits).is_none() {
            return Err(Error::Limits);
        }
        Ok(Server {
            state: State::New,
            http: http::Server::new(&limits.http),
            events: Queue::with_capacity(EVENTS),
            requests: Queue::with_capacity(REQUESTS),
            body: List::with_capacity(limits.documents.document_bytes),
            reply: None,
            accepted_generation: None,
            accepted: false,
            head_sent: false,
            body_demanded: false,
        })
    }
    #[must_use]
    pub fn has_work(&self, issuer: &Issuer, now: Time) -> bool {
        if !self.events.is_empty() || !self.requests.is_empty() {
            return true;
        }
        match self.state {
            State::Waiting => !issuer.plans.is_empty() || self.accepted_generation != Some(issuer.generation),
            State::Delaying => match &self.reply {
                Some(reply) => reply.head_at <= now,
                None => false,
            },
            State::Sending => {
                !self.body_demanded
                    && match &self.reply {
                        Some(reply) => reply.body_at <= now,
                        None => false,
                    }
            }
            State::New | State::Reading | State::Body | State::Closing | State::Closed => false,
        }
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let reply = self.reply.as_ref()?;
        match self.state {
            State::Delaying => Some(reply.head_at),
            State::Sending if !self.body_demanded => Some(reply.body_at),
            State::New
            | State::Reading
            | State::Body
            | State::Waiting
            | State::Sending
            | State::Closing
            | State::Closed => None,
        }
    }
}
pub fn start(
    server: &mut Server,
    issuer: &mut Issuer,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state != State::New {
        return;
    }
    server.state = State::Reading;
    http_down(server, env, http::Request::Next, below);
    resume(server, issuer, env, above, below);
}
pub fn up(
    server: &mut Server,
    issuer: &mut Issuer,
    env: &Env<Limits>,
    event: Up,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match server.state {
        State::New | State::Closing | State::Closed => return,
        State::Reading | State::Body | State::Waiting | State::Delaying | State::Sending => {}
    }
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.http };
    http::up(&mut server.http, &child, event, &mut server.events, below);
    resume(server, issuer, env, above, below);
}
pub fn resume(
    server: &mut Server,
    issuer: &mut Issuer,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    for _route in 0..ROUTES {
        if below.room() < 2 || above.room() < 2 {
            break;
        }
        if let Some(event) = server.events.pop() {
            http_event(server, issuer, env, event, above, below);
        } else if let Some(request) = server.requests.pop() {
            http_down(server, env, request, below);
        } else {
            match server.state {
                State::Waiting => {
                    if !prepare_reply(server, issuer, env, above, below) {
                        break;
                    }
                }
                State::Delaying => {
                    let Some(reply) = &mut server.reply else {
                        break;
                    };
                    if reply.head_at > env.now {
                        break;
                    }
                    let mut headers = List::with_capacity(2);
                    headers
                        .push(Header {
                            name: bytes::copy_of(b"content-type"),
                            value: bytes::copy_of(b"application/json"),
                        })
                        .expect("two headers");
                    if let Some(value) = reply.retry_after.take() {
                        headers.push(Header { name: bytes::copy_of(b"retry-after"), value }).expect("second header");
                    }
                    server.requests.push(http::Request::Respond(http::Response {
                        status: reply.status,
                        headers: headers.into_boxed(),
                        body: http::Body::Length(u64::try_from(reply.body.len()).expect("document fits u32")),
                        close: false,
                    }));
                    server.head_sent = true;
                    server.body_demanded = false;
                    server.state = State::Sending;
                }
                State::Sending => {
                    if server.body_demanded {
                        break;
                    }
                    let Some(reply) = &server.reply else {
                        break;
                    };
                    if reply.body_at > env.now {
                        break;
                    }
                    server.body_demanded = true;
                    if reply.body.is_empty() {
                        server.requests.push(http::Request::Reply(Down::Finish));
                    } else {
                        server.requests.push(http::Request::Reply(Down::Demand {
                            read: Read::Nothing,
                            room: env.limits.http.send,
                        }));
                    }
                }
                State::New | State::Reading | State::Body | State::Closing | State::Closed => break,
            }
        }
    }
}
pub fn fire(
    server: &mut Server,
    issuer: &mut Issuer,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    resume(server, issuer, env, above, below);
}
pub fn close(server: &mut Server, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if server.state == State::Closing || server.state == State::Closed {
        return;
    }
    for _slot in 0..server.events.capacity() {
        if server.events.pop().is_none() {
            break;
        }
    }
    for _slot in 0..server.requests.capacity() {
        if server.requests.pop().is_none() {
            break;
        }
    }
    server.reply = None;
    server.body.clear();
    http_down(server, env, http::Request::Close, below);
    server.state = State::Closing;
    above.push(Event::Close);
}
pub fn closed(server: &mut Server, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if server.state == State::Closed {
        return;
    }
    if server.state != State::Closing {
        close(server, env, above, below);
    }
    server.state = State::Closed;
    above.push(Event::Closed);
}
fn http_down(server: &mut Server, env: &Env<Limits>, request: http::Request, below: &mut Queue<Down>) {
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.http };
    http::down(&mut server.http, &child, request, &mut server.events, below);
}
fn http_event(
    server: &mut Server,
    issuer: &mut Issuer,
    env: &Env<Limits>,
    event: http::Event,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state == State::Closing || server.state == State::Closed {
        return;
    }
    match event {
        http::Event::Call(call) => {
            issuer.posts = issuer.posts.saturating_add(1);
            server.accepted = call.method == Method::Post && call.target == issuer.config.path;
            let mut content = 0_u32;
            for header in &call.headers {
                if header.is(b"content-type") {
                    content = content.saturating_add(1);
                    if header.value.as_ref() != b"application/json" {
                        server.accepted = false;
                    }
                }
            }
            if content != 1 {
                server.accepted = false;
            }
            server.state = State::Body;
            server.requests.push(http::Request::Body(Down::Demand { read: Read::Fill(1), room: 0 }));
        }
        http::Event::Body(Up::Bytes(data)) => {
            if data.len() > usize::try_from(server.body.room()).expect("u32 fits usize") {
                close(server, env, above, below);
                return;
            }
            for byte in data {
                server.body.push(byte).expect("checked body cap");
            }
            server.requests.push(http::Request::Body(Down::Demand { read: Read::Fill(1), room: 0 }));
        }
        http::Event::Body(Up::End) => requested(server, issuer, env, above),
        http::Event::Reply(Up::Room) => {
            let Some(reply) = &mut server.reply else {
                close(server, env, above, below);
                return;
            };
            let end = reply
                .offset
                .saturating_add(usize::try_from(env.limits.http.send).expect("u32 fits usize"))
                .min(reply.body.len());
            let Some(piece) = reply.body.get(reply.offset..end) else {
                close(server, env, above, below);
                return;
            };
            server.requests.push(http::Request::Reply(Down::Send(bytes::copy_of(piece))));
            reply.offset = end;
            if end == reply.body.len() {
                server.requests.push(http::Request::Reply(Down::Finish));
            } else {
                server
                    .requests
                    .push(http::Request::Reply(Down::Demand { read: Read::Nothing, room: env.limits.http.send }));
            }
        }
        http::Event::Done(reuse) => {
            server.reply = None;
            server.head_sent = false;
            server.body_demanded = false;
            match reuse {
                http::Reuse::Keep => {
                    server.state = State::Reading;
                    server.requests.push(http::Request::Next);
                }
                http::Reuse::Close => close(server, env, above, below),
            }
        }
        http::Event::Ended
        | http::Event::Failed(_)
        | http::Event::Refused(_)
        | http::Event::Body(Up::Failed(_) | Up::Room)
        | http::Event::Reply(Up::Failed(_) | Up::Bytes(_) | Up::End) => close(server, env, above, below),
        http::Event::Closed => {}
    }
}
fn requested(server: &mut Server, issuer: &mut Issuer, env: &Env<Limits>, above: &mut Queue<Event>) {
    let accepted = match documents::Json::from_bytes(server.body.as_slice(), &env.limits.documents) {
        Ok(json) => match documents::decode_request(&json, &env.limits.documents) {
            Ok(request) => {
                server.accepted
                    && request.client_id == issuer.config.client_id
                    && request.refresh_token == issuer.config.refresh_token
            }
            Err(
                documents::DecodeError::Malformed
                | documents::DecodeError::Missing
                | documents::DecodeError::WrongType
                | documents::DecodeError::TooLarge
                | documents::DecodeError::Version,
            ) => false,
        },
        Err(
            documents::DecodeError::Malformed
            | documents::DecodeError::Missing
            | documents::DecodeError::WrongType
            | documents::DecodeError::TooLarge
            | documents::DecodeError::Version,
        ) => false,
    };
    server.body.clear();
    above.push(Event::Requested { number: issuer.posts, accepted });
    if accepted {
        server.accepted_generation = Some(issuer.generation);
        server.state = State::Waiting;
    } else {
        refused(server, env);
    }
}
fn refused(server: &mut Server, env: &Env<Limits>) {
    let error = documents::OAuthError {
        code: bytes::copy_of(b"invalid_grant"),
        detail: bytes::copy_of(b"credential or client refused"),
    };
    let body = match documents::encode_error(&error, &env.limits.documents) {
        Ok(body) => body,
        Err(
            documents::DecodeError::Malformed
            | documents::DecodeError::Missing
            | documents::DecodeError::WrongType
            | documents::DecodeError::TooLarge
            | documents::DecodeError::Version,
        ) => bytes::copy_of(b"{}"),
    };
    server.reply = Some(Reply { status: 400, body, retry_after: None, head_at: env.now, body_at: env.now, offset: 0 });
    server.state = State::Delaying;
}
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.http.body < u64::from(limits.documents.document_bytes) || limits.http.send == 0 || limits.http.read == 0 {
        return None;
    }
    u64::try_from(size_of::<Server>())
        .ok()?
        .checked_add(http::worst_case(&limits.http)?)?
        .checked_add(documents::worst_case(&limits.documents)?)?
        .checked_add(List::<u8>::worst_case(limits.documents.document_bytes)?)?
        .checked_add(Queue::<Queued>::worst_case(limits.plans)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(limits.rotations)?)?
        .checked_add(u64::from(limits.rotations).checked_mul(u64::from(limits.documents.token_bytes))?)?
        .checked_add(List::<Issued>::worst_case(limits.rotations.checked_add(1)?)?)?
        .checked_add(
            u64::from(limits.rotations)
                .checked_add(1)?
                .checked_mul(u64::from(limits.documents.token_bytes).checked_mul(2)?)?,
        )?
        .checked_add(
            u64::from(limits.documents.document_bytes)
                .checked_add(u64::from(limits.documents.token_bytes))?
                .checked_add(u64::from(limits.documents.detail_bytes))?
                .checked_mul(u64::from(limits.plans).checked_add(3)?)?,
        )?
        .checked_add(Queue::<http::Event>::worst_case(EVENTS)?)?
        .checked_add(Queue::<http::Request>::worst_case(REQUESTS)?)?
        .checked_add(u64::from(limits.http.head).checked_mul(2)?)
}
fn issue(issuer: &mut Issuer, credential: Issued, now: Time) -> bool {
    let mut vacancy = None;
    for index in 0..issuer.issued.len() {
        let Some(old) = issuer.issued.get(index) else {
            return false;
        };
        if old.token == credential.token || old.expires <= now {
            vacancy = Some(index);
            break;
        }
    }
    match vacancy {
        Some(index) => match issuer.issued.get_mut(index) {
            Some(slot) => {
                *slot = credential;
                true
            }
            None => false,
        },
        None => issuer.issued.push(credential).is_ok(),
    }
}

fn prepare_reply(
    server: &mut Server,
    issuer: &mut Issuer,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) -> bool {
    if server.accepted_generation != Some(issuer.generation) {
        refused(server, env);
        return true;
    }
    let Some(plan) = issuer.plans.pop() else {
        return false;
    };
    let rotation = if let Some(next) = plan.next_refresh
        && next != issuer.config.refresh_token
    {
        let mut reused = false;
        for old in issuer.spent.as_slice() {
            if *old == next {
                reused = true;
            }
        }
        if reused || issuer.spent.room() == 0 {
            close(server, env, above, below);
            return true;
        }
        let Some(generation) = issuer.generation.checked_add(1) else {
            close(server, env, above, below);
            return true;
        };
        Some((next, generation))
    } else {
        None
    };
    if let Some((token, seconds)) = plan.access {
        let (account_id, valid) = match documents::read_claims(&token, env.wall, &env.limits.documents) {
            Ok(claims) => (
                Some(claims.account_id),
                claims.valid.unwrap_or(Duration::from_secs(seconds)).min(Duration::from_secs(seconds)),
            ),
            Err(
                documents::DecodeError::Malformed
                | documents::DecodeError::Missing
                | documents::DecodeError::WrongType
                | documents::DecodeError::TooLarge
                | documents::DecodeError::Version,
            ) => (None, Duration::from_secs(seconds)),
        };
        let credential = Issued { token, account_id, expires: env.now.saturating_add(valid) };
        if !issue(issuer, credential, env.now) {
            close(server, env, above, below);
            return true;
        }
    }
    if let Some((next, generation)) = rotation {
        let old = core::mem::replace(&mut issuer.config.refresh_token, next);
        issuer.spent.push(old).expect("checked rotation capacity");
        issuer.generation = generation;
    }
    let head_at = env.now.saturating_add(plan.head_delay);
    server.reply = Some(Reply {
        status: plan.status,
        body: plan.body,
        retry_after: plan.retry_after,
        head_at,
        body_at: head_at.saturating_add(plan.body_delay),
        offset: 0,
    });
    server.state = State::Delaying;
    true
}

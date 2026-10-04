//! A single bounded OAuth HTTP attempt. The owner supplies an authenticated
//! plaintext stream and retains its socket binding until actual Closed.
use alloc::boxed::Box;
use skein_http::{Header, client as http};
use skein_lib::{
    Env, List, Queue, bytes,
    stream::{Down, Read, Up},
};
use temper_oauth::{self as document, RefreshRequest, TokenResponse};

pub const MAX_DOWN: u32 = 4;
pub const MAX_UP: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub http: http::Limits,
    pub documents: document::Limits,
}
pub enum Event {
    Completed(TokenResponse),
    Failed(document::Failure),
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Start,
    Upload,
    Send,
    Finish,
    Read,
    Waiting,
    Terminal,
    Closed,
}
pub struct Exchange {
    client: http::Client,
    events: Queue<http::Event>,
    lower: Queue<Down>,
    call: Option<http::Call>,
    upload: Box<[u8]>,
    sent: usize,
    wrote: bool,
    body: List<u8>,
    action: Action,
    status: u16,
    remaining: Option<u64>,
    retry: Box<[u8]>,
}
impl Exchange {
    pub fn new(
        request: &RefreshRequest,
        host: &[u8],
        path: &[u8],
        limits: &Limits,
    ) -> Result<Exchange, document::DecodeError> {
        if worst_case(limits).is_none() {
            return Err(document::DecodeError::TooLarge);
        }
        let upload = document::encode_request(request, &limits.documents)?;
        let mut headers = List::with_capacity(3);
        for (name, value) in [
            (b"Host".as_slice(), host),
            (b"Content-Type".as_slice(), b"application/json".as_slice()),
            (b"Accept-Encoding".as_slice(), b"identity".as_slice()),
        ] {
            headers
                .push(Header { name: bytes::copy_of(name), value: bytes::copy_of(value) })
                .expect("three request headers");
        }
        let call = http::Call {
            method: http::Method::Post,
            target: bytes::copy_of(path),
            headers: headers.into_boxed(),
            body: http::Body::Length(u64::try_from(upload.len()).expect("bounded document")),
            close: true,
        };
        Ok(Exchange {
            client: http::Client::new(&limits.http),
            events: Queue::with_capacity(2),
            lower: Queue::with_capacity(2),
            call: Some(call),
            upload,
            sent: 0,
            wrote: false,
            body: List::with_capacity(limits.documents.document_bytes),
            action: Action::Start,
            status: 0,
            remaining: None,
            retry: Box::new([]),
        })
    }
    #[must_use]
    pub fn is_ready(&self) -> bool {
        if !self.events.is_empty() {
            return true;
        }
        match self.action {
            Action::Start | Action::Upload | Action::Send | Action::Finish | Action::Read => true,
            Action::Waiting | Action::Terminal | Action::Closed => false,
        }
    }
    #[must_use]
    pub const fn started(&self) -> bool {
        self.wrote
    }
    #[must_use]
    pub fn retry_after(&self, wall: skein_lib::Wall) -> skein_lib::Duration {
        crate::oauth_time::retry_after(&self.retry, wall)
    }
}
pub fn up(exchange: &mut Exchange, env: &Env<Limits>, event: Up, below: &mut Queue<Down>) {
    if exchange.action == Action::Closed || exchange.action == Action::Terminal {
        return;
    }
    http::up(
        &mut exchange.client,
        &Env { now: env.now, wall: env.wall, limits: env.limits.http },
        event,
        &mut exchange.events,
        &mut exchange.lower,
    );
    lower(exchange, below);
}
pub fn resume(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if let Some(event) = exchange.events.pop() {
        event_up(exchange, env, event, above, below);
        return;
    }
    match exchange.action {
        Action::Start => {
            exchange.action = Action::Upload;
            let call = exchange.call.take().expect("a prepared request starts once");
            down(exchange, env, http::Request::Call(call), below);
        }
        Action::Upload => {
            exchange.action = Action::Waiting;
            let room = u32::try_from(exchange.upload.len().saturating_sub(exchange.sent))
                .expect("bounded document")
                .min(env.limits.http.send);
            down(exchange, env, http::Request::Upload(Down::Demand { read: Read::Nothing, room }), below);
        }
        Action::Send => {
            let end = exchange
                .sent
                .saturating_add(usize::try_from(env.limits.http.send).expect("u32 fits usize"))
                .min(exchange.upload.len());
            let chunk = bytes::copy_of(exchange.upload.get(exchange.sent..end).expect("bounded upload slice"));
            exchange.sent = end;
            exchange.action = if end == exchange.upload.len() { Action::Finish } else { Action::Upload };
            down(exchange, env, http::Request::Upload(Down::Send(chunk)), below);
        }
        Action::Finish => {
            exchange.upload = Box::new([]);
            exchange.action = Action::Waiting;
            down(exchange, env, http::Request::Upload(Down::Finish), below);
        }
        Action::Read => {
            exchange.action = Action::Waiting;
            // A length-framed body can be filled by bounded chunks. Unknown
            // framing reads one byte so EOF never drops a partial Fill.
            let count = match exchange.remaining {
                Some(left) if left > 0 => {
                    u32::try_from(left.min(u64::from(env.limits.http.read))).expect("bounded body read")
                }
                Some(_) | None => 1,
            };
            down(exchange, env, http::Request::Body(Down::Demand { read: Read::Fill(count), room: 0 }), below);
        }
        Action::Waiting | Action::Terminal | Action::Closed => {}
    }
}
fn event_up(
    exchange: &mut Exchange,
    env: &Env<Limits>,
    event: http::Event,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if exchange.action == Action::Terminal || exchange.action == Action::Closed {
        return;
    }
    match event {
        http::Event::Response(response) => {
            exchange.status = response.status;
            exchange.retry = bytes::copy_of(response.header(b"retry-after").unwrap_or_default());
            exchange.remaining = match response.framing {
                http::Framing::Length(length) => Some(length),
                http::Framing::Empty => Some(0),
                http::Framing::Chunked | http::Framing::UntilEnd => None,
            };
            if let Some(length) = exchange.remaining
                && length > u64::from(env.limits.documents.document_bytes)
            {
                terminal(
                    exchange,
                    env,
                    Event::Failed(document::classify(exchange.status, None, exchange.retry_after(env.wall))),
                    above,
                    below,
                );
            } else {
                exchange.action = Action::Read;
            }
        }
        http::Event::Upload(Up::Room) => exchange.action = Action::Send,
        http::Event::Upload(Up::Failed(_)) => {
            exchange.upload = Box::new([]);
        }
        http::Event::Upload(Up::Bytes(_) | Up::End) | http::Event::Body(Up::Room | Up::Failed(_)) => {
            terminal(exchange, env, Event::Failed(document::Failure::TimedOut), above, below);
        }
        http::Event::Body(Up::Bytes(data)) => {
            let fits = match u32::try_from(data.len()) {
                Ok(length) => length <= exchange.body.room(),
                Err(_) => false,
            };
            if !fits {
                terminal(
                    exchange,
                    env,
                    Event::Failed(document::classify(exchange.status, None, exchange.retry_after(env.wall))),
                    above,
                    below,
                );
                return;
            }
            for &byte in &data {
                exchange.body.push(byte).expect("checked OAuth body cap");
            }
            if let Some(left) = &mut exchange.remaining {
                *left = left.saturating_sub(u64::try_from(data.len()).expect("bounded body"));
            }
            exchange.action = Action::Read;
        }
        http::Event::Body(Up::End) => exchange.action = Action::Waiting,
        http::Event::Done(_) => {
            let json = document::Json::from_bytes(exchange.body.as_slice(), &env.limits.documents);
            let result = if exchange.status == 200 {
                match json {
                    Ok(json) => match document::decode_response(&json, &env.limits.documents) {
                        Ok(response) => Event::Completed(response),
                        Err(_) => Event::Failed(document::Failure::TimedOut),
                    },
                    Err(_) => Event::Failed(document::Failure::TimedOut),
                }
            } else {
                let error = match json {
                    Ok(json) => document::decode_error(&json, &env.limits.documents).ok(),
                    Err(_) => None,
                };
                Event::Failed(document::classify(exchange.status, error.as_ref(), exchange.retry_after(env.wall)))
            };
            terminal(exchange, env, result, above, below);
        }
        http::Event::Failed(error) => {
            let failure = match error {
                http::Error::Refused(_) | http::Error::Closed(_) => document::Failure::Unavailable,
                http::Error::Truncated { .. }
                | http::Error::Stream(_)
                | http::Error::Status
                | http::Error::Version
                | http::Error::Header
                | http::Error::HeadTooLong
                | http::Error::TooManyHeaders
                | http::Error::Framing
                | http::Error::ChunkSize
                | http::Error::Chunk
                | http::Error::Trailer
                | http::Error::Upgrade => document::Failure::TimedOut,
            };
            terminal(exchange, env, Event::Failed(failure), above, below);
        }
        http::Event::Closed => {}
    }
}
fn terminal(
    exchange: &mut Exchange,
    env: &Env<Limits>,
    event: Event,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    exchange.action = Action::Terminal;
    exchange.call = None;
    exchange.upload = Box::new([]);
    exchange.body.clear();
    clear_events(exchange);
    above.push(event);
    down(exchange, env, http::Request::Close, below);
}
pub fn close(exchange: &mut Exchange, env: &Env<Limits>, below: &mut Queue<Down>) {
    if exchange.action == Action::Closed {
        return;
    }
    let terminal = exchange.action == Action::Terminal;
    exchange.action = Action::Closed;
    exchange.call = None;
    exchange.upload = Box::new([]);
    exchange.body.clear();
    clear_events(exchange);
    if !terminal {
        down(exchange, env, http::Request::Close, below);
    }
}
fn clear_events(exchange: &mut Exchange) {
    for _event in 0..exchange.events.capacity() {
        if exchange.events.pop().is_none() {
            break;
        }
    }
}
fn down(exchange: &mut Exchange, env: &Env<Limits>, request: http::Request, below: &mut Queue<Down>) {
    http::down(
        &mut exchange.client,
        &Env { now: env.now, wall: env.wall, limits: env.limits.http },
        request,
        &mut exchange.events,
        &mut exchange.lower,
    );
    lower(exchange, below);
}
fn lower(exchange: &mut Exchange, below: &mut Queue<Down>) {
    for _effect in 0..exchange.lower.capacity() {
        let Some(effect) = exchange.lower.pop() else {
            break;
        };
        match &effect {
            Down::Send(_) => exchange.wrote = true,
            Down::Demand { .. } | Down::Finish => {}
        }
        below.push(effect);
    }
}
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.http.read == 0 || limits.http.send == 0 || limits.documents.document_bytes == 0 || limits.http.headers < 3
    {
        return None;
    }
    u64::try_from(size_of::<Exchange>())
        .ok()?
        .checked_add(http::worst_case(&limits.http)?)?
        .checked_add(document::worst_case(&limits.documents)?)?
        .checked_add(List::<u8>::worst_case(limits.documents.document_bytes)?)?
        .checked_add(u64::from(limits.documents.document_bytes).checked_mul(3)?)?
        .checked_add(u64::from(limits.http.request).checked_mul(3)?)?
        .checked_add(u64::from(limits.http.head))?
        .checked_add(Queue::<http::Event>::worst_case(2)?)?
        .checked_add(Queue::<Down>::worst_case(2)?)
}

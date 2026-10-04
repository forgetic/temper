//! One HTTP/SSE/dialect exchange over an externally owned plaintext stream
//! (llm.md, 3–8). Closing the machines never claims the socket is settled.
use crate::{Error, Limits, grants, payload, translate};
use alloc::boxed::Box;
use core::mem::size_of;
use skein_http::{Header, Method, client as http, sse};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Duration, Env, List, Queue, Time, Token, Writer, bytes};
use temper_agent_domain::{GrantName, llm};
use temper_channel::wire::{EndpointDescriptor, Provider};
use temper_llm_anthropic as anthropic;
use temper_llm_openai as openai;

/// The caller reserves these slots before every entry point. Work left in
/// the bounded routing queues is drained by `resume` before another input.
pub const MAX_UP: u32 = 4;
pub const MAX_DOWN: u32 = 18;
const HTTP_EVENTS: u32 = 4;
const SSE_EVENTS: u32 = 2;
const REQUESTS: u32 = 4;
const ROUTES: u32 = 8;

/// Identity is supplied by deployment. Historical dialect constants may be
/// selected explicitly; this layer establishes no fresh compatibility claim.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Identity {
    pub headers: Box<[Header]>,
    pub anthropic: translate::AnthropicIdentity,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    Completed {
        owner: Token,
        completion: llm::Completion,
    },
    Failed {
        owner: Token,
        failure: llm::Failure,
        evidence: Evidence,
        detail: Box<[u8]>,
    },
    /// The owner must abort the actual stream, and later call `closed`.
    Close,
    /// Only after actual lower settlement. Cancellation's terminal is here.
    Closed {
        owner: Token,
        cancelled: bool,
    },
    /// The body ended normally. The owner may retain this HTTP connection.
    Idle,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Evidence {
    /// HTTP explicitly refused, or reported Closed before sending began.
    Unsent,
    /// The peer may have seen any or all of the request. Never replay it.
    Unknown,
    /// A provider response was received.
    Response,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Prepared,
    Head,
    Streaming,
    Erring,
    Draining,
    Idle,
    Closing,
    Closed,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    Pending,
    Terminal,
    Cancelled,
}
#[derive(Debug)]
enum Decoder {
    Anthropic(anthropic::StreamDecoder),
    OpenAi(openai::StreamDecoder),
}

#[expect(missing_debug_implementations, reason = "HTTP heads retain bearer and account bytes")]
pub struct Exchange {
    owner: Token,
    state: State,
    http: http::Client,
    sse: sse::Reader,
    decoder: Decoder,
    completion: Option<translate::Completion>,
    call: Option<http::Call>,
    upload: Option<Box<[u8]>>,
    upload_offset: usize,
    http_events: Queue<http::Event>,
    sse_events: Queue<sse::Event>,
    requests: Queue<http::Request>,
    anthropic_events: Queue<anthropic::Output>,
    openai_events: Queue<openai::Output>,
    error: List<u8>,
    status: u16,
    rate: openai::RateLimit,
    absolute: Time,
    progress: Time,
    outcome: Outcome,
    close_sent: bool,
    sse_closed: bool,
    evidence: Evidence,
}

#[derive(Debug)]
pub struct Admission {
    pub owner: Token,
    pub grant: GrantName,
    pub prompt: llm::Prompt,
    pub timeout: Duration,
    /// Stable, salted conversation UUID and fresh per-attempt UUID.
    pub session: Box<[u8]>,
    pub request: Box<[u8]>,
}

impl Exchange {
    /// Fully measures the request and validates descriptors/credentials before
    /// the owner creates a connection. The grant's bytes remain protocol-only.
    pub fn prepare(
        input: Admission,
        endpoint: &EndpointDescriptor,
        table: &grants::Table,
        identity: &Identity,
        now: Time,
        limits: &Limits,
    ) -> Result<Exchange, llm::Failure> {
        if worst_case(limits).is_none() || payload::endpoints(core::slice::from_ref(endpoint), limits).is_err() {
            return Err(llm::Failure::Invalid);
        }
        if input.grant.account != endpoint.account {
            return Err(llm::Failure::Unauthorized);
        }
        let value = table.get(input.grant, now, limits.skew).ok_or(llm::Failure::Unauthorized)?;
        if !uuid(&input.session) || !uuid(&input.request) {
            return Err(llm::Failure::Invalid);
        }
        if identity_admit(identity, limits).is_err() {
            return Err(llm::Failure::Invalid);
        }
        let completion = translate::Completion::new(
            endpoint.provider.clone(),
            input.prompt.tools,
            input.prompt.served.clone(),
            limits,
        );
        let body = match endpoint.provider {
            Provider::Anthropic => {
                let Ok(request) = translate::anthropic(input.prompt, endpoint, &identity.anthropic, limits) else {
                    return Err(llm::Failure::Invalid);
                };
                match anthropic::encode_request(&request, &limits.anthropic()) {
                    Ok(body) => body,
                    Err(_) => return Err(llm::Failure::Invalid),
                }
            }
            Provider::OpenAi => {
                let Ok(request) = translate::openai(input.prompt, endpoint, &input.session, limits) else {
                    return Err(llm::Failure::Invalid);
                };
                match openai::encode_request(&request, &limits.openai()) {
                    Ok(body) => body,
                    Err(_) => return Err(llm::Failure::Invalid),
                }
            }
        };
        let Ok(headers) = headers(endpoint, value, identity, &input.session, &input.request, limits) else {
            return Err(llm::Failure::Invalid);
        };
        let call = http::Call {
            method: Method::Post,
            target: endpoint.path.clone(),
            headers,
            body: http::Body::Length(u64::try_from(body.len()).or(Err(llm::Failure::Invalid))?),
            close: false,
        };
        let decoder = match endpoint.provider {
            Provider::Anthropic => Decoder::Anthropic(anthropic::StreamDecoder::new(&event_anthropic(limits))),
            Provider::OpenAi => Decoder::OpenAi(openai::StreamDecoder::new(&event_openai(limits))),
        };
        Ok(Exchange {
            owner: input.owner,
            state: State::Prepared,
            http: http::Client::new(&http_limits(limits)),
            sse: sse::Reader::new(&sse_limits(limits)),
            decoder,
            completion: Some(completion),
            call: Some(call),
            upload: Some(body),
            upload_offset: 0,
            http_events: Queue::with_capacity(HTTP_EVENTS),
            sse_events: Queue::with_capacity(SSE_EVENTS),
            requests: Queue::with_capacity(REQUESTS),
            anthropic_events: Queue::with_capacity(anthropic::MAX_OUT),
            openai_events: Queue::with_capacity(openai::MAX_OUT),
            error: List::with_capacity(limits.error_bytes),
            status: 0,
            rate: openai::RateLimit::NONE,
            absolute: now.saturating_add(input.timeout),
            progress: now,
            outcome: Outcome::Pending,
            close_sent: false,
            sse_closed: false,
            evidence: Evidence::Unsent,
        })
    }
    #[must_use]
    pub const fn owner(&self) -> Token {
        self.owner
    }
    #[must_use]
    pub const fn absolute(&self) -> Time {
        self.absolute
    }
    #[must_use]
    pub fn has_work(&self) -> bool {
        !self.http_events.is_empty()
            || !self.sse_events.is_empty()
            || !self.requests.is_empty()
            || !self.anthropic_events.is_empty()
            || !self.openai_events.is_empty()
            || match &self.decoder {
                Decoder::OpenAi(decoder) => self.state == State::Streaming && decoder.has_ready(),
                Decoder::Anthropic(_) => false,
            }
    }
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.state == State::Idle
    }
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state == State::Closed
    }
    /// Preserves the HTTP machine and its carry-over for the next admitted
    /// call. A fresh client is never substituted over an existing binding.
    #[expect(clippy::result_large_err, reason = "refusal returns the owned attempt without allocating another owner")]
    pub fn next(&mut self, mut prepared: Exchange) -> Result<(), Exchange> {
        if self.state != State::Idle || self.has_work() || self.http.waiting() != http::Waiting::Call {
            return Err(prepared);
        }
        core::mem::swap(&mut self.http, &mut prepared.http);
        *self = prepared;
        Ok(())
    }
    #[must_use]
    pub fn deadline(&self, limits: &Limits) -> Option<Time> {
        let span = match self.state {
            State::Prepared => limits.connect,
            State::Head => limits.head,
            State::Streaming | State::Erring | State::Draining => limits.idle,
            State::Idle => limits.keep_idle,
            State::Closing | State::Closed => return None,
        };
        let local = self.progress.saturating_add(span);
        Some(if self.outcome == Outcome::Pending { local.min(self.absolute) } else { local })
    }
}

pub fn start(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if exchange.state != State::Prepared {
        return;
    }
    let Some(call) = exchange.call.take() else {
        return;
    };
    exchange.state = State::Head;
    exchange.progress = env.now;
    http_down(exchange, env, http::Request::Call(call), below);
    exchange.requests.push(http::Request::Upload(Down::Demand { read: Read::Nothing, room: env.limits.chunk }));
    resume(exchange, env, above, below);
}
pub fn up(exchange: &mut Exchange, env: &Env<Limits>, event: Up, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    match exchange.state {
        State::Prepared | State::Closing | State::Closed => return,
        State::Head | State::Streaming | State::Erring | State::Draining | State::Idle => {}
    }
    // The HTTP machine is the authority for pre-send evidence. An emitted
    // head below makes subsequent failures ambiguous even before a response.
    let child = Env { now: env.now, wall: env.wall, limits: http_limits(&env.limits) };
    let before = below.len();
    http::up(&mut exchange.http, &child, event, &mut exchange.http_events, below);
    if exchange.evidence == Evidence::Unsent {
        for down in below.iter().skip(usize::try_from(before).expect("u32 fits usize")) {
            match down {
                Down::Send(_) => exchange.evidence = Evidence::Unknown,
                Down::Demand { .. } | Down::Finish => {}
            }
        }
    }
    resume(exchange, env, above, below);
    if exchange.state == State::Idle && exchange.http.waiting() == http::Waiting::Close {
        closing(exchange, env, above, below);
    }
}
pub fn resume(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    for _route in 0..ROUTES {
        if below.room() < 2 || above.room() < 2 {
            break;
        }
        if let Some(event) = exchange.anthropic_events.pop() {
            anthropic_output(exchange, event, env, above, below);
        } else if let Some(event) = exchange.openai_events.pop() {
            openai_output(exchange, event, env, above, below);
        } else if let Some(event) = exchange.sse_events.pop() {
            sse_event(exchange, event, env, above, below);
        } else if let Some(event) = exchange.http_events.pop() {
            http_event(exchange, event, env, above, below);
        } else if let Some(request) = exchange.requests.pop() {
            http_down(exchange, env, request, below);
        } else {
            match &mut exchange.decoder {
                Decoder::OpenAi(decoder) if exchange.state == State::Streaming && decoder.has_ready() => {
                    decoder.ready(&mut exchange.openai_events);
                }
                Decoder::OpenAi(_) | Decoder::Anthropic(_) => break,
            }
        }
    }
}
pub fn cancel(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if exchange.outcome == Outcome::Terminal || exchange.state == State::Closed {
        return;
    }
    exchange.outcome = Outcome::Cancelled;
    closing(exchange, env, above, below);
}
/// A connection or security failure, including before HTTP starts. No retry.
pub fn transport_failed(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"provider transport failed"), above);
    closing(exchange, env, above, below);
}
/// An idle connection evicted by its owner, or otherwise closed after its
/// call's terminal. Closing the stack does not settle the lower binding.
pub fn close(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    closing(exchange, env, above, below);
}
pub fn fire(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    let expired = match exchange.deadline(&env.limits) {
        Some(deadline) => deadline <= env.now,
        None => false,
    };
    if expired {
        if exchange.outcome == Outcome::Pending {
            fail(exchange, llm::Failure::TimedOut, bytes::copy_of(b"provider deadline"), above);
        }
        closing(exchange, env, above, below);
    }
}
pub fn closed(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if exchange.state == State::Closed {
        return;
    }
    if exchange.outcome == Outcome::Pending {
        fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"stream closed"), above);
    }
    close_machines(exchange, env, below);
    exchange.state = State::Closed;
    above.push(Event::Closed { owner: exchange.owner, cancelled: exchange.outcome == Outcome::Cancelled });
}

fn http_down(exchange: &mut Exchange, env: &Env<Limits>, request: http::Request, below: &mut Queue<Down>) {
    let child = Env { now: env.now, wall: env.wall, limits: http_limits(&env.limits) };
    http::down(&mut exchange.http, &child, request, &mut exchange.http_events, below);
}
fn sse_down(exchange: &mut Exchange, env: &Env<Limits>, request: sse::Request) {
    let child = Env { now: env.now, wall: env.wall, limits: sse_limits(&env.limits) };
    let mut below = Queue::with_capacity(1);
    sse::down(&mut exchange.sse, &child, request, &mut exchange.sse_events, &mut below);
    if let Some(down) = below.pop() {
        exchange.requests.push(http::Request::Body(down));
    }
}
fn http_event(
    exchange: &mut Exchange,
    event: http::Event,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if exchange.state == State::Closing || exchange.state == State::Closed {
        return;
    }
    match event {
        http::Event::Response(response) => response_head(exchange, response, env, above, below),
        http::Event::Upload(Up::Room) => {
            if let Some(body) = &exchange.upload {
                let end = exchange
                    .upload_offset
                    .saturating_add(usize::try_from(env.limits.chunk).expect("u32 fits usize"))
                    .min(body.len());
                let Some(piece) = body.get(exchange.upload_offset..end) else {
                    fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"invalid upload offset"), above);
                    closing(exchange, env, above, below);
                    return;
                };
                exchange.requests.push(http::Request::Upload(Down::Send(bytes::copy_of(piece))));
                exchange.upload_offset = end;
                if end == body.len() {
                    exchange.upload = None;
                    exchange.requests.push(http::Request::Upload(Down::Finish));
                } else {
                    exchange
                        .requests
                        .push(http::Request::Upload(Down::Demand { read: Read::Nothing, room: env.limits.chunk }));
                }
            }
        }
        http::Event::Upload(Up::Failed(_)) => {
            exchange.upload = None;
        }
        http::Event::Upload(Up::Bytes(_) | Up::End) => {
            fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"invalid upload event"), above);
            closing(exchange, env, above, below);
        }
        http::Event::Body(event) => body_event(exchange, event, env, above, below),
        http::Event::Done(reuse) => {
            if exchange.outcome == Outcome::Pending {
                if exchange.state == State::Erring {
                    error_end(exchange, env, above);
                } else {
                    fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"incomplete provider stream"), above);
                }
            }
            match reuse {
                http::Reuse::Keep if exchange.state == State::Draining => {
                    exchange.state = State::Idle;
                    exchange.progress = env.now;
                    above.push(Event::Idle);
                }
                http::Reuse::Keep | http::Reuse::Close => closing(exchange, env, above, below),
            }
        }
        http::Event::Failed(error) => {
            let failure = match error {
                http::Error::Refused(_) => {
                    exchange.evidence = Evidence::Unsent;
                    llm::Failure::Invalid
                }
                http::Error::Closed(_) => {
                    exchange.evidence = Evidence::Unsent;
                    llm::Failure::Unavailable
                }
                http::Error::Stream(_)
                | http::Error::Truncated { .. }
                | http::Error::Status
                | http::Error::Version
                | http::Error::Header
                | http::Error::HeadTooLong
                | http::Error::TooManyHeaders
                | http::Error::Framing
                | http::Error::ChunkSize
                | http::Error::Chunk
                | http::Error::Trailer
                | http::Error::Upgrade => llm::Failure::Unavailable,
            };
            fail(exchange, failure, bytes::copy_of(b"HTTP exchange failed"), above);
            closing(exchange, env, above, below);
        }
        http::Event::Closed => {}
    }
}
fn response_head(
    exchange: &mut Exchange,
    response: http::Response,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    exchange.evidence = Evidence::Response;
    exchange.status = response.status;
    exchange.progress = env.now;
    for header in &response.headers {
        exchange.rate.observe(&header.name, &header.value);
    }
    let event_stream = media_type(&response.headers, b"text/event-stream");
    let encoding = single_header(&response.headers, b"content-encoding");
    let identity_encoding = match encoding {
        Ok(None) => true,
        Ok(Some(at)) => match response.headers.get(at) {
            Some(header) => identity_header(header),
            None => false,
        },
        Err(Error::Malformed | Error::TooLarge | Error::Endpoint | Error::Grant | Error::Unsupported) => false,
    };
    if !identity_encoding {
        fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"unsupported content encoding"), above);
        closing(exchange, env, above, below);
        return;
    }
    if (200..300).contains(&response.status) {
        if !event_stream {
            fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"response is not an event stream"), above);
            closing(exchange, env, above, below);
            return;
        }
        exchange.state = State::Streaming;
        sse_down(exchange, env, sse::Request::Next);
    } else {
        exchange.state = State::Erring;
        exchange.requests.push(http::Request::Body(Down::Demand { read: Read::Fill(1), room: 0 }));
    }
}
fn body_event(
    exchange: &mut Exchange,
    event: Up,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match exchange.state {
        State::Streaming => {
            // Idle is between whole events; a partial SSE line is not a ping.
            let child = Env { now: env.now, wall: env.wall, limits: sse_limits(&env.limits) };
            let mut down = Queue::with_capacity(1);
            sse::up(&mut exchange.sse, &child, event, &mut exchange.sse_events, &mut down);
            if let Some(down) = down.pop() {
                exchange.requests.push(http::Request::Body(down));
            }
        }
        State::Erring => match event {
            Up::Bytes(data) => {
                exchange.progress = env.now;
                let room = usize::try_from(exchange.error.room()).expect("u32 fits usize");
                for &byte in data.iter().take(room) {
                    exchange.error.push(byte).expect("checked error cap");
                }
                if data.len() > room || exchange.error.room() == 0 {
                    error_end(exchange, env, above);
                    closing(exchange, env, above, below);
                } else {
                    exchange.requests.push(http::Request::Body(Down::Demand { read: Read::Fill(1), room: 0 }));
                }
            }
            Up::End => error_end(exchange, env, above),
            Up::Failed(_) | Up::Room => {
                fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"error body failed"), above);
                closing(exchange, env, above, below);
            }
        },
        State::Draining | State::Idle | State::Closing | State::Closed | State::Prepared | State::Head => {}
    }
}
fn sse_event(
    exchange: &mut Exchange,
    event: sse::Event,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if exchange.state != State::Streaming {
        return;
    }
    match event {
        sse::Event::Message(message) => {
            exchange.progress = env.now;
            let result = match &mut exchange.decoder {
                Decoder::Anthropic(decoder) => {
                    let limits = event_anthropic(&env.limits);
                    match anthropic::Json::from_bytes(&message.data, &limits) {
                        Ok(json) => match anthropic::decode_event(&json, &limits) {
                            Ok(event) => {
                                decoder.event(event, &limits, env.wall, &mut exchange.anthropic_events);
                                Ok(())
                            }
                            Err(error) => Err(anthropic_invalid(error)),
                        },
                        Err(error) => Err(anthropic_invalid(error)),
                    }
                }
                Decoder::OpenAi(decoder) => {
                    let limits = event_openai(&env.limits);
                    match openai::Json::from_bytes(&message.data, &limits) {
                        Ok(json) => match openai::decode_event(&json, &limits) {
                            Ok(event) => {
                                decoder.event(event, &limits, env.wall, &mut exchange.openai_events);
                                Ok(())
                            }
                            Err(error) => Err(openai_invalid(error)),
                        },
                        Err(error) => Err(openai_invalid(error)),
                    }
                }
            };
            if result.is_err() {
                fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"malformed provider event"), above);
                closing(exchange, env, above, below);
            } else {
                sse_down(exchange, env, sse::Request::Next);
            }
        }
        sse::Event::Ended => match &mut exchange.decoder {
            Decoder::Anthropic(decoder) => decoder.end(&mut exchange.anthropic_events),
            Decoder::OpenAi(decoder) => decoder.end(&mut exchange.openai_events),
        },
        sse::Event::Failed(_) => {
            fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"event stream failed"), above);
            closing(exchange, env, above, below);
        }
        sse::Event::Closed => {}
    }
}
fn anthropic_output(
    exchange: &mut Exchange,
    event: anthropic::Output,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if exchange.state != State::Streaming {
        return;
    }
    match event {
        anthropic::Output::Part(part) => {
            if let Some(completion) = &mut exchange.completion
                && completion.anthropic(part, &env.limits).is_err()
            {
                fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"completion translation failed"), above);
                closing(exchange, env, above, below);
            }
        }
        anthropic::Output::Completed { stop, usage } => {
            completed(exchange, translate::anthropic_stop(stop), translate::anthropic_usage(usage), env, above, below);
        }
        anthropic::Output::Failed { failure, detail } => {
            fail(exchange, anthropic_failure(failure), detail, above);
            closing(exchange, env, above, below);
        }
        anthropic::Output::Progress => {}
    }
}
fn openai_output(
    exchange: &mut Exchange,
    event: openai::Output,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if exchange.state != State::Streaming {
        return;
    }
    match event {
        openai::Output::Part(part) => {
            if let Some(completion) = &mut exchange.completion
                && completion.openai(part, &env.limits).is_err()
            {
                fail(exchange, llm::Failure::Unavailable, bytes::copy_of(b"completion translation failed"), above);
                closing(exchange, env, above, below);
            }
        }
        openai::Output::Completed { stop, usage } => {
            completed(exchange, translate::openai_stop(stop), translate::openai_usage(usage), env, above, below);
        }
        openai::Output::Failed { failure, detail } => {
            fail(exchange, openai_failure(failure), detail, above);
            closing(exchange, env, above, below);
        }
        openai::Output::Progress => {}
    }
}
fn completed(
    exchange: &mut Exchange,
    stop: llm::Stop,
    usage: llm::Usage,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if exchange.outcome != Outcome::Pending {
        return;
    }
    let Some(completion) = exchange.completion.take() else {
        return;
    };
    exchange.outcome = Outcome::Terminal;
    above.push(Event::Completed { owner: exchange.owner, completion: completion.finish(stop, usage) });
    if stop == llm::Stop::MaxTokens {
        closing(exchange, env, above, below);
    } else {
        exchange.state = State::Draining;
        close_sse(exchange, env);
        clear_requests(exchange);
        exchange.requests.push(http::Request::Discard);
    }
}
fn fail(exchange: &mut Exchange, failure: llm::Failure, detail: Box<[u8]>, above: &mut Queue<Event>) {
    if exchange.outcome != Outcome::Pending {
        return;
    }
    exchange.outcome = Outcome::Terminal;
    exchange.completion = None;
    above.push(Event::Failed { owner: exchange.owner, failure, evidence: exchange.evidence, detail });
}
fn error_end(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>) {
    let (failure, detail) = match &exchange.decoder {
        Decoder::Anthropic(_) => {
            let mut limits = event_anthropic(&env.limits);
            limits.document_bytes = env.limits.error_bytes;
            let error = match anthropic::Json::from_bytes(exchange.error.as_slice(), &limits) {
                Ok(json) => anthropic::decode_error(&json, &limits).ok(),
                Err(_) => None,
            };
            let rate = anthropic::RateLimit {
                retry_after: exchange.rate.retry_after,
                reset: exchange.rate.reset,
                exhausted: exchange.rate.exhausted,
            };
            let failure = anthropic_failure(anthropic::classify(exchange.status, error.as_ref(), rate, env.wall));
            let detail = match error {
                Some(error) => error.message,
                None => bytes::copy_of(b"provider HTTP error"),
            };
            (failure, detail)
        }
        Decoder::OpenAi(_) => {
            let mut limits = event_openai(&env.limits);
            limits.document_bytes = env.limits.error_bytes;
            let error = match openai::Json::from_bytes(exchange.error.as_slice(), &limits) {
                Ok(json) => openai::decode_error(&json, &limits).ok(),
                Err(_) => None,
            };
            let failure = openai_failure(openai::classify(exchange.status, error.as_ref(), exchange.rate, env.wall));
            let detail = match error {
                Some(error) => error.message,
                None => bytes::copy_of(b"provider HTTP error"),
            };
            (failure, detail)
        }
    };
    exchange.error.clear();
    fail(exchange, failure, detail, above);
}
fn close_sse(exchange: &mut Exchange, env: &Env<Limits>) {
    if exchange.sse_closed {
        return;
    }
    for _event in 0..exchange.sse_events.capacity() {
        if exchange.sse_events.pop().is_none() {
            break;
        }
    }
    clear_requests(exchange);
    sse_down(exchange, env, sse::Request::Close);
    exchange.sse_closed = true;
}
fn close_machines(exchange: &mut Exchange, env: &Env<Limits>, below: &mut Queue<Down>) {
    if exchange.close_sent {
        return;
    }
    close_sse(exchange, env);
    clear_requests(exchange);
    for _event in 0..exchange.http_events.capacity() {
        if exchange.http_events.pop().is_none() {
            break;
        }
    }
    exchange.upload = None;
    exchange.call = None;
    exchange.error.clear();
    http_down(exchange, env, http::Request::Close, below);
    exchange.close_sent = true;
}
fn closing(exchange: &mut Exchange, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if exchange.state == State::Closing || exchange.state == State::Closed {
        return;
    }
    close_machines(exchange, env, below);
    exchange.state = State::Closing;
    above.push(Event::Close);
}

#[must_use]
pub fn http_limits(limits: &Limits) -> http::Limits {
    http::Limits {
        request: limits.head_bytes,
        head: limits.head_bytes,
        headers: limits.headers,
        read: limits.chunk,
        send: limits.chunk,
    }
}
#[must_use]
pub fn sse_limits(limits: &Limits) -> sse::Limits {
    sse::Limits { line: limits.event_bytes, event: limits.event_bytes, field: limits.name_bytes, chunk: limits.chunk }
}
fn event_openai(limits: &Limits) -> openai::Limits {
    let mut out = limits.openai();
    out.document_bytes = limits.event_bytes;
    out
}
fn event_anthropic(limits: &Limits) -> anthropic::Limits {
    let mut out = limits.anthropic();
    out.document_bytes = limits.event_bytes;
    out
}
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.chunk == 0 || limits.error_bytes == 0 || limits.headers < 8 {
        return None;
    }
    u64::try_from(size_of::<Exchange>())
        .ok()?
        .checked_add(http::worst_case(&http_limits(limits))?)?
        .checked_add(sse::worst_case(&sse_limits(limits))?)?
        .checked_add(openai::worst_case(&limits.openai())?)?
        .checked_add(anthropic::worst_case(&limits.anthropic())?)?
        .checked_add(List::<llm::Said>::worst_case(limits.parts)?)?
        .checked_add(u64::from(limits.answer_bytes).checked_mul(8)?)?
        .checked_add(u64::from(limits.head_bytes).checked_mul(4)?)?
        .checked_add(u64::from(limits.request_bytes).checked_mul(4)?)?
        .checked_add(u64::from(limits.event_bytes).checked_mul(3)?)?
        .checked_add(List::<u8>::worst_case(limits.error_bytes)?)?
        .checked_add(Queue::<http::Event>::worst_case(HTTP_EVENTS)?)?
        .checked_add(Queue::<sse::Event>::worst_case(SSE_EVENTS)?)?
        .checked_add(Queue::<http::Request>::worst_case(REQUESTS)?)?
        .checked_add(Queue::<anthropic::Output>::worst_case(anthropic::MAX_OUT)?)?
        .checked_add(Queue::<openai::Output>::worst_case(openai::MAX_OUT)?)?
        .checked_add(Queue::<Down>::worst_case(2)?)
}
fn anthropic_invalid(_error: anthropic::DecodeError) -> llm::Failure {
    llm::Failure::Invalid
}
fn openai_invalid(_error: openai::DecodeError) -> llm::Failure {
    llm::Failure::Invalid
}
fn anthropic_failure(failure: anthropic::Failure) -> llm::Failure {
    match failure {
        anthropic::Failure::Unauthorized => llm::Failure::Unauthorized,
        anthropic::Failure::Exhausted { retry_after } => llm::Failure::Exhausted { retry_after },
        anthropic::Failure::RateLimited { retry_after } => llm::Failure::RateLimited { retry_after },
        anthropic::Failure::Overloaded => llm::Failure::Overloaded,
        anthropic::Failure::Unavailable => llm::Failure::Unavailable,
        anthropic::Failure::ContextTooLong => llm::Failure::ContextTooLong,
        anthropic::Failure::Invalid => llm::Failure::Invalid,
    }
}
fn openai_failure(failure: openai::Failure) -> llm::Failure {
    match failure {
        openai::Failure::Unauthorized => llm::Failure::Unauthorized,
        openai::Failure::Exhausted { retry_after } => llm::Failure::Exhausted { retry_after },
        openai::Failure::RateLimited { retry_after } => llm::Failure::RateLimited { retry_after },
        openai::Failure::Overloaded => llm::Failure::Overloaded,
        openai::Failure::Unavailable => llm::Failure::Unavailable,
        openai::Failure::ContextTooLong => llm::Failure::ContextTooLong,
        openai::Failure::Invalid => llm::Failure::Invalid,
    }
}

fn headers(
    endpoint: &EndpointDescriptor,
    value: &grants::Value,
    identity: &Identity,
    session: &[u8],
    request: &[u8],
    limits: &Limits,
) -> Result<Box<[Header]>, Error> {
    let mut result = List::with_capacity(limits.headers);
    let mut total: usize = 0;
    for header in &identity.headers {
        if reserved(&header.name) || !header_name(&header.name) || !grants::header(&header.value) {
            return Err(Error::Malformed);
        }
        for previous in result.as_slice() {
            if Header::is(previous, &header.name) {
                return Err(Error::Malformed);
            }
        }
        result.push(header.clone()).or(Err(Error::TooLarge))?;
    }
    let port = skein_lib::Decimal::of(u64::from(endpoint.port));
    let authority_len = endpoint
        .host
        .len()
        .checked_add(1)
        .ok_or(Error::TooLarge)?
        .checked_add(port.as_bytes().len())
        .ok_or(Error::TooLarge)?;
    let mut authority = Writer::new(authority_len);
    authority.put(&endpoint.host).or(Err(Error::TooLarge))?;
    authority.put(b":").or(Err(Error::TooLarge))?;
    authority.put(port.as_bytes()).or(Err(Error::TooLarge))?;
    result.push(Header { name: bytes::copy_of(b"Host"), value: authority.finish() }).or(Err(Error::TooLarge))?;
    result
        .push(Header { name: bytes::copy_of(b"Content-Type"), value: bytes::copy_of(b"application/json") })
        .or(Err(Error::TooLarge))?;
    result
        .push(Header { name: bytes::copy_of(b"Accept-Encoding"), value: bytes::copy_of(b"identity") })
        .or(Err(Error::TooLarge))?;
    let length = value.token().len().checked_add(7).ok_or(Error::TooLarge)?;
    let mut bearer = Writer::new(length);
    bearer.put(b"Bearer ").or(Err(Error::TooLarge))?;
    bearer.put(value.token()).or(Err(Error::TooLarge))?;
    result.push(Header { name: bytes::copy_of(b"Authorization"), value: bearer.finish() }).or(Err(Error::TooLarge))?;
    let (accept, session_header, request_header) = match endpoint.provider {
        Provider::Anthropic => {
            (b"application/json".as_slice(), anthropic::identity::SESSION_HEADER, anthropic::identity::REQUEST_HEADER)
        }
        Provider::OpenAi => {
            if value.account_id().is_empty() {
                return Err(Error::Grant);
            }
            result
                .push(Header {
                    name: bytes::copy_of(openai::identity::ACCOUNT_HEADER),
                    value: bytes::copy_of(value.account_id()),
                })
                .or(Err(Error::TooLarge))?;
            (b"text/event-stream".as_slice(), openai::identity::SESSION_HEADER, openai::identity::REQUEST_HEADER)
        }
    };
    for (name, value) in [(b"Accept".as_slice(), accept), (session_header, session), (request_header, request)] {
        result.push(Header { name: bytes::copy_of(name), value: bytes::copy_of(value) }).or(Err(Error::TooLarge))?;
    }
    for header in result.as_slice() {
        total = total
            .checked_add(header.name.len())
            .ok_or(Error::TooLarge)?
            .checked_add(header.value.len())
            .ok_or(Error::TooLarge)?
            .checked_add(4)
            .ok_or(Error::TooLarge)?;
    }
    // Includes method/target/version, Content-Length and trailing blank line.
    total = total.checked_add(endpoint.path.len()).ok_or(Error::TooLarge)?.checked_add(70).ok_or(Error::TooLarge)?;
    if total > usize::try_from(limits.head_bytes).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    Ok(result.into_boxed())
}
pub(crate) fn identity_admit(identity: &Identity, limits: &Limits) -> Result<(), Error> {
    if identity.headers.len() > usize::try_from(limits.headers).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    let mut head = 0_usize;
    for (index, header) in identity.headers.iter().enumerate() {
        if reserved(&header.name) || !header_name(&header.name) || !grants::header(&header.value) {
            return Err(Error::Malformed);
        }
        for previous in identity.headers.get(..index).ok_or(Error::Malformed)? {
            if previous.is(&header.name) {
                return Err(Error::Malformed);
            }
        }
        head = head
            .checked_add(header.name.len())
            .ok_or(Error::TooLarge)?
            .checked_add(header.value.len())
            .ok_or(Error::TooLarge)?
            .checked_add(4)
            .ok_or(Error::TooLarge)?;
    }
    if head > usize::try_from(limits.head_bytes).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    if identity.anthropic.system.len() > usize::try_from(limits.parts).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    let mut system = 0_usize;
    for block in &identity.anthropic.system {
        system = system.checked_add(block.len()).ok_or(Error::TooLarge)?;
    }
    if system > usize::try_from(limits.request_bytes).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    let bounded = limits.anthropic();
    for value in [&identity.anthropic.metadata, &identity.anthropic.context_management].into_iter().flatten() {
        if value.as_tokens().len() > usize::try_from(limits.tokens).expect("u32 fits usize") {
            return Err(Error::TooLarge);
        }
        if anthropic::Json::from_tokens(value.as_tokens(), &bounded).is_err() {
            return Err(Error::Malformed);
        }
    }
    Ok(())
}
fn header_name(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    for &byte in name {
        if !(byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)) {
            return false;
        }
    }
    true
}
fn reserved(name: &[u8]) -> bool {
    for field in [
        b"host".as_slice(),
        b"authorization",
        b"content-type",
        b"accept",
        b"accept-encoding",
        b"content-length",
        b"transfer-encoding",
        b"connection",
        anthropic::identity::SESSION_HEADER,
        anthropic::identity::REQUEST_HEADER,
        openai::identity::SESSION_HEADER,
        openai::identity::REQUEST_HEADER,
        openai::identity::ACCOUNT_HEADER,
    ] {
        if name.eq_ignore_ascii_case(field) {
            return true;
        }
    }
    false
}
fn uuid(value: &[u8]) -> bool {
    if value.len() != 36 {
        return false;
    }
    for (index, byte) in value.iter().enumerate() {
        if [8, 13, 18, 23].contains(&index) {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}
fn single_header(headers: &[Header], name: &[u8]) -> Result<Option<usize>, Error> {
    let mut result = None;
    for (index, header) in headers.iter().enumerate() {
        if header.is(name) {
            if result.is_some() {
                return Err(Error::Malformed);
            }
            result = Some(index);
        }
    }
    Ok(result)
}
fn media_type(headers: &[Header], expected: &[u8]) -> bool {
    match single_header(headers, b"content-type") {
        Ok(Some(index)) => {
            let Some(header) = headers.get(index) else {
                return false;
            };
            let value = &header.value;
            let mut end = value.len();
            for (index, &byte) in value.iter().enumerate() {
                if byte == b';' {
                    end = index;
                    break;
                }
            }
            let raw = value.get(..end).unwrap_or_default();
            let mut tail = raw.len();
            for &byte in raw.iter().rev() {
                if byte == b' ' || byte == b'\t' {
                    tail = tail.saturating_sub(1);
                } else {
                    break;
                }
            }
            match raw.get(..tail) {
                Some(value) => value.eq_ignore_ascii_case(expected),
                None => false,
            }
        }
        Ok(None) | Err(Error::Malformed | Error::TooLarge | Error::Endpoint | Error::Grant | Error::Unsupported) => {
            false
        }
    }
}
fn identity_header(header: &Header) -> bool {
    header.value.eq_ignore_ascii_case(b"identity")
}
fn clear_requests(exchange: &mut Exchange) {
    for _request in 0..exchange.requests.capacity() {
        if exchange.requests.pop().is_none() {
            break;
        }
    }
}

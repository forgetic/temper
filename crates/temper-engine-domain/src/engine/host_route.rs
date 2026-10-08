//! Typed host calls cross the protocol boundary before tool decisions and after
//! their answers (jig's domain/engine.md, 7.3; domain/hosts.md, 2).
use super::{
    Call, CallAnswer, CallKey, Decision, Delivery, Domain, Env, Limits, Payload, Request, Work, emit, relay_payload,
};
use alloc::boxed::Box;
use jig_core::{SettledAnswer, SettledCall};
use jig_core_fleet as fleet;
use skein_lib::{ReplyTo, Token};

#[derive(PartialEq, Eq, Debug)]
pub enum HostRequest {
    Decode { to: ReplyTo, task: u64, attempt: u64, call: fleet::Call },
    Busy { channel: Token, task: u64, attempt: u64, name: Box<[u8]> },
    Dropped { call: fleet::Call },
    Undelivered { task: u64, attempt: u64, message: fleet::Message },
}

#[derive(PartialEq, Eq, Debug)]
pub enum HostDelivery {
    Render { to: ReplyTo, key: CallKey, name: Box<[u8]>, tool: Box<[u8]>, answer: CallAnswer },
    Answer { channel: Token, task: u64, attempt: u64, name: Box<[u8]>, call: SettledCall },
    Inbound { channel: Token, task: u64, attempt: u64, message: HostMessage },
}

/// An opaque protocol label and whole words, owned by the root while fleet routes them.
#[derive(PartialEq, Eq, Debug)]
pub struct HostMessage {
    pub name: u64,
    pub sender: Box<[u8]>,
    pub words: Box<[u8]>,
}

#[derive(Debug)]
pub(super) struct Flight {
    pub task: u64,
    pub attempt: u64,
    pub key: Option<CallKey>,
    pub name: Box<[u8]>,
    pub tool: Box<[u8]>,
}

pub(super) fn decode(domain: &mut Domain, to: ReplyTo, task: u64, attempt: u64, call: fleet::Call) {
    let right = to.into_token();
    let to = ReplyTo::new(right);
    let flight = Flight { task, attempt, key: None, name: call.name.clone(), tool: call.tool.clone() };
    assert!(domain.host_calls.insert(right, flight).is_ok(), "typed flights fit fleet calls");
    super::now(domain, Request::Host(Box::new(HostRequest::Decode { to, task, attempt, call })));
}

pub(super) fn decoded(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, to: ReplyTo, body: Call) {
    let right = to.into_token();
    let to = ReplyTo::new(right);
    let Some(flight) = domain.host_calls.get_mut(&right) else { return };
    if flight.key.is_some() {
        return;
    }
    let body = super::checked_call(body, &env.limits);
    let key =
        CallKey { task: flight.task, attempt: flight.attempt, completion: body.completion, position: body.position };
    if body.completion == 0
        || !super::current_proof(domain, key.task, key.attempt)
        || domain.core.pending_calls.contains_key(&key)
        || (!domain.core.call_parts.contains_key(&key)
            && domain.core.call_parts.len().saturating_add(domain.core.pending_calls.len()) >= env.limits.call_records)
        || (super::call_needs_input(&body.tool) && domain.result_reads.len() >= domain.result_reads.capacity())
    {
        let flight = domain.host_calls.remove(&right).expect("current typed flight");
        let id = domain
            .payloads
            .insert(Some(Payload::SettledCall(SettledCall {
                serial: 0,
                name: flight.name,
                tool: flight.tool,
                answer: SettledAnswer::Host { error: true, body: Box::from(&b"busy"[..]) },
            })))
            .expect("fleet call reserved answer room");
        domain.work.push(Work::Fleet(fleet::Event::Relayed { to, answer: id.token() }));
        return;
    }
    domain.host_calls.get_mut(&right).expect("current typed flight").key = Some(key);
    let id = domain.payloads.insert(Some(Payload::Call { key, body })).expect("fleet call reserved payload room");
    relay_payload(domain, env, decision, to, Token::new(key.task), Token::new(key.attempt), id.token());
}

pub(super) fn render(
    domain: &mut Domain,
    limits: &Limits,
    decision: &mut Decision,
    to: ReplyTo,
    answer: CallAnswer,
) -> Option<(ReplyTo, CallAnswer)> {
    let right = to.into_token();
    let to = ReplyTo::new(right);
    let Some(flight) = domain.host_calls.get(&right) else { return Some((to, answer)) };
    let key = flight.key.expect("decoded typed call");
    if let Some(call) = domain.core.call_settled.get(&key).cloned() {
        drop(domain.host_calls.remove(&right));
        settled(domain, limits, decision, to, call);
        return None;
    }
    emit(
        decision,
        limits,
        Delivery::Host(Box::new(HostDelivery::Render {
            to,
            key: flight.key.expect("decoded typed call"),
            name: flight.name.clone(),
            tool: flight.tool.clone(),
            answer,
        })),
    );
    None
}

pub(super) fn rendered(domain: &mut Domain, to: ReplyTo, answer: SettledAnswer) {
    let right = to.into_token();
    let to = ReplyTo::new(right);
    let Some(flight) = domain.host_calls.remove(&right) else { return };
    let Some(key) = flight.key else { return };
    let mut call = SettledCall { serial: 0, name: flight.name, tool: flight.tool, answer };
    let recorded = domain.core.call_parts.contains_key(&key);
    let answer_bytes = match super::call_answer(domain, key) {
        Some(answer) => crate::store::call_answer_bytes(&answer),
        None => Some(0),
    };
    let fits = call.valid(&super::core_limits(&domain.limits))
        && match answer_bytes {
            Some(answer) => match call.owned_bytes() {
                Some(bytes) => match bytes.checked_add(answer) {
                    Some(total) => total <= u64::from(domain.limits.journal.transcript_bytes),
                    None => false,
                },
                None => false,
            },
            None => false,
        };
    if !fits {
        call.answer = SettledAnswer::Host { error: true, body: Box::from(&b"busy"[..]) };
    }
    if fits && recorded {
        domain.work.push(Work::Core(jig_core::Event::SettledCall { to, key, call }));
    } else {
        let id = domain.payloads.insert(Some(Payload::SettledCall(call))).expect("fleet call reserved answer room");
        domain.work.push(Work::Fleet(fleet::Event::Relayed { to, answer: id.token() }));
    }
}

pub(super) fn settled(domain: &mut Domain, limits: &Limits, decision: &mut Decision, to: ReplyTo, call: SettledCall) {
    let id = domain.payloads.insert(Some(Payload::SettledCall(call))).expect("fleet call reserved answer room");
    emit(decision, limits, Delivery::Fleet(fleet::Event::Relayed { to, answer: id.token() }));
}

/// Assemble the answer its owner kept; the core chooses its durability route.
pub(super) fn relayed(
    domain: &mut Domain,
    channel: Token,
    run: Token,
    attempt: Token,
    name: Box<[u8]>,
    answer: Token,
) -> Delivery {
    match super::take_payload(domain, answer).expect("fleet relays an owned answer") {
        Payload::SettledCall(call) => Delivery::Host(Box::new(HostDelivery::Answer {
            channel,
            task: run.raw(),
            attempt: attempt.raw(),
            name,
            call,
        })),
        Payload::CallAnswer(answer) => {
            let call = Token::new(skein_lib::Reader::new(&name).u64().expect("decoded call name"));
            Delivery::CallAnswer { channel, task: run.raw(), attempt: attempt.raw(), call, answer }
        }
        Payload::Call { .. }
        | Payload::Message(_)
        | Payload::InboxWord(_)
        | Payload::Turn { .. }
        | Payload::Answer { .. } => unreachable!("fleet answer payload"),
    }
}

/// A read's owned value crosses the journal door before any durable decision.
pub(super) fn read(domain: &mut Domain, to: ReplyTo, answer: CallAnswer) {
    let right = to.into_token();
    let to = ReplyTo::new(right);
    match domain.host_calls.get(&right) {
        Some(flight) => {
            let request = Request::Deliver(Delivery::Host(Box::new(HostDelivery::Render {
                to,
                key: flight.key.expect("decoded read"),
                name: flight.name.clone(),
                tool: flight.tool.clone(),
                answer,
            })));
            assert!(super::now(domain, request), "read render door reserved");
        }
        None => {
            let id =
                domain.payloads.insert(Some(Payload::CallAnswer(answer))).expect("fleet read reserves payload room");
            domain.work.push(Work::Fleet(fleet::Event::Relayed { to, answer: id.token() }));
        }
    }
}

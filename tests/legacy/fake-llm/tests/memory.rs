//! The provider filled to its script and answer caps, with every call held.
use std::mem::size_of;

use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use temper_legacy_fake_llm_domain::api::{Error, Finish, Line, Message, Part, Query, Role, Script, Turn};
use temper_legacy_fake_llm_domain::{Config, Domain, Event, MAX_OUT, Request, fire, step, worst_case};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

fn config() -> Config {
    Config {
        calls: 2,
        query_bytes: 4096,
        script_bytes: 4096,
        answer_bytes: 4096,
        latency_min: Duration::from_secs(1),
        latency_max: Duration::from_secs(1),
        overloaded: 0,
        rate_limited: 0,
        retry_after: Duration::ZERO,
        unavailable: 0,
        too_long: 0,
        unauthorized: 0,
        refused: 0,
        no_calls: 0,
        answer_tokens: 1,
        calls_per_answer: 2,
        malformed: 0,
        tool_rounds: 0,
    }
}

fn scripts() -> Box<[Script]> {
    Box::new([Script {
        cue: Box::from(&b"cue"[..]),
        turns: Box::new([Turn {
            lines: Box::new([Line::Text { text: vec![b'x'; 256].into_boxed_slice() }]),
            finish: Finish::Stop,
            tokens: 1,
        }]),
    }])
}

fn query() -> Query {
    Query {
        model: Box::new([]),
        system: Box::from(&b"cue"[..]),
        tools: Box::new([]),
        messages: Box::new([Message {
            role: Role::User,
            parts: Box::new([Part::Text { text: Box::from(&b"hi"[..]) }]),
        }]),
        max_tokens: 100,
    }
}

fn event(token: u64) -> Event {
    Event::Call { reply_to: ReplyTo::new(Token::new(token)), query: query() }
}

#[test]
fn full_scripts_and_delayed_answers_stay_within_the_worst_case() {
    let mut config = config();
    config.script_bytes =
        u32::try_from(size_of::<Script>() + 3 + size_of::<Turn>() + size_of::<Line>() + 256).expect("small");
    config.answer_bytes = u32::try_from(size_of::<Part>() + 256).expect("small");
    config.query_bytes = u32::try_from(3 + size_of::<Message>() + size_of::<Part>() + 2).expect("small");
    // The output queue belongs to the harness, constructed before its meter.
    let mut out = Queue::with_capacity(MAX_OUT);
    let bound = worst_case(&config).expect("fits");
    let meter = Meter::new();
    let mut domain = Domain::try_scripted(&config, 7, scripts()).expect("scripts exactly fit");
    let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config };
    assert!(meter.held() <= bound);
    for token in 1..=3 {
        meter.start();
        step(&mut domain, &env, event(token), &mut out);
        let measured = meter.end();
        while let Some(Request::Reply { result, .. }) = out.pop() {
            assert_eq!(result, Err(Error::Overloaded), "only the full slab replies immediately");
        }
        meter.check(measured, bound, "admission");
    }
    assert_eq!(domain.calls(), config.calls, "every slot holds its maximum-sized answer");
    env.now = Time::from_nanos(1_000_000_000);
    let mut replies = 0;
    while domain.is_due(env.now) {
        meter.start();
        fire(&mut domain, &env, &mut out);
        let measured = meter.end();
        let Request::Reply { result, .. } = out.pop().expect("one answer");
        let answer = result.expect("accepted query");
        assert!(matches!(answer.parts.first(), Some(Part::Text { text }) if text.len() == 256));
        drop(answer);
        meter.check(measured, bound, "fire");
        replies += 1;
    }
    assert_eq!(replies, config.calls);
    domain.reclaim();
    assert_eq!(domain.calls(), 0);
}

#[test]
fn scripts_queries_and_answers_past_their_caps_are_refused() {
    let config = config();
    assert!(Domain::try_scripted(&Config { script_bytes: 0, ..config }, 7, scripts()).is_err());
    for config in [Config { query_bytes: 0, ..config }, Config { answer_bytes: 0, ..config }] {
        let mut domain = Domain::scripted(&config, 7, scripts());
        let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config };
        let mut out = Queue::with_capacity(MAX_OUT);
        step(&mut domain, &env, event(1), &mut out);
        env.now = Time::from_nanos(1_000_000_000);
        fire(&mut domain, &env, &mut out);
        let Request::Reply { result, .. } = out.pop().expect("one refusal");
        assert_eq!(result, Err(Error::ContextTooLong));
    }
}

#[test]
fn an_unrepresentable_memory_bound_is_refused() {
    let config = Config { calls: u32::MAX, answer_bytes: u32::MAX, ..config() };
    assert!(worst_case(&config).is_none(), "container and payload accounting must not wrap");
}

#[test]
fn random_and_truncated_tool_answers_stay_within_the_scratch_bound() {
    use temper_legacy_fake_llm_domain::api::ToolSpec;

    for scripted in [false, true] {
        let config = Config { tool_rounds: 1, answer_bytes: 2048, ..config() };
        let mut out = Queue::with_capacity(MAX_OUT);
        let bound = worst_case(&config).expect("fits");
        let meter = Meter::new();
        let scripts: Box<[Script]> = if scripted {
            Box::new([Script {
                cue: Box::from(&b"cue"[..]),
                turns: Box::new([Turn {
                    lines: (0..2)
                        .map(|_| Line::Call {
                            name: vec![b'n'; 64].into_boxed_slice(),
                            arguments: vec![b'a'; 256].into_boxed_slice(),
                        })
                        .collect(),
                    finish: Finish::ToolCalls,
                    tokens: 500,
                }]),
            }])
        } else {
            Box::new([])
        };
        let mut domain = Domain::scripted(&config, 7, scripts);
        let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config };
        for token in 1..=2 {
            meter.start();
            let mut query = query();
            query.tools = Box::new([ToolSpec {
                name: vec![b'n'; 64].into_boxed_slice(),
                description: Box::new([]),
                parameters: Box::new([]),
            }]);
            query.max_tokens = 0;
            step(&mut domain, &env, Event::Call { reply_to: ReplyTo::new(Token::new(token)), query }, &mut out);
            let measured = meter.end();
            meter.check(measured, bound, "generating full and cut tool answers");
        }
        env.now = Time::from_nanos(1_000_000_000);
        for _ in 0..2 {
            meter.start();
            fire(&mut domain, &env, &mut out);
            let measured = meter.end();
            let Request::Reply { result, .. } = out.pop().expect("one reply");
            let answer = result.expect("accepted query");
            assert_eq!(answer.finish, Finish::Length);
            drop(answer);
            meter.check(measured, bound, "sending truncated tool answer");
        }
        domain.reclaim();
        assert_eq!(domain.calls(), 0);
    }
}

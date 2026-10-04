#![expect(clippy::match_like_matches_macro, reason = "workspace bans matches so tests use explicit match assertions")]
#![expect(clippy::wildcard_enum_match_arm, reason = "ordinary tests make focused partial-variant assertions")]
//! Synthetic scenarios and separately identified archived provider traffic.
#![expect(clippy::disallowed_types, reason = "ordinary test code collects provider outputs in Vec")]
#![expect(clippy::disallowed_methods, reason = "tests inspect fixture text and collect outputs")]
#![expect(clippy::arithmetic_side_effects, reason = "the test's trusted archive cursor uses ordinary arithmetic")]
use crate::{
    Block, DecodeError, Delta, Event, Failure, Json, Limits, Message, Output, Part, ProviderError, RateLimit, Request,
    Role, Stop, StreamDecoder, Tool, Usage, classify, decode_event, decode_request, encode_event, encode_request,
    identity, json, worst_case,
};
use alloc::boxed::Box;
use alloc::vec::Vec;
use skein_json::Token;
use skein_lib::{Duration, Queue, Wall, bytes};

const LIMITS: Limits = Limits {
    request_bytes: 65536,
    document_bytes: 65536,
    string_bytes: 16384,
    depth: 32,
    tokens: 4096,
    parts: 32,
    input_bytes: 8192,
    opaque_bytes: 8192,
    answer_bytes: 32768,
    detail_bytes: 256,
};
fn owned(b: &[u8]) -> Box<[u8]> {
    bytes::copy_of(b)
}
fn value(b: &[u8]) -> Json {
    Json::from_bytes(b, &LIMITS).unwrap()
}
fn request() -> Request {
    Request {
        model: owned(b"claude-test"),
        system: Box::new([owned(identity::SYSTEM), owned(b"line\n\"quoted\"")]),
        tools: Box::new([Tool {
            name: owned(b"Read"),
            description: owned(b"Read a file"),
            schema: value(br#"{"type":"object","properties":{"path":{"type":"string"}}}"#),
        }]),
        messages: Box::new([Message { role: Role::User, content: Box::new([Block::Text { text: owned(b"hello") }]) }]),
        max_tokens: 200,
        thinking_budget: Some(100),
        metadata: Some(value(br#"{"user_id":"masked"}"#)),
        context_management: Some(value(br#"{"edits":[]}"#)),
    }
}
fn drain(queue: &mut Queue<Output>, trace: &mut Vec<Output>) {
    while let Some(output) = queue.pop() {
        trace.push(output);
    }
}
fn stream(events: &[Event], limits: &Limits) -> Vec<Output> {
    let mut decoder = StreamDecoder::new(limits);
    let mut out = Queue::with_capacity(crate::MAX_OUT);
    let mut trace = Vec::new();
    for event in events {
        decoder.event(event.clone(), limits, Wall::EPOCH, &mut out);
        drain(&mut out, &mut trace);
    }
    decoder.end(&mut out);
    drain(&mut out, &mut trace);
    trace
}
fn fixture(scenario: &str, file: &str) -> Vec<u8> {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    std::fs::read(std::path::Path::new(&root).join("tests/fixtures").join(scenario).join(file)).unwrap()
}
fn dechunk(input: &[u8]) -> Vec<u8> {
    let mut at: usize = 0;
    let mut out = Vec::new();
    while at < input.len() {
        let end = bytes::find(&input[at..], b"\r\n").unwrap();
        let size = usize::from_str_radix(core::str::from_utf8(&input[at..at + end]).unwrap(), 16).unwrap();
        at += end + 2;
        if size == 0 {
            assert_eq!(&input[at..], b"\r\n");
            return out;
        }
        out.extend_from_slice(&input[at..at + size]);
        at += size;
        assert_eq!(&input[at..at + 2], b"\r\n");
        at += 2;
    }
    panic!("archive misses terminal chunk")
}

#[test]
fn measured_request_roundtrips_and_marks_only_four_tail_positions() {
    let request = request();
    let body = encode_request(&request, &LIMITS).unwrap();
    let document = Json::from_bytes(&body, &LIMITS).unwrap();
    assert_eq!(decode_request(&document, &LIMITS).unwrap(), request);
    assert_eq!(body.as_ref(), document.to_bytes(&LIMITS).unwrap().as_ref());
    assert_eq!(bytes::count(&body, b"cache_control", 10), 3);
    let tight = Limits { request_bytes: u32::try_from(body.len()).unwrap() - 1, ..LIMITS };
    assert_eq!(encode_request(&request, &tight), Err(DecodeError::TooLarge));
    let mut invalid = request.clone();
    invalid.messages[0].role = Role::Assistant;
    invalid.messages[0].content =
        Box::new([Block::ToolResult { id: owned(b"tool"), content: owned(b"result"), error: false }]);
    assert_eq!(encode_request(&invalid, &LIMITS), Err(DecodeError::WrongType));
    invalid = request.clone();
    invalid.model = owned(&[0xff]);
    assert_eq!(encode_request(&invalid, &LIMITS), Err(DecodeError::Malformed));
}

#[test]
fn opaque_thinking_and_unknown_blocks_are_replayed_with_all_fields() {
    let events = [
        Event::MessageStart { usage: Usage::ZERO },
        Event::BlockStart {
            index: 0,
            block: Block::Opaque { value: value(br#"{"type":"thinking","thinking":"","signature":""}"#) },
        },
        Event::BlockDelta { index: 0, delta: Delta::Thinking { text: owned(b"thought") } },
        Event::BlockDelta { index: 0, delta: Delta::Signature { text: owned(b"signed") } },
        Event::BlockStop { index: 0 },
        Event::BlockStart {
            index: 1,
            block: Block::Opaque { value: value(br#"{"type":"future_block","proof":{"a":[1,2]},"data":"opaque"}"#) },
        },
        Event::BlockStop { index: 1 },
        Event::MessageDelta { stop: Stop::EndTurn, usage: Usage::ZERO },
        Event::MessageStop,
    ];
    let trace = stream(&events, &LIMITS);
    let mut opaque = Vec::new();
    for output in trace {
        if let Output::Part(Part::Opaque { bytes }) = output {
            opaque.push(bytes);
        }
    }
    assert_eq!(opaque.len(), 2);
    assert_eq!(opaque[0].as_ref(), br#"{"type":"thinking","thinking":"thought","signature":"signed"}"#);
    assert_eq!(opaque[1].as_ref(), br#"{"type":"future_block","proof":{"a":[1,2]},"data":"opaque"}"#);
    let mut request = request();
    request.messages = Box::new([Message {
        role: Role::Assistant,
        content: Box::new([
            Block::Opaque { value: Json::from_bytes(&opaque[0], &LIMITS).unwrap() },
            Block::Opaque { value: Json::from_bytes(&opaque[1], &LIMITS).unwrap() },
        ]),
    }]);
    assert_eq!(
        decode_request(&Json::from_bytes(&encode_request(&request, &LIMITS).unwrap(), &LIMITS).unwrap(), &LIMITS)
            .unwrap(),
        request
    );
}

#[test]
fn input_cap_discards_only_input_and_answer_cap_cuts_the_open_tool() {
    let events = [
        Event::MessageStart { usage: Usage::ZERO },
        Event::BlockStart {
            index: 0,
            block: Block::ToolUse { id: owned(b"id"), name: owned(b"Read"), input: value(b"{}") },
        },
        Event::BlockDelta { index: 0, delta: Delta::Input { text: owned(br#"{"path":"too long"}"#) } },
        Event::BlockStop { index: 0 },
        Event::MessageDelta { stop: Stop::EndTurn, usage: Usage::ZERO },
        Event::MessageStop,
    ];
    let small_input = Limits { input_bytes: 4, ..LIMITS };
    let trace = stream(&events, &small_input);
    assert!(trace.iter().any(|out| match out {
        Output::Part(Part::ToolCall { input, too_large: true, .. }) if input.is_empty() => true,
        _ => false,
    }));
    assert!(match trace.last() {
        Some(Output::Completed { stop: Stop::ToolUse, .. }) => true,
        _ => false,
    });
    let small_answer = Limits { answer_bytes: 10, ..LIMITS };
    let trace = stream(&events, &small_answer);
    assert!(trace.iter().any(|out| match out {
        Output::Part(Part::ToolCall { input, too_large: false, .. }) if input.as_ref() == b"{\"pa" => true,
        _ => false,
    }));
    assert!(match trace.last() {
        Some(Output::Completed { stop: Stop::MaxTokens, .. }) => true,
        _ => false,
    });
}

#[test]
fn mutation_of_order_kind_index_or_terminal_fails_exactly_once() {
    let start = Event::MessageStart { usage: Usage::ZERO };
    let text = Event::BlockStart { index: 0, block: Block::Text { text: owned(b"") } };
    let scenarios = [
        Vec::from([start.clone(), start.clone()]),
        Vec::from([text.clone()]),
        Vec::from([start.clone(), Event::BlockStart { index: 1, block: Block::Text { text: owned(b"") } }]),
        Vec::from([
            start.clone(),
            text.clone(),
            Event::BlockDelta { index: 0, delta: Delta::Input { text: owned(b"{}") } },
        ]),
        Vec::from([start.clone(), text, Event::BlockStop { index: 1 }]),
        Vec::from([start, Event::MessageStop]),
    ];
    for events in scenarios {
        let trace = stream(&events, &LIMITS);
        assert_eq!(
            trace
                .iter()
                .filter(|output| match output {
                    Output::Failed { .. } => true,
                    _ => false,
                })
                .count(),
            1
        );
        assert_eq!(
            trace
                .iter()
                .filter(|output| match output {
                    Output::Completed { .. } => true,
                    _ => false,
                })
                .count(),
            0
        );
    }
}

#[test]
fn server_events_roundtrip_and_errors_classify_status_and_resets() {
    let error = ProviderError {
        kind: owned(b"overloaded_error"),
        message: owned(b"busy"),
        resets_at: None,
        resets_in_seconds: None,
    };
    let events = [
        Event::Ping,
        Event::MessageStart {
            usage: Usage { input_tokens: 5, output_tokens: 2, cache_read_tokens: 3, cache_write_tokens: 4 },
        },
        Event::BlockStart { index: 0, block: Block::Text { text: owned(b"hi\n") } },
        Event::BlockDelta { index: 0, delta: Delta::Text { text: owned(b"more") } },
        Event::BlockStop { index: 0 },
        Event::MessageDelta { stop: Stop::MaxTokens, usage: Usage::ZERO },
        Event::MessageStop,
        Event::Error { error: error.clone() },
        Event::Unknown,
    ];
    for event in events {
        let bytes = encode_event(&event, &LIMITS).unwrap();
        assert_eq!(decode_event(&Json::from_bytes(&bytes, &LIMITS).unwrap(), &LIMITS).unwrap(), event);
    }
    let mut rate = RateLimit::NONE;
    rate.observe(b"Anthropic-Ratelimit-Unified-Status", b"rejected");
    rate.observe(b"anthropic-ratelimit-unified-reset", b"120");
    let wall = Wall::from_nanos(100_000_000_000);
    assert_eq!(classify(429, None, rate, wall), Failure::Exhausted { retry_after: Duration::from_secs(20) });
    rate.observe(b"retry-after", b"3");
    assert_eq!(classify(429, None, rate, wall), Failure::Exhausted { retry_after: Duration::from_secs(3) });
    assert_eq!(classify(403, Some(&error), rate, wall), Failure::Invalid);
    assert_eq!(classify(401, None, rate, wall), Failure::Unauthorized);
    assert_eq!(classify(200, Some(&error), RateLimit::NONE, wall), Failure::Overloaded);
}

#[test]
fn malformed_json_duplicates_and_every_document_limit_are_refused() {
    for bytes in [b"{]".as_slice(), b"[1,]", b"{}{}", b"\"\\ud800\"", b"{\"a\":}"] {
        assert_eq!(Json::from_bytes(bytes, &LIMITS).unwrap_err(), DecodeError::Malformed);
    }
    let tokens = [Token::ObjectStart, Token::Key(owned(b"a")), Token::ObjectEnd];
    assert_eq!(Json::from_tokens(&tokens, &LIMITS), Err(DecodeError::Malformed));
    assert_eq!(Json::from_bytes(b"{}", &Limits { tokens: 1, ..LIMITS }), Err(DecodeError::TooLarge));
    assert_eq!(
        Json::from_bytes(br#"{"deep":[{}]}"#, &Limits { depth: 2, ..LIMITS }).unwrap_err(),
        DecodeError::TooLarge
    );
    assert_eq!(
        Json::from_bytes(b"\"long\"", &Limits { string_bytes: 3, ..LIMITS }).unwrap_err(),
        DecodeError::TooLarge
    );
    assert_eq!(decode_event(&value(br#"{"type":"ping","type":"message_stop"}"#), &LIMITS), Err(DecodeError::Malformed));
    assert!(worst_case(&LIMITS).unwrap() > u64::from(LIMITS.answer_bytes));
    assert_eq!(worst_case(&Limits { parts: u32::MAX, ..LIMITS }), None);
}

#[test]
fn archived_real_provider_requests_and_answers_match_known_completions() {
    let scenarios = [
        ("single-text", 39_u64, 4_u64, 0_usize),
        ("tool-call", 603, 53, 1),
        ("parallel-tool-calls", 617, 89, 2),
        ("tool-result-final", 679, 15, 0),
    ];
    for (scenario, input, output, tool_count) in scenarios {
        let wrapper = Json::from_bytes(&fixture(scenario, "request.json"), &LIMITS).unwrap();
        let body = json::text_ref(
            json::value_at(wrapper.as_tokens(), json::required(wrapper.as_tokens(), b"body").unwrap()).unwrap(),
        )
        .unwrap();
        let request = decode_request(&Json::from_bytes(body, &LIMITS).unwrap(), &LIMITS).unwrap();
        assert!(!request.messages.is_empty());
        let captured = dechunk(&fixture(scenario, "response.sse"));
        let mut decoder = StreamDecoder::new(&LIMITS);
        let mut out = Queue::with_capacity(crate::MAX_OUT);
        let mut trace = Vec::new();
        for line in core::str::from_utf8(&captured).unwrap().lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                let event = decode_event(&value(data.as_bytes()), &LIMITS).unwrap();
                decoder.event(event, &LIMITS, Wall::EPOCH, &mut out);
                drain(&mut out, &mut trace);
            }
        }
        decoder.end(&mut out);
        drain(&mut out, &mut trace);
        let calls: Vec<&Part> = trace
            .iter()
            .filter_map(|output| match output {
                Output::Part(part @ Part::ToolCall { .. }) => Some(part),
                Output::Part(Part::Text { .. } | Part::Opaque { .. })
                | Output::Completed { .. }
                | Output::Failed { .. }
                | Output::Progress => None,
            })
            .collect();
        assert_eq!(calls.len(), tool_count);
        for call in calls {
            match call {
                Part::ToolCall { name, input, too_large, .. } => {
                    assert_eq!(name.as_ref(), b"get_weather");
                    assert!(!too_large);
                    assert!(bytes::find(input, b"Paris").is_some() || bytes::find(input, b"London").is_some());
                }
                Part::Text { .. } | Part::Opaque { .. } => unreachable!(),
            }
        }
        assert_eq!(
            trace.last(),
            Some(&Output::Completed {
                stop: if tool_count == 0 { Stop::EndTurn } else { Stop::ToolUse },
                usage: Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0
                }
            })
        );
        if scenario == "single-text" {
            assert!(trace.iter().any(|output| match output {
                Output::Part(Part::Text { text }) if text.as_ref() == b"hello" => true,
                _ => false,
            }));
        }
    }
}

#[test]
fn thinking_start_extensions_survive_and_server_rejects_oversize_request() {
    let events = [
        Event::MessageStart { usage: Usage::ZERO },
        Event::BlockStart {
            index: 0,
            block: Block::Opaque {
                value: value(br#"{"type":"thinking","thinking":"","signature":"","provider_hint":{"signed":true}}"#),
            },
        },
        Event::BlockDelta { index: 0, delta: Delta::Signature { text: owned(b"proof") } },
        Event::BlockStop { index: 0 },
        Event::MessageDelta { stop: Stop::EndTurn, usage: Usage::ZERO },
        Event::MessageStop,
    ];
    let trace = stream(&events, &LIMITS);
    assert!(trace.iter().any(|output| match output {
        Output::Part(Part::Opaque { bytes }) =>
            bytes.as_ref()
                == br#"{"type":"thinking","provider_hint":{"signed":true},"thinking":"","signature":"proof"}"#,
        _ => false,
    }));
    let body = encode_request(&request(), &LIMITS).unwrap();
    let document = value(&body);
    assert_eq!(
        decode_request(&document, &Limits { request_bytes: u32::try_from(body.len()).unwrap() - 1, ..LIMITS }),
        Err(DecodeError::TooLarge)
    );
}

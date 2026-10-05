#![expect(clippy::match_like_matches_macro, reason = "workspace bans matches so tests use explicit match assertions")]
#![expect(clippy::wildcard_enum_match_arm, reason = "ordinary tests make focused partial-variant assertions")]
//! Synthetic scenarios and separately identified archived provider traffic.
#![expect(clippy::disallowed_types, reason = "ordinary test code collects provider outputs in Vec")]
#![expect(clippy::disallowed_methods, reason = "tests inspect fixture text and collect outputs")]
#![expect(clippy::arithmetic_side_effects, reason = "the test's trusted archive cursor uses ordinary arithmetic")]
use crate::{
    DecodeError, Event, Failure, Input, Item, Json, Limits, Output, Part, ProviderError, RateLimit, Request, Role,
    Stop, StreamDecoder, Tool, Usage, classify, decode_error, decode_event, decode_request, encode_error, encode_event,
    encode_request, json, worst_case,
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
        model: owned(b"gpt-test"),
        instructions: owned(b"line\n\"quoted\""),
        tools: Box::new([Tool {
            name: owned(b"read"),
            description: owned(b"Read a file"),
            schema: value(br#"{"type":"object","properties":{"path":{"type":"string"}}}"#),
        }]),
        input: Box::new([
            Input::Message { role: Role::User, text: owned(b"hello"), id: None, phase: None },
            Input::Opaque {
                value: value(br#"{"type":"reasoning","id":"r","encrypted_content":"ciphertext","summary":[]}"#),
            },
            Input::Message {
                role: Role::Assistant,
                text: owned(b"reply"),
                id: Some(owned(b"message")),
                phase: Some(owned(b"commentary")),
            },
            Input::FunctionCall {
                call_id: owned(b"call"),
                item_id: Some(owned(b"item")),
                name: owned(b"read"),
                arguments: owned(br#"{"path":"a"}"#),
            },
            Input::FunctionOutput { call_id: owned(b"call"), output: owned(b"ok") },
        ]),
        effort: Some(owned(b"high")),
        prompt_cache_key: Some(owned(b"conversation")),
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
        while decoder.has_ready() {
            decoder.ready(&mut out);
            drain(&mut out, &mut trace);
        }
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
fn measured_request_replays_all_input_kinds_and_rejects_bad_arguments() {
    let request = request();
    let body = encode_request(&request, &LIMITS).unwrap();
    let document = Json::from_bytes(&body, &LIMITS).unwrap();
    assert_eq!(decode_request(&document, &LIMITS).unwrap(), request);
    assert_eq!(body.as_ref(), document.to_bytes(&LIMITS).unwrap().as_ref());
    assert!(bytes::find(&body, b"max_output_tokens").is_none());
    assert_eq!(
        encode_request(&request, &Limits { request_bytes: u32::try_from(body.len()).unwrap() - 1, ..LIMITS }),
        Err(DecodeError::TooLarge)
    );
    let mut invalid = request.clone();
    invalid.input = Box::new([Input::FunctionCall {
        call_id: owned(b"call"),
        item_id: Some(owned(b"item")),
        name: owned(b"read"),
        arguments: owned(b"[]"),
    }]);
    assert_eq!(encode_request(&invalid, &LIMITS), Err(DecodeError::WrongType));
    invalid = request;
    invalid.model = owned(&[0xff]);
    assert_eq!(encode_request(&invalid, &LIMITS), Err(DecodeError::Malformed));
}

#[test]
fn synthetic_parallel_tools_reasoning_and_message_heads_keep_order() {
    let reasoning = value(br#"{"type":"reasoning","id":"r","encrypted_content":"opaque","summary":[]}"#);
    let events = [
        Event::Created { echo: Some(request()) },
        Event::Added { index: 0, id: owned(b"r"), kind: owned(b"reasoning") },
        Event::Done { index: 0, item: Item::Opaque { value: reasoning.clone() } },
        Event::Added { index: 1, id: owned(b"m"), kind: owned(b"message") },
        Event::Done {
            index: 1,
            item: Item::Message {
                id: owned(b"m"),
                phase: Some(owned(b"commentary")),
                text: owned(b"hello"),
                refusal: false,
            },
        },
        Event::Added { index: 2, id: owned(b"f1"), kind: owned(b"function_call") },
        Event::Added { index: 3, id: owned(b"f2"), kind: owned(b"function_call") },
        Event::Done {
            index: 2,
            item: Item::FunctionCall {
                id: owned(b"f1"),
                call_id: owned(b"c1"),
                name: owned(b"read"),
                arguments: owned(br#"{"path":"a"}"#),
            },
        },
        Event::Done {
            index: 3,
            item: Item::FunctionCall {
                id: owned(b"f2"),
                call_id: owned(b"c2"),
                name: owned(b"read"),
                arguments: owned(br#"{"path":"b"}"#),
            },
        },
        Event::Completed { stop: Stop::EndTurn, usage: Usage::ZERO },
    ];
    let trace = stream(&events, &LIMITS);
    let parts: Vec<&Part> = trace
        .iter()
        .filter_map(|output| match output {
            Output::Part(part) => Some(part),
            Output::Completed { .. } | Output::Failed { .. } | Output::Progress => None,
        })
        .collect();
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[0], &Part::Opaque { bytes: reasoning.to_bytes(&LIMITS).unwrap() });
    assert_eq!(parts[1], &Part::Opaque { bytes: owned(br#"{"id":"m","phase":"commentary"}"#) });
    assert_eq!(parts[2], &Part::Text { text: owned(b"hello") });
    assert!(match parts[3] {
        Part::ToolCall { id, .. } if id.as_ref() == b"c1|f1" => true,
        _ => false,
    });
    assert!(match parts[4] {
        Part::ToolCall { id, .. } if id.as_ref() == b"c2|f2" => true,
        _ => false,
    });
    assert!(match trace.last() {
        Some(Output::Completed { stop: Stop::ToolUse, .. }) => true,
        _ => false,
    });
}

#[test]
fn input_cap_discards_input_and_part_limit_emits_one_cut_terminal() {
    let events = [
        Event::Added { index: 0, id: owned(b"f"), kind: owned(b"function_call") },
        Event::Done {
            index: 0,
            item: Item::FunctionCall {
                id: owned(b"f"),
                call_id: owned(b"c"),
                name: owned(b"read"),
                arguments: owned(br#"{"path":"a"}"#),
            },
        },
        Event::Completed { stop: Stop::EndTurn, usage: Usage::ZERO },
    ];
    let trace = stream(&events, &Limits { input_bytes: 3, ..LIMITS });
    assert!(trace.iter().any(|out| match out {
        Output::Part(Part::ToolCall { input, too_large: true, .. }) if input.is_empty() => true,
        _ => false,
    }));
    assert!(match trace.last() {
        Some(Output::Completed { stop: Stop::ToolUse, .. }) => true,
        _ => false,
    });
    let events = [
        Event::Added { index: 0, id: owned(b"m"), kind: owned(b"message") },
        Event::Done {
            index: 0,
            item: Item::Message { id: owned(b"m"), phase: None, text: owned(b"hello"), refusal: false },
        },
        Event::Completed { stop: Stop::EndTurn, usage: Usage::ZERO },
    ];
    let trace = stream(&events, &Limits { parts: 1, ..LIMITS });
    assert!(match trace.last() {
        Some(Output::Completed { stop: Stop::MaxTokens, .. }) => true,
        _ => false,
    });
    assert_eq!(
        trace
            .iter()
            .filter(|out| match out {
                Output::Completed { .. } => true,
                _ => false,
            })
            .count(),
        1
    );
    assert!(!trace.iter().any(|out| match out {
        Output::Part(_) => true,
        _ => false,
    }));
}

#[test]
fn mismatched_indices_ids_kinds_and_early_end_fail_once() {
    let added = Event::Added { index: 0, id: owned(b"m"), kind: owned(b"message") };
    let scenarios = [
        Vec::from([Event::Added { index: 1, id: owned(b"m"), kind: owned(b"message") }]),
        Vec::from([
            added.clone(),
            Event::Done {
                index: 0,
                item: Item::Message { id: owned(b"other"), phase: None, text: owned(b"hello"), refusal: false },
            },
        ]),
        Vec::from([
            added.clone(),
            Event::Done {
                index: 0,
                item: Item::FunctionCall {
                    id: owned(b"m"),
                    call_id: owned(b"c"),
                    name: owned(b"read"),
                    arguments: owned(b"{}"),
                },
            },
        ]),
        Vec::from([added, Event::Completed { stop: Stop::EndTurn, usage: Usage::ZERO }]),
        Vec::from([Event::Progress]),
    ];
    for events in scenarios {
        let trace = stream(&events, &LIMITS);
        assert_eq!(
            trace
                .iter()
                .filter(|out| match out {
                    Output::Failed { .. } => true,
                    _ => false,
                })
                .count(),
            1
        );
        assert_eq!(
            trace
                .iter()
                .filter(|out| match out {
                    Output::Completed { .. } => true,
                    _ => false,
                })
                .count(),
            0
        );
    }
}

#[test]
fn server_events_roundtrip_echoes_skipped_and_errors_keep_reset_priority() {
    let events = [
        Event::Created { echo: None },
        Event::InProgress { echo: None },
        Event::Added { index: 0, id: owned(b"m"), kind: owned(b"message") },
        Event::Done {
            index: 0,
            item: Item::Message {
                id: owned(b"m"),
                phase: Some(owned(b"final_answer")),
                text: owned(b"hello"),
                refusal: false,
            },
        },
        Event::Completed {
            stop: Stop::MaxTokens,
            usage: Usage { input_tokens: 12, output_tokens: 3, cache_read_tokens: 4, cache_write_tokens: 0 },
        },
        Event::Progress,
        Event::Unknown,
    ];
    for event in events {
        let encoded = encode_event(&event, &LIMITS).unwrap();
        assert_eq!(decode_event(&value(&encoded), &LIMITS).unwrap(), event);
    }
    let error = ProviderError {
        kind: owned(b"usage_limit_reached"),
        message: owned(b"spent"),
        resets_in_seconds: Some(60),
        resets_at: Some(900),
    };
    let encoded = encode_error(&error, &LIMITS).unwrap();
    assert_eq!(decode_error(&value(&encoded), &LIMITS).unwrap(), error);
    let wall = Wall::from_nanos(100_000_000_000);
    assert_eq!(
        classify(429, Some(&error), RateLimit::NONE, wall),
        Failure::Exhausted { retry_after: Duration::from_secs(60) }
    );
    let mut rate = RateLimit::NONE;
    rate.observe(b"Retry-After", b"2");
    assert_eq!(classify(429, Some(&error), rate, wall), Failure::Exhausted { retry_after: Duration::from_secs(2) });
    let echo = encode_event(&Event::Created { echo: Some(request()) }, &LIMITS).unwrap();
    assert!(bytes::find(&echo, b"instructions").is_some());
    assert!(bytes::find(&echo, b"parameters").is_some());
    assert_eq!(decode_event(&value(&echo), &LIMITS), Ok(Event::Created { echo: None }));
}

#[test]
fn completed_usage_rejects_cached_over_total_and_done_honors_incomplete_status() {
    let valid = value(br#"{"type":"response.done","response":{"status":"incomplete","incomplete_details":{"reason":"content_filter"},"usage":{"input_tokens":8,"input_tokens_details":{"cached_tokens":3},"output_tokens":2},"output":[{"ignored":true}]}}"#);
    assert_eq!(
        decode_event(&valid, &LIMITS),
        Ok(Event::Completed {
            stop: Stop::Refusal,
            usage: Usage { input_tokens: 5, output_tokens: 2, cache_read_tokens: 3, cache_write_tokens: 0 }
        })
    );
    let invalid = value(br#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":2,"input_tokens_details":{"cached_tokens":3},"output_tokens":2}}}"#);
    assert_eq!(decode_event(&invalid, &LIMITS), Err(DecodeError::Malformed));
    assert_eq!(
        decode_event(
            &value(br#"{"type":"response.completed","response":{"status":"in_progress","usage":null}}"#),
            &LIMITS
        ),
        Err(DecodeError::Malformed)
    );
    let error = decode_error(
        &value(br#"{"error":{"type":"invalid_request_error","code":"context_length_exceeded","message":"too long"}}"#),
        &LIMITS,
    )
    .unwrap();
    assert_eq!(classify(400, Some(&error), RateLimit::NONE, Wall::EPOCH), Failure::ContextTooLong);
}

#[test]
fn malformed_tokens_duplicates_and_every_document_limit_are_refused() {
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
    assert_eq!(
        decode_event(&value(br#"{"type":"response.created","type":"error"}"#), &LIMITS),
        Err(DecodeError::Malformed)
    );
    assert!(worst_case(&LIMITS).unwrap() > u64::from(LIMITS.answer_bytes));
    assert_eq!(worst_case(&Limits { parts: u32::MAX, ..LIMITS }), None);
}

#[test]
fn archived_real_provider_requests_and_answers_match_known_completions() {
    for (scenario, input, output, tool_count, opaque_count) in [
        ("single-text", 27_u64, 17_u64, 0_usize, 2_usize),
        ("tool-call", 76, 18, 1, 0),
        ("tool-result-final", 116, 12, 0, 1),
    ] {
        let wrapper = Json::from_bytes(&fixture(scenario, "request.json"), &LIMITS).unwrap();
        let body = json::text_ref(
            json::value_at(wrapper.as_tokens(), json::required(wrapper.as_tokens(), b"body").unwrap()).unwrap(),
        )
        .unwrap();
        let request = decode_request(&value(body), &LIMITS).unwrap();
        assert!(!request.input.is_empty());
        let captured = dechunk(&fixture(scenario, "response.sse"));
        let mut decoder = StreamDecoder::new(&LIMITS);
        let mut out = Queue::with_capacity(crate::MAX_OUT);
        let mut trace = Vec::new();
        for line in core::str::from_utf8(&captured).unwrap().lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                decoder.event(decode_event(&value(data.as_bytes()), &LIMITS).unwrap(), &LIMITS, Wall::EPOCH, &mut out);
                drain(&mut out, &mut trace);
            }
        }
        decoder.end(&mut out);
        drain(&mut out, &mut trace);
        assert_eq!(
            trace
                .iter()
                .filter(|out| match out {
                    Output::Part(Part::ToolCall { .. }) => true,
                    _ => false,
                })
                .count(),
            tool_count
        );
        assert_eq!(
            trace
                .iter()
                .filter(|out| match out {
                    Output::Part(Part::Opaque { .. }) => true,
                    _ => false,
                })
                .count(),
            opaque_count
        );
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
            assert!(trace.iter().any(|out| match out {
                Output::Part(Part::Text { text }) if text.as_ref() == b"hello" => true,
                _ => false,
            }));
            assert!(trace.iter().any(|out| match out {
                Output::Part(Part::Opaque { bytes }) if bytes::find(bytes, b"encrypted_content").is_some() => true,
                _ => false,
            }));
        }
        if scenario == "tool-call" {
            assert!(trace.iter().any(|out| match out {
                Output::Part(Part::ToolCall { name, input, id, too_large: false })
                    if name.as_ref() == b"get_weather"
                        && input.as_ref() == br#"{"city":"Paris"}"#
                        && bytes::find(id, b"|").is_some() =>
                    true,
                _ => false,
            }));
        }
    }
}

#[test]
fn out_of_order_done_waits_for_order_and_terminal_waits_for_ready_parts() {
    let mut decoder = StreamDecoder::new(&LIMITS);
    let mut out = Queue::with_capacity(crate::MAX_OUT);
    let mut trace = Vec::new();
    for index in 0..3_u32 {
        decoder.event(
            Event::Added {
                index,
                id: owned(&[b'a'.wrapping_add(u8::try_from(index).unwrap())]),
                kind: owned(b"function_call"),
            },
            &LIMITS,
            Wall::EPOCH,
            &mut out,
        );
        drain(&mut out, &mut trace);
    }
    for index in [2_u32, 1, 0] {
        let id = owned(&[b'a'.wrapping_add(u8::try_from(index).unwrap())]);
        decoder.event(
            Event::Done {
                index,
                item: Item::FunctionCall {
                    id,
                    call_id: owned(&[b'x'.wrapping_add(u8::try_from(index).unwrap())]),
                    name: owned(b"read"),
                    arguments: owned(b"{}"),
                },
            },
            &LIMITS,
            Wall::EPOCH,
            &mut out,
        );
        drain(&mut out, &mut trace);
    }
    assert!(decoder.has_ready());
    decoder.event(Event::Completed { stop: Stop::EndTurn, usage: Usage::ZERO }, &LIMITS, Wall::EPOCH, &mut out);
    drain(&mut out, &mut trace);
    assert!(decoder.has_ready());
    assert!(!decoder.is_complete());
    decoder.end(&mut out);
    drain(&mut out, &mut trace);
    assert!(decoder.is_complete());
    assert!(!decoder.has_ready());
    let ids: Vec<Box<[u8]>> = trace
        .iter()
        .filter_map(|out| match out {
            Output::Part(Part::ToolCall { id, .. }) => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, Vec::from([owned(b"x|a"), owned(b"y|b"), owned(b"z|c")]));
    assert_eq!(trace.last(), Some(&Output::Completed { stop: Stop::ToolUse, usage: Usage::ZERO }));
}

#[test]
fn incomplete_and_refused_terminals_override_tools_and_absolute_reset_uses_wall() {
    for stop in [Stop::MaxTokens, Stop::Refusal] {
        let events = [
            Event::Added { index: 0, id: owned(b"f"), kind: owned(b"function_call") },
            Event::Done {
                index: 0,
                item: Item::FunctionCall {
                    id: owned(b"f"),
                    call_id: owned(b"c"),
                    name: owned(b"read"),
                    arguments: owned(b"{}"),
                },
            },
            Event::Completed { stop, usage: Usage::ZERO },
        ];
        assert_eq!(stream(&events, &LIMITS).last(), Some(&Output::Completed { stop, usage: Usage::ZERO }));
    }
    let mut decoder = StreamDecoder::new(&LIMITS);
    let mut out = Queue::with_capacity(crate::MAX_OUT);
    decoder.event(
        Event::Failed {
            error: ProviderError {
                kind: owned(b"usage_limit_reached"),
                message: owned(b"spent"),
                resets_in_seconds: None,
                resets_at: Some(180),
            },
        },
        &LIMITS,
        Wall::from_nanos(100_000_000_000),
        &mut out,
    );
    assert_eq!(
        out.pop(),
        Some(Output::Failed {
            failure: Failure::Exhausted { retry_after: Duration::from_secs(80) },
            detail: owned(b"spent")
        })
    );
    assert_eq!(
        stream(&[Event::Completed { stop: Stop::EndTurn, usage: Usage::ZERO }], &LIMITS),
        Vec::from([Output::Completed { stop: Stop::EndTurn, usage: Usage::ZERO }])
    );
}

#[test]
fn error_detail_truncation_preserves_utf8_boundaries() {
    let limits = Limits { detail_bytes: 3, ..LIMITS };
    let message = value("{\"error\":{\"code\":\"server_error\",\"message\":\"éé\"}}".as_bytes());
    let error = decode_error(&message, &limits).unwrap();
    assert_eq!(error.message.as_ref(), "é".as_bytes());
    assert_eq!(decode_error(&value(&encode_error(&error, &LIMITS).unwrap()), &LIMITS).unwrap(), error);
}

#[test]
fn server_refuses_request_limit_even_when_document_limit_is_larger() {
    let body = encode_request(&request(), &LIMITS).unwrap();
    let document = value(&body);
    assert_eq!(
        decode_request(&document, &Limits { request_bytes: u32::try_from(body.len()).unwrap() - 1, ..LIMITS }),
        Err(DecodeError::TooLarge)
    );
}

#[test]
fn fake_completion_echo_exercises_large_event_without_keeping_request() {
    let bytes = crate::encode_completion(Stop::EndTurn, Usage::ZERO, &request(), &LIMITS).unwrap();
    assert!(bytes::find(&bytes, b"instructions").is_some());
    assert!(bytes::find(&bytes, b"parameters").is_some());
    assert_eq!(
        decode_event(&value(&bytes), &LIMITS).unwrap(),
        Event::Completed { stop: Stop::EndTurn, usage: Usage::ZERO }
    );
}

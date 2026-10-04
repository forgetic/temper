use skein_lib::stream::{Fault, Up};
use skein_lib::{Duration, Time};
use temper_agent_domain::llm;
use temper_agent_protocol::exchange::{Event, Evidence};
use temper_agent_protocol_world::http::World;
use temper_channel::wire::Provider;

fn response(body: &[u8], media: &str, status: u16, framing: u8) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status} test\r\nContent-Type: {media}\r\n").into_bytes();
    match framing {
        0 => head.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()),
        1 => {
            head.extend_from_slice(b"Transfer-Encoding: chunked\r\n\r\n");
            head.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
        }
        2 => head.extend_from_slice(b"\r\n"),
        _ => panic!("known test framing"),
    }
    head.extend_from_slice(body);
    if framing == 1 {
        head.extend_from_slice(b"\r\n0\r\n\r\n");
    }
    head
}
fn openai_text() -> Vec<u8> {
    let body = b"event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"msg1\",\"type\":\"message\"}}\n\nevent: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"msg1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"input_tokens_details\":{\"cached_tokens\":1}}}}\n\n";
    response(body, "text/event-stream; charset=utf-8", 200, 0)
}
#[test]
fn compact_error_documents_survive_all_framings_and_slices() {
    for framing in 0..3 {
        for slice in [1, 7, 128, 4096] {
            let mut world = World::new(
                Provider::OpenAi,
                response(
                    br#"{"error":{"code":"usage_limit_reached","message":"spent","resets_in_seconds":12}}"#,
                    "application/json",
                    429,
                    framing,
                ),
                slice,
            );
            world.drive();
            assert!(world.events.iter().any(|event| matches!(event, Event::Failed { failure: llm::Failure::Exhausted { retry_after }, evidence: Evidence::Response, .. } if *retry_after == Duration::from_secs(12))));
            let mut context = World::new(
                Provider::Anthropic,
                response(
                    br#"{"error":{"type":"invalid_request_error","message":"prompt is too long"}}"#,
                    "application/json",
                    400,
                    framing,
                ),
                slice,
            );
            context.drive();
            assert!(context.events.iter().any(|event| matches!(
                event,
                Event::Failed { failure: llm::Failure::ContextTooLong, evidence: Evidence::Response, .. }
            )));
        }
    }
}
#[test]
fn successful_stream_emits_once_and_preserves_http_for_next_call() {
    for slice in [1, 19, 4096] {
        let mut world = World::new(Provider::OpenAi, openai_text(), slice);
        world.drive();
        assert_eq!(world.events.iter().filter(|e| matches!(e, Event::Completed { .. })).count(), 1);
        assert!(world.exchange.is_idle());
        world.replace(Provider::OpenAi, 2, openai_text());
        world.drive();
        assert_eq!(world.events.iter().filter(|e| matches!(e, Event::Completed { .. })).count(), 2);
        assert_eq!(world.sent.windows(17).filter(|w| *w == b"POST /responses H").count(), 2);
        world.deliver(Up::Failed(Fault::Reset));
        assert!(!world.events.iter().any(|e| matches!(e, Event::Failed { .. })));
    }
}
#[test]
fn cancellation_waits_for_actual_closed_and_deadlines_do_not_retry() {
    let mut world = World::new(Provider::OpenAi, Vec::new(), 1);
    world.room_enabled = false;
    world.cancel();
    assert!(!world.events.iter().any(|e| matches!(e, Event::Closed { .. })));
    world.deliver(Up::Room);
    assert!(world.sent.is_empty());
    world.close();
    world.close();
    assert_eq!(world.events.iter().filter(|e| matches!(e, Event::Closed { cancelled: true, .. })).count(), 1);
    let mut timed = World::new(Provider::OpenAi, Vec::new(), 1);
    timed.room_enabled = false;
    timed.fire(Time::from_nanos(1_000_000_000));
    assert!(
        timed
            .events
            .iter()
            .any(|e| matches!(e, Event::Failed { failure: llm::Failure::TimedOut, evidence: Evidence::Unsent, .. }))
    );
    assert!(timed.sent.is_empty());
    timed.fire(Time::from_nanos(9_000_000_000));
    assert_eq!(timed.events.iter().filter(|e| matches!(e, Event::Failed { .. })).count(), 1);
}
#[test]
fn media_type_and_cut_stream_fail_with_response_evidence() {
    for body in [
        response(b"{}", "application/json", 200, 0),
        response(b"data: {\"type\":\"response.created\",\"response\":{}}\n\n", "text/event-stream", 200, 0),
    ] {
        let mut world = World::new(Provider::OpenAi, body, 4096);
        world.drive();
        assert!(world.events.iter().any(|e| matches!(
            e,
            Event::Failed { failure: llm::Failure::Unavailable, evidence: Evidence::Response, .. }
        )));
    }
}
#[test]
fn whole_ping_events_reset_idle_but_partial_lines_and_absolute_deadlines_do_not() {
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
    let ping = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";
    let mut world = World::new(Provider::Anthropic, head.to_vec(), 4096);
    world.live = true;
    world.drive();
    world.env.now = Time::from_nanos(800_000_000);
    world.append(ping);
    world.drive();
    world.fire(Time::from_nanos(1_100_000_000));
    assert!(!world.closing, "whole ping keeps idle alive");
    world.env.now = Time::from_nanos(1_700_000_000);
    world.append(b"data: {");
    world.drive();
    world.fire(Time::from_nanos(1_900_000_000));
    assert!(world.events.iter().any(|e| matches!(e, Event::Failed { failure: llm::Failure::TimedOut, .. })));
    let mut absolute = World::new(Provider::Anthropic, head.to_vec(), 4096);
    absolute.live = true;
    absolute.drive();
    for tick in 1..=24 {
        absolute.env.now = Time::from_nanos(tick * 800_000_000);
        absolute.append(ping);
        absolute.drive();
    }
    absolute.fire(Time::from_nanos(20_000_000_000));
    assert!(absolute.events.iter().any(|e| matches!(e, Event::Failed { failure: llm::Failure::TimedOut, .. })));
}

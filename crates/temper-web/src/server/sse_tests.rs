// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::board::{BoardEvent, Lane};

#[test]
fn data_frame_is_valid_sse() {
    let json = r#"{"t":"card.move","seq":1,"id":"i42","lane":"implement","now":5}"#;
    let frame = data_frame(json);
    assert_eq!(frame, format!("data: {json}\n\n"));
    assert!(frame.starts_with("data: "));
    assert!(frame.ends_with("\n\n"));
}

#[test]
fn id_frame_carries_the_canonical_resume_cursor() {
    assert_eq!(
        id_data_frame(42, r#"{"seq":42}"#),
        "id: 42\ndata: {\"seq\":42}\n\n"
    );
}

#[test]
fn board_event_serializes_to_a_single_line_frame() {
    // Frames must be newline-free so the `\n\n` terminator is unambiguous;
    // compact serde_json never embeds a newline.
    let event = BoardEvent::CardMove {
        seq: 7,
        id: "i42".to_string(),
        lane: Lane::Done,
        now: 99,
    };
    let json = serde_json::to_string(&event).expect("serializes");
    assert!(!json.contains('\n'));
    let frame = data_frame(&json);
    assert_eq!(frame.matches("\n\n").count(), 1);
}

#[test]
fn keep_alive_is_a_comment_frame() {
    let frame = keep_alive();
    assert!(frame.starts_with(':'));
    assert!(frame.ends_with("\n\n"));
    // a comment frame carries no `data:` so EventSource.onmessage never fires
    assert!(!frame.contains("data:"));
}

#[test]
fn sse_head_declares_event_stream() {
    let head = sse_response_head();
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(head.contains("Content-Type: text/event-stream\r\n"));
    assert!(head.contains("Cache-Control: no-cache\r\n"));
    assert!(head.ends_with("\r\n\r\n"));
}

#[test]
fn broadcaster_fans_frames_to_all_subscribers() {
    let hub = Broadcaster::new();
    let a = hub.subscribe();
    let b = hub.subscribe();
    assert_eq!(hub.subscriber_count(), 2);

    hub.broadcast(&data_frame(r#"{"seq":1}"#));
    assert_eq!(a.recv().unwrap(), "data: {\"seq\":1}\n\n");
    assert_eq!(b.recv().unwrap(), "data: {\"seq\":1}\n\n");
}

#[test]
fn dropped_subscriber_is_pruned_on_next_broadcast() {
    let hub = Broadcaster::new();
    let live = hub.subscribe();
    let dropped = hub.subscribe();
    drop(dropped);
    hub.broadcast("x"); // prunes the dropped one
    assert_eq!(hub.subscriber_count(), 1);
    assert_eq!(live.recv().unwrap(), "x");
}

#[test]
fn frames_arrive_in_order() {
    let hub = Broadcaster::new();
    let sub = hub.subscribe();
    for seq in 1..=3 {
        hub.broadcast(&data_frame(&format!("{{\"seq\":{seq}}}")));
    }
    assert_eq!(sub.recv().unwrap(), "data: {\"seq\":1}\n\n");
    assert_eq!(sub.recv().unwrap(), "data: {\"seq\":2}\n\n");
    assert_eq!(sub.recv().unwrap(), "data: {\"seq\":3}\n\n");
}

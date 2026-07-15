use std::io::Write as _;
use std::net::{Shutdown, TcpStream};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use temper_protocol_activity::{
    AgentActivityChildRecordV1, AgentActivityEventV1, AssistantMessageV1, BlobAttachmentV1,
    BlobMediaTypeV1, CaptureModeV1, CapturedContentV1,
};

use super::tests::{context, send, usage_frame};
use super::*;

fn collector(root: &Path) -> TraceCollector {
    TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(root.to_path_buf()),
    })
}

fn attachment_record(message_id: &str) -> AgentActivityChildRecordV1 {
    let attachment = BlobAttachmentV1::from_bytes(BlobMediaTypeV1::TextMarkdownUtf8, b"x");
    let mut frame = usage_frame(2);
    frame.event = AgentActivityEventV1::AssistantMessage(AssistantMessageV1 {
        message_id: message_id.to_string(),
        content: CapturedContentV1::Blob {
            blob: attachment.blob.clone(),
        },
    });
    let record = AgentActivityChildRecordV1 {
        frame,
        blobs: vec![attachment],
    };
    record.validate().expect("valid attachment record");
    record
}

#[test]
fn persistent_endpoint_stream_survives_idle_and_partial_record_polls() {
    const READ_POLL: Duration = Duration::from_millis(25);

    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let run = collector
        .begin_run("job-persistent-endpoint", &context())
        .expect("begin")
        .expect("enabled");
    let endpoint = ActivityEndpoint::bind_with_read_timeout(run.clone(), READ_POLL)
        .expect("bind endpoint with test read timeout");
    let mut stream = TcpStream::connect(endpoint.address()).expect("connect persistent stream");

    write_record(&mut stream, &serde_json::to_vec(&usage_frame(1)).unwrap());
    wait_for_event_count(&collector, 2);
    thread::sleep(Duration::from_millis(100));

    let second = serde_json::to_vec(&usage_frame(2)).unwrap();
    let split = second.len() / 2;
    stream
        .write_all(&second[..split])
        .expect("write partial second record");
    thread::sleep(Duration::from_millis(100));
    stream
        .write_all(&second[split..])
        .expect("complete second record");
    stream.write_all(b"\n").expect("write second delimiter");
    stream.shutdown(Shutdown::Write).expect("close writer");
    wait_for_event_count(&collector, 3);

    endpoint.stop();
    assert_eq!(run.finish_success(None).expect("finish"), 4);
    drop(run);

    let recovered = collector.recover().expect("recover persistent stream");
    assert_eq!(
        recovered[0]
            .events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(
        recovered[0]
            .events
            .iter()
            .filter_map(|event| match &event.event {
                AgentActivityEventV1::Usage(usage) => Some(usage.input_tokens),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn endpoint_record_limit_is_cumulative_across_idle_polls() {
    const READ_POLL: Duration = Duration::from_millis(25);

    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..Default::default()
        },
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("job-endpoint-record-limit", &context())
        .expect("begin")
        .expect("enabled");
    let endpoint = ActivityEndpoint::bind_with_read_timeout(run.clone(), READ_POLL)
        .expect("bind endpoint with test read timeout");

    let mut stream = TcpStream::connect(endpoint.address()).expect("connect near-limit stream");
    write_record(&mut stream, &serde_json::to_vec(&usage_frame(1)).unwrap());
    wait_for_event_count(&collector, 2);

    let encoded_record = serde_json::to_vec(&attachment_record("near-limit-message")).unwrap();
    assert!(encoded_record.len() < MAX_CHILD_ACTIVITY_RECORD_BYTES);
    let mut near_limit = vec![b' '; MAX_CHILD_ACTIVITY_RECORD_BYTES - encoded_record.len()];
    near_limit.extend_from_slice(&encoded_record);
    near_limit.push(b'\n');
    stream
        .write_all(&near_limit)
        .expect("write maximum-sized record");
    stream.shutdown(Shutdown::Write).expect("close writer");
    wait_for_event_count(&collector, 3);
    drop(near_limit);

    // If an idle retry discarded the prefix or reset its allowance, the valid
    // frame suffix would be accepted. Cumulative accounting instead consumes
    // the three sentinel bytes and rejects this oversized record.
    let mut oversized = TcpStream::connect(endpoint.address()).expect("connect oversized stream");
    oversized
        .write_all(&vec![b' '; MAX_CHILD_ACTIVITY_RECORD_BYTES])
        .expect("write oversized prefix");
    thread::sleep(Duration::from_millis(100));
    let suffix = serde_json::to_vec(&usage_frame(3)).unwrap();
    let _ = oversized.write_all(&suffix);
    let _ = oversized.write_all(b"\n");
    let _ = oversized.shutdown(Shutdown::Write);

    send(&endpoint, &serde_json::to_vec(&usage_frame(4)).unwrap());
    wait_for_usage(&collector, 4);
    endpoint.stop();
    assert_eq!(
        run.finish_success(None).expect("finish"),
        5,
        "the valid suffix of an oversized split record must not be accepted"
    );
    drop(run);

    let recovered = collector.recover().expect("recover bounded stream");
    assert_eq!(
        recovered[0]
            .events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(
        recovered[0]
            .events
            .iter()
            .filter_map(|event| match &event.event {
                AgentActivityEventV1::Usage(usage) => Some(usage.input_tokens),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![1, 4]
    );
}

#[test]
fn stopping_an_accepted_idle_stream_is_bounded_by_one_read_poll() {
    const READ_POLL: Duration = Duration::from_millis(40);
    const SCHEDULER_TOLERANCE: Duration = Duration::from_millis(250);

    let temp = tempfile::tempdir().expect("tempdir");
    let collector = collector(temp.path());
    let run = collector
        .begin_run("job-stop-idle-endpoint", &context())
        .expect("begin")
        .expect("enabled");
    let endpoint = ActivityEndpoint::bind_with_read_timeout(run.clone(), READ_POLL)
        .expect("bind endpoint with test read timeout");
    let mut stream = TcpStream::connect(endpoint.address()).expect("connect idle stream");
    write_record(&mut stream, &serde_json::to_vec(&usage_frame(1)).unwrap());
    wait_for_event_count(&collector, 2);

    let started = Instant::now();
    endpoint.stop();
    assert!(
        started.elapsed() <= READ_POLL + SCHEDULER_TOLERANCE,
        "idle endpoint stop took {:?}",
        started.elapsed()
    );
    drop(stream);
    assert_eq!(run.finish_success(None).expect("finish"), 3);
    drop(run);

    let recovered = collector.recover().expect("recover stopped stream");
    assert_eq!(recovered[0].events.len(), 3);
}

fn wait_for_event_count(collector: &TraceCollector, expected: usize) {
    for _ in 0..500 {
        if collector
            .recover()
            .ok()
            .is_some_and(|runs| runs[0].events.len() >= expected)
        {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    let actual = collector
        .recover()
        .expect("recover while waiting for endpoint")
        .into_iter()
        .next()
        .map_or(0, |run| run.events.len());
    panic!("timed out waiting for {expected} events; recovered {actual}");
}

fn wait_for_usage(collector: &TraceCollector, input_tokens: u64) {
    for _ in 0..500 {
        if collector.recover().ok().is_some_and(|runs| {
            runs[0].events.iter().any(|event| {
                matches!(
                    &event.event,
                    AgentActivityEventV1::Usage(usage)
                        if usage.input_tokens == input_tokens
                )
            })
        }) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for usage event with {input_tokens} input tokens");
}

fn write_record(stream: &mut TcpStream, payload: &[u8]) {
    stream.write_all(payload).expect("write activity record");
    stream.write_all(b"\n").expect("write record delimiter");
}

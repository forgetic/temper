//! Occupied page/token memory and finite convenience paths, including copies.
use temper_forge_forgejo::{
    Limits,
    json::Json,
    response::{self, Kind, Selection},
    types::{self, Document, ObjectFormat},
    webhook,
};
use temper_world::heap::{self, Meter};
#[global_allocator]
static HEAP: heap::Counting = heap::Counting;
fn limits() -> Limits {
    Limits {
        depth: 16,
        tokens: 1024,
        page: 3,
        fields: 8,
        name_bytes: 64,
        title_bytes: 128,
        body_bytes: 128,
        marker_bytes: 16,
        document_bytes: 8192,
        hook_bytes: 8192,
    }
}
#[test]
fn occupied_comments_page_with_maximum_text_stays_within_bound() {
    let limits = limits();
    let user = types::User { id: 1, login: vec![b'u'; 64].into_boxed_slice() };
    let comments = (0..limits.page)
        .map(|id| types::Comment {
            id: u64::from(id) + 1,
            user: user.clone(),
            body: vec![b'x'; 128].into_boxed_slice(),
            issue_url: vec![b'i'; 64].into_boxed_slice(),
            created: 1_700_000_000_000_000_000,
            updated: 1_700_000_000_000_000_000,
            revision: 0,
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let wire = response::encode(&Document::Comments(comments), ObjectFormat::Sha1, &limits).unwrap();
    let tokens = Json::from_bytes(&wire, &limits).unwrap();
    let bound = response::worst_case(&limits).unwrap();
    let meter = Meter::new();
    let mut decoder =
        response::Decoder::new(Kind::Comments, ObjectFormat::Sha1, Selection::first(limits.page), &limits).unwrap();
    let mut peak = 0;
    for token in tokens.tokens() {
        let token = token.clone();
        meter.start();
        decoder.token(token).unwrap();
        let step = meter.end();
        peak = peak.max(step.peak);
        meter.check(step, bound, "row decoding and retained full page");
    }
    eprintln!("response occupied peak={peak}, bound={bound}");
    meter.start();
    let page = decoder.finish().unwrap();
    let step = meter.end();
    drop(page);
    meter.check(step, bound, "final typed page assembly");
    assert_eq!(meter.held(), 0);
    let meter = Meter::new();
    meter.start();
    let decoded =
        response::from_bytes(&wire, Kind::Comments, ObjectFormat::Sha1, Selection::first(limits.page), &limits)
            .unwrap();
    let step = meter.end();
    eprintln!("finite response peak={}, bound={}", step.peak, temper_forge_forgejo::worst_case(&limits).unwrap());
    drop(decoded);
    meter.check(step, temper_forge_forgejo::worst_case(&limits).unwrap(), "finite tokenizer/response convenience path");
    assert_eq!(meter.held(), 0);
}
#[test]
fn webhook_retains_only_metadata_from_large_commit_history() {
    let limits = limits();
    let body = format!(
        "{{\"repository\":{{\"id\":1,\"full_name\":\"owner/repository\"}},\"sender\":{{\"id\":7}},\"ref\":\"refs/heads/main\",\"after\":\"1111111111111111111111111111111111111111\",\"commits\":[{{\"message\":\"{}\"}}]}}",
        "ignored".repeat(256)
    );
    let tokens = Json::from_bytes(body.as_bytes(), &limits).unwrap();
    let bound = webhook::worst_case(&limits).unwrap();
    let meter = Meter::new();
    let mut decoder = webhook::Decoder::new(&limits);
    for token in tokens.tokens() {
        let token = token.clone();
        meter.start();
        decoder.token(token).unwrap();
        let step = meter.end();
        meter.check(step, bound, "ignored subtree and retained webhook fields");
    }
    meter.start();
    let payload = decoder.finish(b"push", ObjectFormat::Sha1, &limits).unwrap().unwrap();
    let step = meter.end();
    eprintln!("webhook peak={}, bound={bound}", step.peak);
    drop(payload);
    meter.check(step, bound, "webhook payload projection");
    assert_eq!(meter.held(), 0);
}

use temper_fake_forge_protocol_world::world::World;
use temper_forge_forgejo::{
    request::{Items, Operation, Page, Repository, Request},
    response::{self, Kind, Selection},
    types::{Document, ObjectFormat},
};
fn request(operation: Operation) -> Request {
    Request {
        repository: Some(Repository { owner: Box::from(b"owner".as_slice()), name: Box::from(b"repo".as_slice()) }),
        operation,
    }
}
fn body(wire: &[u8]) -> &[u8] {
    let at = wire.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
    &wire[at..]
}
#[test]
fn create_list_comment_and_label_requests_use_the_real_http_server() {
    let mut world = World::new();
    world.wire(&request(Operation::CreateIssue {
        title: Box::from(b"title".as_slice()),
        body: Box::from(b"body".as_slice()),
        labels: Box::from([10, 999]),
    }));
    world.drive();
    assert!(world.output.starts_with(b"HTTP/1.1 201"));
    let created = response::from_bytes(
        body(&world.output),
        Kind::Item,
        ObjectFormat::Sha1,
        Selection::first(2),
        &world.env.limits.documents,
    )
    .unwrap();
    let Document::Item(item) = created else { panic!("item") };
    assert_eq!(item.number, 1);
    assert_eq!(item.labels.len(), 1);
    world.output.clear();
    world.wire(&request(Operation::Items(Items {
        state: None,
        pulls: None,
        label: None,
        author: None,
        since: None,
        page: Page { number: 1, limit: 2 },
    })));
    world.drive();
    assert!(world.output.starts_with(b"HTTP/1.1 200"));
    let listed = response::from_bytes(
        body(&world.output),
        Kind::Items,
        ObjectFormat::Sha1,
        Selection::first(2),
        &world.env.limits.documents,
    )
    .unwrap();
    let Document::Items(items) = listed else { panic!("items") };
    assert_eq!(items.len(), 1);
    world.output.clear();
    world.wire(&request(Operation::Post { number: 1, body: Box::from(b"comment".as_slice()) }));
    world.drive();
    assert!(world.output.starts_with(b"HTTP/1.1 201"));
    let comment = response::from_bytes(
        body(&world.output),
        Kind::Comment,
        ObjectFormat::Sha1,
        Selection::first(2),
        &world.env.limits.documents,
    )
    .unwrap();
    let Document::Comment(comment) = comment else { panic!("comment") };
    assert_eq!(comment.body.as_ref(), b"comment");
    world.output.clear();
    world.wire(&request(Operation::AddLabels { number: 1, labels: Box::from([Box::from(b"unknown".as_slice())]) }));
    world.drive();
    assert!(world.output.starts_with(b"HTTP/1.1 200"));
    assert_eq!(world.connection.calls(), 4);
}
#[test]
fn bad_auth_and_oversized_heads_close_after_a_wire_response() {
    let mut world = World::new();
    world.raw(b"GET /api/v1/user HTTP/1.1\r\nHost: fixture\r\nAuthorization: token wrong\r\nContent-Length: 0\r\n\r\n");
    world.drive();
    assert!(world.output.starts_with(b"HTTP/1.1 403"));
    let mut world = World::new();
    world.raw(format!("GET /{} HTTP/1.1\r\nHost: fixture\r\n\r\n", "x".repeat(3000)).as_bytes());
    world.drive();
    assert!(world.output.starts_with(b"HTTP/1.1 414"));
    assert!(world.closed);
}
#[test]
fn oversized_announced_body_is_refused_at_the_http_entrance() {
    let mut world = World::new();
    world.raw(b"POST /api/v1/repos/owner/repo/issues HTTP/1.1\r\nHost: fixture\r\nAuthorization: token fixture-token\r\nContent-Length: 999999\r\n\r\n");
    world.drive();
    assert!(world.output.starts_with(b"HTTP/1.1 413"));
    assert!(world.closed);
    assert_eq!(world.connection.calls(), 0);
}

fn unchunk(wire: &[u8]) -> Vec<u8> {
    let mut body = body(wire);
    let mut out = Vec::new();
    for _ in 0..8192 {
        let at = body.windows(2).position(|bytes| bytes == b"\r\n").unwrap();
        let count = usize::from_str_radix(std::str::from_utf8(&body[..at]).unwrap(), 16).unwrap();
        body = &body[at + 2..];
        if count == 0 {
            return out;
        }
        out.extend_from_slice(&body[..count]);
        assert_eq!(&body[count..count + 2], b"\r\n");
        body = &body[count + 2..];
    }
    panic!("finite chunked fixture");
}
#[test]
fn unpaged_comments_stream_all_rows_across_store_pages() {
    let mut world = World::new();
    world.wire(&request(Operation::CreateIssue {
        title: Box::from(b"thread".as_slice()),
        body: Box::from(b"body".as_slice()),
        labels: Box::from([]),
    }));
    world.drive();
    for index in 0..5 {
        world.output.clear();
        world.wire(&request(Operation::Post {
            number: 1,
            body: format!("comment {index}").into_bytes().into_boxed_slice(),
        }));
        world.drive();
        assert!(world.output.starts_with(b"HTTP/1.1 201"));
    }
    world.output.clear();
    world.wire(&request(Operation::Comments { number: 1, since: None }));
    world.drive();
    assert!(world.output.starts_with(b"HTTP/1.1 200"));
    assert!(world.output.windows(26).any(|bytes| bytes.eq_ignore_ascii_case(b"Transfer-Encoding: chunked")));
    let bytes = unchunk(&world.output);
    let json = temper_forge_forgejo::json::Json::from_bytes(&bytes, &world.env.limits.documents).unwrap();
    let mut decoder =
        response::Decoder::new(Kind::Comments, ObjectFormat::Sha1, Selection::first(2), &world.env.limits.documents)
            .unwrap();
    for token in json.tokens() {
        decoder.token(token.clone()).unwrap();
    }
    assert_eq!(decoder.rows_seen(), 5, "the server has streamed every stored page");
    let Document::Comments(first) = decoder.finish().unwrap() else { panic!("comments") };
    assert_eq!(first.len(), 2);
    assert_eq!(world.connection.calls(), 7, "one HTTP comments request, regardless of history pages");
}

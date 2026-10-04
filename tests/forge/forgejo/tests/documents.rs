use temper_forge_forgejo::{
    Limits, binary,
    request::{self, Operation, Page, Repository, Request},
    response::{self, Kind, Selection},
    time,
    types::*,
};

fn repo() -> Option<Repository> {
    Some(Repository { owner: Box::from(b"owner name".as_slice()), name: Box::from(b"repo".as_slice()) })
}
fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}
#[test]
fn requests_use_explicit_pages_encoded_segments_and_correct_label_types() {
    let limits = Limits::STARTING;
    let operations = [
        Operation::Branch { name: bytes(b"feature/a ?%") },
        Operation::CreateIssue { title: bytes(b"title"), body: bytes(b"body"), labels: Box::from([11, 12]) },
        Operation::AddLabels { number: 7, labels: Box::from([bytes(b"track")]) },
        Operation::Reviews { number: 7, page: Page { number: 2, limit: 3 } },
        Operation::Comments { number: 7, since: Some(1_700_000_000_000_000_000) },
        Operation::Merge {
            number: 7,
            style: bytes(b"squash"),
            head: bytes(b"0000000000000000000000000000000000000001"),
        },
        Operation::PutPage {
            name: bytes(b"notes/a"),
            create: false,
            content_base64: bytes(b"Ym9keQ=="),
            message: bytes(b"temper"),
        },
        Operation::PullFor { base: bytes(b"main"), head: bytes(b"feature/a") },
        Operation::SearchUser { id: 9 },
    ];
    for operation in operations {
        let repository = if matches!(operation, Operation::SearchUser { .. }) { None } else { repo() };
        let request = Request { repository, operation };
        let encoded = request::encode(&request, &limits).unwrap();
        let decoded =
            request::decode(encoded.method, &encoded.target, encoded.body.as_deref().unwrap_or(b""), 30, &limits)
                .unwrap();
        assert_eq!(decoded.request, request);
    }
    let branch = request::encode(
        &Request { repository: repo(), operation: Operation::Branch { name: bytes(b"feature/a ?%") } },
        &limits,
    )
    .unwrap();
    assert_eq!(branch.target.as_ref(), b"/api/v1/repos/owner%20name/repo/branches/feature%2Fa%20%3F%25");
}
#[test]
fn source_dates_and_object_ids_are_checked() {
    for value in [0, 1_700_000_000_000_000_000, u64::MAX / 1_000_000_000 * 1_000_000_000] {
        assert_eq!(time::parse(&time::format(value)).unwrap(), value);
    }
    assert_eq!(time::parse(b"2026-10-04T12:34:56+02:00").unwrap(), time::parse(b"2026-10-04T10:34:56Z").unwrap());
    assert!(time::parse(b"2025-02-29T00:00:00Z").is_err());
    assert!(time::parse(b"2024-02-29T00:00:00Z").is_ok());
    let sha1 = binary::commit(b"0000000000000000000000000000000000000001", ObjectFormat::Sha1).unwrap();
    assert_eq!(
        binary::commit_hex(&sha1, ObjectFormat::Sha1).unwrap().as_ref(),
        b"0000000000000000000000000000000000000001"
    );
    assert!(binary::commit(b"0000000000000000000000000000000000000001", ObjectFormat::Sha256).is_err());
    let mut bad = sha1;
    bad[31] = 1;
    assert!(binary::commit_hex(&bad, ObjectFormat::Sha1).is_err());
}
#[test]
fn independent_status_fixture_uses_status_inside_rows() {
    // Hand-authored from retained v15 Swagger definitions, not a real capture.
    let body=br#"{"state":"warning","total_count":1,"repository":{"owner":{"id":99}},"statuses":[{"context":"ci","status":"warning","creator":{"id":1,"login":"bot"},"description":"ok","target_url":"https://example.test/ci","created_at":"2026-10-04T00:00:00Z"}]}"#;
    let response =
        response::from_bytes(body, Kind::Statuses, ObjectFormat::Sha1, Selection::first(1), &Limits::STARTING).unwrap();
    let Document::Statuses { state, total_count, statuses } = response else { panic!("combined status") };
    assert_eq!(state, Check::Warning);
    assert_eq!(total_count, 1);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, Check::Warning);
}
#[test]
fn all_finite_response_shapes_roundtrip_and_unknown_fields_are_discarded() {
    let limits = Limits::STARTING;
    let user = User { id: 1, login: bytes(b"bot") };
    let commit = [0; 32];
    let at = 1_700_000_000_000_000_000;
    let item = Item {
        number: 7,
        kind: ItemKind::Pull,
        state: State::Open,
        user: user.clone(),
        title: bytes(b"title"),
        body: bytes(b"body"),
        labels: Box::from([Label { id: 3, name: bytes(b"track") }]),
        created: at,
        updated: at,
    };
    let comment = Comment {
        id: 8,
        user: user.clone(),
        body: bytes(b"comment"),
        issue_url: bytes(b"https://example.test/owner/repo/issues/7"),
        created: at,
        updated: at,
        revision: 0,
    };
    let page = WikiPage { title: bytes(b"notes"), content: Some(bytes(b"body")), sha: commit };
    let documents = [
        (Kind::Settings, Document::Settings { max_response_items: 50, default_paging_num: 30 }),
        (Kind::User, Document::User(user.clone())),
        (Kind::Users, Document::Users(Box::from([user.clone()]))),
        (
            Kind::Repository,
            Document::Repository(RepositoryInfo {
                id: 1,
                full_name: bytes(b"owner/repo"),
                object_format: ObjectFormat::Sha1,
                has_wiki: true,
                default_branch: bytes(b"main"),
            }),
        ),
        (Kind::Labels, Document::Labels(item.labels.clone())),
        (Kind::Item, Document::Item(item.clone())),
        (Kind::Comments, Document::Comments(Box::from([comment]))),
        (
            Kind::Pull,
            Document::Pull(Pull {
                number: 7,
                state: State::Open,
                head: bytes(b"feature"),
                base: bytes(b"main"),
                commit,
                base_commit: commit,
                merged: false,
                merge_commit: None,
                mergeable: true,
                reviewers: Box::from([user.clone()]),
            }),
        ),
        (
            Kind::Reviews,
            Document::Reviews(Box::from([Review {
                id: 1,
                user: user.clone(),
                state: ReviewState::Approved,
                commit,
                body: bytes(b"okay"),
                submitted: at,
                official: true,
                dismissed: false,
            }])),
        ),
        (Kind::Permission, Document::Permission(Permission::Owner)),
        (Kind::Branch, Document::Branch(commit)),
        (Kind::Page, Document::Page(page)),
        (
            Kind::Dependencies,
            Document::Dependencies(Box::from([Dependency {
                number: 7,
                owner: bytes(b"owner"),
                repository: bytes(b"repo"),
            }])),
        ),
        (Kind::Error, Document::Error { message: bytes(b"no") }),
        (Kind::Done, Document::Done),
    ];
    for (kind, document) in documents {
        let body = response::encode(&document, ObjectFormat::Sha1, &limits).unwrap();
        let decoded = response::from_bytes(&body, kind, ObjectFormat::Sha1, Selection::first(50), &limits).unwrap();
        if kind == Kind::Comments {
            let Document::Comments(comments) = decoded else { panic!("comments") };
            assert_ne!(comments[0].revision, 0);
        } else {
            assert_eq!(decoded, document);
        }
    }
    let body = br#"{"id":1,"login":"bot","unused":{"very":["large",{"id":999}]}}"#;
    assert_eq!(
        response::from_bytes(body, Kind::User, ObjectFormat::Sha1, Selection::first(1), &limits).unwrap(),
        Document::User(user)
    );
}
#[test]
fn duplicate_known_fields_and_invalid_token_documents_are_refused() {
    for body in [
        br#"{"id":1,"id":2,"login":"bot"}"#.as_slice(),
        br#"{"id":-1,"login":"bot"}"#,
        br#"{"id":1,"login":"bot"}x"#,
        br#"{"id":1,"login":"unterminated}"#,
    ] {
        assert!(
            response::from_bytes(body, Kind::User, ObjectFormat::Sha1, Selection::first(1), &Limits::STARTING).is_err()
        );
    }
}

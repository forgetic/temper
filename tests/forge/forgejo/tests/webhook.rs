use temper_forge_forgejo::{
    Limits, binary,
    json::Json,
    types::ObjectFormat,
    webhook::{self, Payload, Signature},
};

#[test]
fn rfc4231_hmac_known_vector_matches_every_fragmentation() {
    let expected = b"b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7";
    for split in 0..=8 {
        let mut signer = Signature::new(&[0x0b; 20], 8);
        signer.update(&b"Hi There"[..split]).unwrap();
        signer.update(&b"Hi There"[split..]).unwrap();
        assert!(signer.verify(expected).unwrap());
    }
    let mut poisoned = Signature::new(b"key", 1);
    assert!(poisoned.update(b"two").is_err());
    assert!(poisoned.sign().is_err());
    let mut wrong = Signature::new(b"wrong", 8);
    wrong.update(b"Hi There").unwrap();
    assert!(!wrong.verify(expected).unwrap());
}
#[test]
fn event_documents_roundtrip_and_skip_push_history() {
    let limits = Limits::STARTING;
    let payload = Payload {
        repository: Box::from(b"owner/repo".as_slice()),
        repository_id: 7,
        by: Some(9),
        item: Some(1),
        branch: Some(Box::from(b"feature/a".as_slice())),
        commit: Some([0; 32]),
        wiki: false,
    };
    for event in [
        b"issues".as_slice(),
        b"issue_comment",
        b"pull_request",
        b"pull_request_approved",
        b"pull_request_rejected",
        b"push",
        b"create",
        b"delete",
        b"status",
        b"wiki",
        b"action_run_success",
        b"action_run_recover",
        b"action_run_failure",
    ] {
        let encoded = webhook::encode(event, &payload, ObjectFormat::Sha1, &limits).unwrap();
        let json = Json::from_bytes(&encoded, &limits).unwrap();
        let mut decoder = webhook::Decoder::new(&limits);
        for token in json.tokens() {
            decoder.token(token.clone()).unwrap();
        }
        let decoded = decoder.finish(event, ObjectFormat::Sha1, &limits).unwrap().unwrap();
        assert_eq!(decoded.repository, payload.repository);
        assert_eq!(decoded.by, payload.by);
        if event == b"push" {
            assert_eq!(decoded.branch, payload.branch);
            assert!(decoded.commit.is_none(), "all-zero push after means deletion");
        }
        if event == b"wiki" {
            assert!(decoded.wiki);
        }
        let mut signature = Signature::new(b"secret", limits.hook_bytes);
        for chunk in encoded.chunks(3) {
            signature.update(chunk).unwrap();
        }
        assert_eq!(binary::hex(&signature.sign().unwrap()).len(), 64);
    }
    let json=Json::from_bytes(br#"{"repository":{"id":7,"full_name":"owner/repo"},"sender":{"id":9},"ref":"refs/tags/tag","after":"0000000000000000000000000000000000000000","commits":[{"message":"history","author":{"id":88}}]}"#,&limits).unwrap();
    let mut decoder = webhook::Decoder::new(&limits);
    for token in json.tokens() {
        decoder.token(token.clone()).unwrap();
    }
    assert!(decoder.finish(b"push", ObjectFormat::Sha1, &limits).unwrap().is_none());
    assert!(webhook::decode(b"unknown", json.tokens(), ObjectFormat::Sha1, &limits).unwrap().is_none());
}

#[test]
fn source_derived_v15_action_metadata_is_inside_run() {
    let limits = Limits::STARTING;
    // Source-derived selected-field fixture, not a live Actions capture. See
    // fixtures/PROVENANCE.md for the exact v15 struct and notifier sources.
    let body = br#"{"action":"success","prior_status":"running","run":{"repository":{"id":7,"full_name":"owner/repo"},"trigger_user":{"id":9},"commit_sha":"1111111111111111111111111111111111111111","event_payload":"ignored history","last_run":{"repository":{"id":999}}}}"#;
    let json = Json::from_bytes(body, &limits).unwrap();
    let mut decoder = webhook::Decoder::new(&limits);
    for token in json.tokens() {
        decoder.token(token.clone()).unwrap();
    }
    let payload = decoder.finish(b"action_run_success", ObjectFormat::Sha1, &limits).unwrap().unwrap();
    assert_eq!(payload.repository_id, 7);
    assert_eq!(payload.by, Some(9));
    assert_eq!(&payload.commit.unwrap()[..20], &[0x11; 20]);
    assert_eq!(webhook::kind(b"workflow_run"), webhook::Kind::Unknown);
    assert_eq!(webhook::kind(b"pull_request_review_approved"), webhook::Kind::Unknown);
}

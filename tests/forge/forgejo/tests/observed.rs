//! Actual disposable Forgejo v15 captures; see fixtures/v15/provenance.json.
use temper_forge_forgejo::{
    Limits,
    json::Json,
    response::{self, Kind, Selection},
    types::ObjectFormat,
    webhook::{Decoder, Signature},
};
#[test]
fn captured_v15_documents_decode_with_the_production_codecs() {
    let limits = Limits::STARTING;
    for (kind, body) in [
        (Kind::Settings, include_bytes!("../fixtures/v15/api-settings.json").as_slice()),
        (Kind::User, include_bytes!("../fixtures/v15/current-user.json").as_slice()),
        (Kind::Users, include_bytes!("../fixtures/v15/user-search-uid.json").as_slice()),
        (Kind::Repository, include_bytes!("../fixtures/v15/repository.json").as_slice()),
        (Kind::Labels, include_bytes!("../fixtures/v15/labels.json").as_slice()),
        (Kind::Item, include_bytes!("../fixtures/v15/item.json").as_slice()),
        (Kind::Items, include_bytes!("../fixtures/v15/items-leastupdate.json").as_slice()),
        (Kind::Comment, include_bytes!("../fixtures/v15/comment.json").as_slice()),
        (Kind::Comments, include_bytes!("../fixtures/v15/comments-unpaged-since.json").as_slice()),
        (Kind::Permission, include_bytes!("../fixtures/v15/permission.json").as_slice()),
        (Kind::Branch, include_bytes!("../fixtures/v15/branch.json").as_slice()),
        (Kind::Pull, include_bytes!("../fixtures/v15/pull.json").as_slice()),
        (Kind::Review, include_bytes!("../fixtures/v15/review.json").as_slice()),
        (Kind::Reviews, include_bytes!("../fixtures/v15/reviews.json").as_slice()),
        (Kind::Statuses, include_bytes!("../fixtures/v15/combined-status.json").as_slice()),
        (Kind::Pull, include_bytes!("../fixtures/v15/merged-pull.json").as_slice()),
        (Kind::Pages, include_bytes!("../fixtures/v15/wiki-pages.json").as_slice()),
        (Kind::Page, include_bytes!("../fixtures/v15/wiki-page.json").as_slice()),
        (Kind::Error, include_bytes!("../fixtures/v15/missing-item.json").as_slice()),
    ] {
        response::from_bytes(body, kind, ObjectFormat::Sha1, Selection::first(limits.page), &limits)
            .unwrap_or_else(|error| panic!("{kind:?}: {error:?}"));
    }
}
#[test]
fn captured_v15_webhooks_verify_and_decode() {
    let limits = Limits::STARTING;
    for (event, header, body) in [
        (
            b"issues".as_slice(),
            b"a6e4f12a1a423063a835531db11dcd975345aaa97ec0c9e6b5aafef793fe3574".as_slice(),
            include_bytes!("../fixtures/v15/hook-0.json").as_slice(),
        ),
        (
            b"issues".as_slice(),
            b"5994e6424ddf3beea77b97fe11c628fe065be9b4f13d4def50180729e15b8e25".as_slice(),
            include_bytes!("../fixtures/v15/hook-1.json").as_slice(),
        ),
        (
            b"issue_comment".as_slice(),
            b"dd7ecb95d1f0285abfe03034ab66c2aa3f55a4379ac3e831bf67727b2e70c24c".as_slice(),
            include_bytes!("../fixtures/v15/hook-2.json").as_slice(),
        ),
        (
            b"issue_comment".as_slice(),
            b"44b344de2f0aac9adaf397d9e9434e81e88e061763425edf08d8ce83dc707124".as_slice(),
            include_bytes!("../fixtures/v15/hook-3.json").as_slice(),
        ),
        (
            b"issues".as_slice(),
            b"009356d8ffffc0518c943c69491602a2bf3a4e9a80243eb514f5c11e7e4e9d9c".as_slice(),
            include_bytes!("../fixtures/v15/hook-4.json").as_slice(),
        ),
        (
            b"issues".as_slice(),
            b"d03f616dc22dff59f71b7e5c498c5bcf4893a75dcd36afa25fd6cc42d8b1e838".as_slice(),
            include_bytes!("../fixtures/v15/hook-5.json").as_slice(),
        ),
        (
            b"pull_request".as_slice(),
            b"3ea5588bac05096d4c4d68876c54ba75aaa8d95a47e439b790428082f7e3e26d".as_slice(),
            include_bytes!("../fixtures/v15/hook-6.json").as_slice(),
        ),
        (
            b"push".as_slice(),
            b"ddf2e001980976d58b2e3056716e6ad81d998b56db28a7f28a2a5499bc8daca0".as_slice(),
            include_bytes!("../fixtures/v15/hook-7.json").as_slice(),
        ),
        (
            b"push".as_slice(),
            b"4422e0ed2bbf53858af4e558c60e8b19b371483c53e51329a7461b419d816613".as_slice(),
            include_bytes!("../fixtures/v15/hook-8.json").as_slice(),
        ),
        (
            b"pull_request".as_slice(),
            b"f8432fc8e65a3c19a7cac1531b8a79e5ba2f38a5d80b73a4145c0a0d6b163a5d".as_slice(),
            include_bytes!("../fixtures/v15/hook-9.json").as_slice(),
        ),
        (
            b"pull_request_approved".as_slice(),
            b"9b02adf1469b7d3933429dfb15dd2ca0fac3157c5acbabfcf4607bb4fa2b9cd0".as_slice(),
            include_bytes!("../fixtures/v15/hook-10.json").as_slice(),
        ),
        (
            b"pull_request".as_slice(),
            b"4c2686c2dde6f121fb95cc4fe132e77745171b79255d0ea4b37ab5ec730d64eb".as_slice(),
            include_bytes!("../fixtures/v15/hook-11.json").as_slice(),
        ),
        (
            b"wiki".as_slice(),
            b"f20b5191353ceb6eb574edb18174ceaf9ab23cae4ada7e5e32a62f62884b57d4".as_slice(),
            include_bytes!("../fixtures/v15/hook-12.json").as_slice(),
        ),
        (
            b"issues".as_slice(),
            b"2deb48ce58c699a952c82f6f2461ce6a51565396322b68c6b4d7295087849643".as_slice(),
            include_bytes!("../fixtures/v15/hook-13.json").as_slice(),
        ),
        (
            b"push".as_slice(),
            b"0bf57b8fec3ce6c10bc7adf0d90d4c619201b90f395778bde1ff0de47a7145c3".as_slice(),
            include_bytes!("../fixtures/v15/hook-14.json").as_slice(),
        ),
    ] {
        let mut signature = Signature::new(b"isolated-fixture-hook-secret", limits.hook_bytes);
        signature.update(body).unwrap();
        assert!(signature.verify(header).unwrap());
        let json = Json::from_bytes(body, &limits).unwrap();
        let mut decoder = Decoder::new(&limits);
        for token in json.tokens() {
            decoder.token(token.clone()).unwrap();
        }
        let hint = decoder.finish(event, ObjectFormat::Sha1, &limits).unwrap().unwrap();
        assert_eq!(hint.repository.as_ref(), b"fixture/specimen");
        assert!(hint.by.is_some());
    }
}

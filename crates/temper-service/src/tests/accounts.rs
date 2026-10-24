use super::fixtures;
use jig_domain_accounts::{Holder, Kind, Stored};
use jig_protocol_store::{Event as StoreEvent, SecretOwner, Secrets};
use skein_lib::{Time, Token};
use temper_engine_domain::{Record, Write};

const TOKEN: &[u8] = b"test-token";

#[test]
fn a_restored_key_is_durable_before_its_value_is_lent() {
    let mut bootstrap = fixtures::ready();
    let mut header = bootstrap.engine.as_ref().expect("engine").domain.deployment();
    header.commits = header.commits.checked_add(1).unwrap();
    let mut rows = skein_lib::List::with_capacity(4);
    rows.push(Write::Save(Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Deployment(
        header,
    )))))
    .unwrap();
    for row in [
        Stored::Account {
            number: 0,
            provider: 1,
            holder: Holder::Deployment,
            place: 0,
            uses: None,
            subject: Box::new([]),
            display: Box::new([]),
        },
        Stored::Source { account: 0, number: 1, place: 0, kind: Kind::Key, name: Box::new([]), held: None },
        Stored::Next { number: 2 },
    ] {
        rows.push(Write::Save(Record::Core(jig_domain_core::Record::Accounts(row)))).unwrap();
    }
    assert!(
        bootstrap
            .engine
            .as_mut()
            .expect("engine")
            .store
            .down(temper_engine_domain::StoreRequest::Commit { number: header.commits, writes: rows.into_boxed() })
    );
    let _kept = bootstrap.engine.as_mut().expect("engine").store.secrets(
        Token::new(crate::tokens::ACCOUNTS | 1),
        Secrets::Replace {
            owner: SecretOwner::Source,
            number: 1,
            value: skein_lib::Secret::new(TOKEN.into()),
            replaces: None,
        },
    );
    while bootstrap.engine.as_mut().expect("engine").store.above.pop().is_some() {}
    let mut service = fixtures::start_with_store(&[(1, 100)], Some(bootstrap.engine.expect("engine").store));
    for _ in 0_u32..256 {
        fixtures::turn(&mut service);
        let lent = service.engine.as_mut().expect("engine").values.accounts.value(1, 1, Time::from_nanos(1)).is_some();
        let Some(StoreEvent::Secret { value, .. }) = service
            .engine
            .as_mut()
            .expect("engine")
            .store
            .secrets(Token::new(99), Secrets::Get { owner: SecretOwner::Source, number: 1 })
        else {
            panic!("a get is answered at once");
        };
        assert!(!lent || value.is_some(), "no value is lent before its record is kept");
        if lent {
            break;
        }
    }
    let value = service
        .engine
        .as_mut()
        .expect("engine")
        .values
        .accounts
        .value(1, 1, Time::from_nanos(1))
        .expect("restored key");
    assert_eq!(value.token.expose(), TOKEN);
    assert_eq!(value.scheme, jig_protocol_accounts::table::Scheme::Key);
}

fn replaced(input: &[u8], from: &[u8], to: &[u8]) -> Box<[u8]> {
    let at = skein_lib::bytes::find(input, from).expect("fixture member");
    let end = at.checked_add(from.len()).unwrap();
    let length = input.len().checked_sub(from.len()).unwrap().checked_add(to.len()).unwrap();
    let mut out = skein_lib::Writer::new(length);
    out.put(input.get(..at).unwrap()).unwrap();
    out.put(to).unwrap();
    out.put(input.get(end..).unwrap()).unwrap();
    out.finish()
}
#[test]
fn configured_accounts_are_refused_at_their_path() {
    let document = replaced(&fixtures::document(), br#""llm": {"#, br#""llm": {"accounts": [],"#);
    let found = crate::named(&document, &jig_protocol_config::Directories::working()).unwrap_err();
    assert_eq!(&*crate::startup::lines(&found), b"engine.llm.accounts: unknown member\n");
}
#[test]
fn an_endpoint_account_is_refused_at_its_path() {
    let document = replaced(&fixtures::document(), br#""dialect": 1,"#, br#""dialect": 1,"account": 1,"#);
    let found = crate::named(&document, &jig_protocol_config::Directories::working()).unwrap_err();
    assert_eq!(&*crate::startup::lines(&found), b"engine.llm.endpoints[0].account: unknown member\n");
}

#![expect(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::wildcard_enum_match_arm,
    clippy::match_like_matches_macro,
    reason = "test scans captured diagnostics and inspects closed event variants"
)]
//! A managed Forgejo's paste is held through Reconcile and Create, then kept.
use super::create_tests::Forge;
use super::*;
use crate::setup::lifecycle;
use jig_protocol_store::{SecretOwner, Secrets};

const ADMIN: &[u8] = b"admin-token-placeholder-0123";

fn config() -> crate::settings::Engine {
    let mut config = local_config(true, false);
    let entry = &mut config.setup[0];
    entry.entry.mode = temper_forge_setup::setup::Mode::Managed { admin_token: None };
    entry.entry.directory = Box::new([]);
    entry.entry.orgs = Box::new([]);
    entry.handover = None;
    config.identities[0].login = Box::new([]);
    config
}
fn store(forge: &Forge) -> crate::store::Store {
    let mut store = fixtures::records_store();
    for (item, bytes) in [(Item::ApiToken, b"test-api".as_slice()), (Item::GitToken, b"test-git")] {
        kept_record(&mut store, 1, item, bytes);
    }
    kept_record(&mut store, 1, Item::User, &forge.service.setup().find(b"temper").unwrap().id.to_le_bytes());
    store
}
fn forge(admin: bool, marked: bool, refuse_file: bool) -> Forge {
    let mut forge = Forge::new(refuse_file);
    let setup = forge.service.setup_mut();
    let user = setup.account(b"elsewhere-owner", admin, b"");
    setup.token(
        user,
        b"pasted",
        &[b"write:admin", b"read:user", b"write:organization", b"write:repository"],
        Secret::new(ADMIN.into()),
    );
    if !marked {
        setup.delete_account(b"temper");
        setup.account(b"temper", false, b"a person's account");
    }
    forge
}
fn service(forge: &Forge) -> crate::Service {
    let mut service = local_service(config(), store(forge));
    restore_local(&mut service);
    // Fresh managed records offer Connect instead of staging a handover.
    assert_eq!(service.engine.as_ref().unwrap().domain.forge_connection(1), domain::Connection::NotConnected);
    service
}
fn bundle(item: u16, bytes: &[u8]) -> Secret {
    let mut writer = skein_lib::Writer::new(bytes.len().checked_add(6).unwrap());
    writer.put(&[1, 1]).unwrap();
    writer.put(&item.to_le_bytes()).unwrap();
    writer.put(&u16::try_from(bytes.len()).unwrap().to_le_bytes()).unwrap();
    writer.put(bytes).unwrap();
    Secret::new(writer.finish())
}
fn first(label: &[u8]) -> domain::FirstAsk {
    domain::FirstAsk {
        org: b"work".as_slice().into(),
        repository: b"hello".as_slice().into(),
        project: domain::SetUpProject::Existing(1),
        label: label.into(),
    }
}
fn ask(service: &mut crate::Service, value: Secret, first: Option<domain::FirstAsk>) {
    let engine = service.engine.as_mut().unwrap();
    let digest = skein_lib::sha256::digest(value.expose());
    engine
        .store
        .secrets(Token::new(901), Secrets::Keep { owner: SecretOwner::Application, number: records::CARRIED, value });
    assert!(
        (match engine.store.above.pop() {
            Some(StoreEvent::Kept { .. }) => true,
            _ => false,
        })
    );
    engine.step(root::Event::Forge(Box::new(domain::Event::Application {
        role: Some(jig_domain_people::Role::Owner),
        carried: Some(jig_domain_core::Carried { number: records::CARRIED, digest }),
        request: Token::new(90),
        token: Token::new(91),
        retention: Duration::from_secs(60),
        ask: domain::Ask::Connect { forge: 1, how: domain::How::ForgejoAdmin { first } },
    })));
}
fn scan(engine: &crate::engine::Engine) {
    for note in &engine.notes {
        assert!(!contains(alloc::format!("{note:?}").as_bytes(), ADMIN));
    }
    for event in &engine.events {
        assert!(!contains(alloc::format!("{event:?}").as_bytes(), ADMIN));
    }
    for answer in &engine.setup.answers {
        assert!(!contains(alloc::format!("{answer:?}").as_bytes(), ADMIN));
    }
    assert!(!contains(alloc::format!("{:?}", engine.domain.forges()).as_bytes(), ADMIN));
    for (_, plan) in &engine.setup.staged {
        assert!(
            !(match plan {
                host::Plan::TakeOver { .. } => true,
                _ => false,
            })
        );
    }
    for (_, run) in &engine.setup.runs {
        assert_ne!(run.kind, Kind::LocalTakeOver);
    }
}
fn turn(service: &mut crate::Service, forge: &mut Forge, tick: u64) {
    let engine = service.engine.as_mut().unwrap();
    scan(engine);
    while engine.notes.pop().is_some() {}
    while engine.setup.answers.pop().is_some() {}
    engine.env.now = Time::from_nanos(tick.checked_mul(1_000_000).unwrap());
    advance(engine);
    engine.forge.iterate(engine.env.now, Wall::EPOCH);
    forge.turn_io(engine);
    forge.turn_server(engine);
    for _ in 0_u32..4 {
        scan(engine);
        crate::engine::iterate(engine, engine.env.now, Wall::EPOCH);
    }
    forge.service.reclaim();
}
fn settle(service: &mut crate::Service, forge: &mut Forge) {
    for tick in 1..20_000 {
        turn(service, forge, tick);
        let engine = service.engine.as_ref().unwrap();
        if engine.setup.public.is_empty() && engine.setup.runs.is_empty() && engine.store.erases > 0 {
            scan(engine);
            return;
        }
    }
    panic!("admin connect did not settle: {:?}", service.engine.as_ref().unwrap().domain.forges());
}
fn admin_record(service: &crate::Service) -> Option<&[u8]> {
    service.engine.as_ref().unwrap().forge_records.rows.forge(1)?.get(Item::AdminToken).ok()
}
fn erased(service: &mut crate::Service, connected: bool) {
    let engine = service.engine.as_mut().unwrap();
    assert!(engine.setup.public.is_empty());
    assert!(engine.setup.runs.is_empty());
    assert_eq!(engine.store.erases, 1);
    assert!(
        (match engine
            .store
            .secrets(Token::new(902), Secrets::Get { owner: SecretOwner::Application, number: records::CARRIED })
        {
            Some(StoreEvent::Secret { value: None, .. }) => true,
            _ => false,
        })
    );
    let Some(StoreEvent::SecretsLoaded { rows, .. }) =
        engine.store.secrets(Token::new(903), Secrets::Load { owner: SecretOwner::Application, after: None })
    else {
        panic!("store rows")
    };
    for row in rows {
        if row.number == records::number(1, Item::AdminToken) && connected {
            assert!(contains(row.value.expose(), ADMIN));
        } else {
            assert!(!contains(row.value.expose(), ADMIN));
        }
    }
    if !connected {
        assert!(admin_record(service).is_none());
        assert_eq!(service.engine.as_ref().unwrap().domain.forge_connection(1), domain::Connection::NotConnected);
    }
}
#[test]
fn admin_paste_reconciles_creates_with_its_label_then_keeps_and_restarts_from_records() {
    let mut forge = forge(true, true, false);
    forge.service.setup_mut().org(b"work", b"elsewhere-owner");
    let mut service = service(&forge);
    ask(&mut service, bundle(1, ADMIN), Some(first(b"docker_1.x-y")));
    let mut saw_create = false;
    for tick in 1..20_000 {
        turn(&mut service, &mut forge, tick);
        let engine = service.engine.as_ref().unwrap();
        for (_, run) in &engine.setup.runs {
            if run.kind == Kind::AdminCreate {
                saw_create = true;
                assert!(admin_record(&service).is_none(), "Reconcile alone cannot keep the admin token");
            }
        }
        if engine.setup.public.is_empty()
            && engine.store.erases > 0
            && (match engine.domain.forge_connection(1) {
                domain::Connection::Connected { .. } => true,
                _ => false,
            })
        {
            break;
        }
    }
    assert!(saw_create);
    assert_eq!(admin_record(&service), Some(ADMIN));
    erased(&mut service, true);
    let engine = service.engine.as_ref().unwrap();
    assert!(
        (match engine.domain.forge_connection(1) {
            domain::Connection::Connected { .. } => true,
            _ => false,
        })
    );
    assert_eq!(engine.domain.forges().rows[0].first.as_ref().unwrap().state, domain::FirstState::Made { id: 1 });
    assert!(engine.domain.forges().rows[0].admin);
    assert_eq!(engine.domain.forges().rows[0].first.as_ref().unwrap().label.as_ref(), b"docker_1.x-y");
    let head = forge.git.branch(b"work/hello", b"main").unwrap();
    let object = forge.git.object(head).unwrap();
    assert!(contains(object.tree.get(temper_forge_setup::STARTER_PATH).unwrap(), b"runs-on: docker_1.x-y\n"));
    assert!(forge.service.setup().repository_hooks(1).next().is_none());
    service.engine.as_mut().unwrap().forge.down(skein_forge_connection::Request::Close);
    for _ in 0_u32..16 {
        forge.turn_io(service.engine.as_mut().unwrap());
        service.engine.as_mut().unwrap().forge.iterate(Time::ZERO, Wall::EPOCH);
    }
    let store = service.engine.take().unwrap().store;
    let mut restarted = local_service(config(), store);
    restore_local(&mut restarted);
    for tick in 1..20_000 {
        turn(&mut restarted, &mut forge, tick);
        let engine = restarted.engine.as_ref().unwrap();
        if match engine.domain.forge_connection(1) {
            domain::Connection::Connected { .. } => true,
            _ => false,
        } {
            break;
        }
    }
    let engine = restarted.engine.as_ref().unwrap();
    assert!(
        (match engine.domain.forge_connection(1) {
            domain::Connection::Connected { .. } => true,
            _ => false,
        })
    );
    assert!(engine.setup.public.is_empty());
    assert!(engine.domain.forges().rows[0].first.is_none(), "restart never repeats Create");
}
#[test]
fn admin_connect_without_first_keeps_after_reconcile_and_connects() {
    let mut forge = forge(true, true, false);
    let mut service = service(&forge);
    ask(&mut service, bundle(1, ADMIN), None);
    settle(&mut service, &mut forge);
    assert_eq!(admin_record(&service), Some(ADMIN));
    erased(&mut service, true);
    assert!(forge.service.setup().organization(b"work").is_none());
}
#[test]
fn bad_admin_bundle_erases_without_keeping_a_token() {
    for value in [bundle(11, ADMIN), bundle(1, b""), Secret::new(ADMIN.into())] {
        let mut forge = forge(true, true, false);
        let mut service = service(&forge);
        ask(&mut service, value, None);
        settle(&mut service, &mut forge);
        erased(&mut service, false);
    }
}
#[test]
fn refused_admin_token_erases_without_keeping_it() {
    let mut forge = forge(true, true, false);
    let mut service = service(&forge);
    forge.service.setup_mut().revoke(ADMIN);
    ask(&mut service, bundle(1, ADMIN), None);
    settle(&mut service, &mut forge);
    erased(&mut service, false);
    assert_eq!(
        service.engine.as_ref().unwrap().domain.forges().rows[0].ended_stop,
        Some(domain::AdminStop::Refused { status: 401, stage: domain::SetupStage::Admin })
    );
    assert!(contains(
        service.engine.as_ref().unwrap().domain.forges().rows[0].ended.as_ref().unwrap(),
        b"Refused(401)"
    ));
}
#[test]
fn not_admin_erases_without_keeping_the_token() {
    let mut forge = forge(false, true, false);
    let mut service = service(&forge);
    ask(&mut service, bundle(1, ADMIN), None);
    settle(&mut service, &mut forge);
    erased(&mut service, false);
    assert_eq!(service.engine.as_ref().unwrap().domain.forges().rows[0].ended_stop, Some(domain::AdminStop::NotAdmin));
    assert!(contains(service.engine.as_ref().unwrap().domain.forges().rows[0].ended.as_ref().unwrap(), b"NotAdmin"));
}
#[test]
fn unmarked_bot_erases_without_keeping_the_token() {
    let mut forge = forge(true, false, false);
    let mut service = service(&forge);
    ask(&mut service, bundle(1, ADMIN), None);
    settle(&mut service, &mut forge);
    erased(&mut service, false);
    assert_eq!(service.engine.as_ref().unwrap().domain.forges().rows[0].ended_stop, Some(domain::AdminStop::Unmarked));
    assert!(contains(service.engine.as_ref().unwrap().domain.forges().rows[0].ended.as_ref().unwrap(), b"Unmarked"));
}
#[test]
fn create_file_stop_marks_first_failed_and_erases_without_keeping_the_token() {
    let mut forge = forge(true, true, true);
    forge.service.setup_mut().org(b"work", b"elsewhere-owner");
    let mut service = service(&forge);
    ask(&mut service, bundle(1, ADMIN), Some(first(b"docker")));
    settle(&mut service, &mut forge);
    erased(&mut service, false);
    assert_eq!(
        service.engine.as_ref().unwrap().domain.forges().rows[0].ended_stop,
        Some(domain::AdminStop::Refused { status: 413, stage: domain::SetupStage::File })
    );
    assert_eq!(
        service.engine.as_ref().unwrap().domain.forges().rows[0].first.as_ref().unwrap().state,
        domain::FirstState::Failed { step: domain::CreateStep::File }
    );
}
#[test]
fn restart_without_an_admin_record_offers_connect_and_sweeps_a_crashed_carry() {
    let forge = forge(true, true, false);
    let mut store = store(&forge);
    store.secrets(
        Token::new(901),
        Secrets::Keep { owner: SecretOwner::Application, number: records::CARRIED, value: bundle(1, ADMIN) },
    );
    assert!(
        (match store.above.pop() {
            Some(StoreEvent::Kept { .. }) => true,
            _ => false,
        })
    );
    let mut service = local_service(config(), store);
    restore_local(&mut service);
    erased(&mut service, false);
}
#[test]
fn both_admin_plans_read_the_paste_even_with_different_records_and_adoptions() {
    let forge = forge(true, true, false);
    let mut service = service(&forge);
    let engine = service.engine.as_mut().unwrap();
    row_placeholder(engine);
    engine
        .setup
        .lifecycle
        .adoptions
        .insert(
            1,
            lifecycle::Adoption {
                next: core::num::NonZeroU64::new(1).unwrap(),
                repositories: 1,
                files: Some(lifecycle::AdoptionFiles {
                    admin_token: Secret::new(b"different-adoption-token".as_slice().into()),
                    access: skein_forge_connection::Access {
                        credential: skein_forge::credential::Credential::Token(Secret::new(
                            b"different-api-token".as_slice().into(),
                        )),
                        user: 3,
                        webhook_secret: None,
                    },
                }),
            },
        )
        .unwrap();
    let carried = {
        let value = bundle(1, ADMIN);
        let digest = skein_lib::sha256::digest(value.expose());
        engine.store.secrets(
            Token::new(901),
            Secrets::Keep { owner: SecretOwner::Application, number: records::CARRIED, value },
        );
        assert!(
            (match engine.store.above.pop() {
                Some(StoreEvent::Kept { .. }) => true,
                _ => false,
            })
        );
        jig_domain_core::Carried { number: records::CARRIED, digest }
    };
    public::admin_connect(engine, 1, Token::new(90), Some(first(b"docker")), carried);
    let run = *engine.setup.runs.first().unwrap().0;
    for kind in [Kind::AdminReconcile, Kind::AdminCreate] {
        engine.setup.runs.get_mut(&run).unwrap().kind = kind;
        let value = read_item(engine, run, 1, skein_forge_setup::item::Item::AdminToken).unwrap();
        assert_eq!(value.expose(), ADMIN);
        assert!(read_item(engine, run, 1, skein_forge_setup::item::Item::Pasted).is_none());
    }
}
fn row_placeholder(engine: &mut crate::engine::Engine) {
    engine.forge_records.rows.push(
        records::number(1, Item::AdminToken),
        Secret::new(records::encode(b"different-record-token").into_boxed_slice()),
        4096,
    );
}
#[test]
fn runner_labels_are_checked_again_before_yaml_is_made() {
    for label in [b"".as_slice(), b"x\nruns-on: hostile", b"x: y", &[b'a'; 65]] {
        assert!(temper_forge_setup::starter_workflow(label).is_none());
    }
    for label in [b"docker".as_slice(), b"._-", &[b'a'; 64]] {
        assert!(temper_forge_setup::starter_workflow(label).is_some());
    }
    let mut forge = forge(true, true, false);
    let mut service = service(&forge);
    ask(&mut service, bundle(1, ADMIN), Some(first(b"bad\nlabel")));
    settle(&mut service, &mut forge);
    erased(&mut service, false);
    assert!(forge.service.setup().organization(b"work").is_none());
}

#[test]
fn admin_create_makes_a_missing_organization_owned_by_the_verified_admin() {
    let mut forge = forge(true, true, false);
    let mut service = service(&forge);
    ask(&mut service, bundle(1, ADMIN), Some(first(b"docker")));
    settle(&mut service, &mut forge);
    assert_eq!(admin_record(&service), Some(ADMIN));
    assert_eq!(forge.service.setup().organization(b"work").unwrap().owner.as_ref(), b"elsewhere-owner");
}

fn disk_turn(store: &mut crate::store::Store, kernel: &mut skein_shell::Kernel) {
    store.iterate();
    let wait = if kernel.in_flight() > 0 || !store.submitted.is_empty() {
        skein_shell::Wait::Forever
    } else {
        skein_shell::Wait::No
    };
    kernel.submit(&mut store.submitted, wait);
    let mut completions = Queue::with_capacity(64);
    kernel.reap(&mut completions);
    while let Some(completion) = completions.pop() {
        store.complete(completion);
    }
}
#[test]
fn admin_record_files_and_directories_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = skein_scratch::Scratch::new_in(&std::env::current_dir().unwrap(), "admin-record-modes");
    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let (root, core, profile) = fixtures::store_limits();
    let limits = crate::store::limits(&root, &core, &root.forge, &profile).unwrap();
    let mut store = crate::store::Store::new(&root, &limits, &[], fixtures::seeds().installation);
    let fd = skein_shell::open_root(scratch.path()).unwrap();
    store.adopt(fd, skein_shell::effective_user()).unwrap();
    let mut kernel =
        skein_shell::Kernel::open_on(skein_shell::Config { operations: 64 }, skein_shell::Backend::Readiness).unwrap();
    let mut opened = false;
    for _ in 0_u32..4096 {
        disk_turn(&mut store, &mut kernel);
        if let Some(event) = store.above.pop() {
            assert!(
                (match event {
                    StoreEvent::Opened { .. } => true,
                    _ => false,
                }),
                "{event:?}"
            );
            opened = true;
            break;
        }
    }
    assert!(opened);
    store.secrets(
        Token::new(901),
        Secrets::Keep {
            owner: SecretOwner::Application,
            number: records::number(1, Item::AdminToken),
            value: Secret::new(records::encode(ADMIN).into_boxed_slice()),
        },
    );
    let mut kept = false;
    for _ in 0_u32..4096 {
        disk_turn(&mut store, &mut kernel);
        if let Some(event) = store.above.pop() {
            assert!(
                (match event {
                    StoreEvent::Kept { .. } => true,
                    _ => false,
                }),
                "{event:?}"
            );
            kept = true;
            break;
        }
    }
    assert!(kept);
    let mut paths = Queue::with_capacity(64);
    paths.push(scratch.path().join("generations/1/secrets"));
    let mut files = 0_u32;
    while let Some(path) = paths.pop() {
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(!metadata.is_symlink());
        if metadata.is_dir() {
            assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
            for entry in std::fs::read_dir(&path).unwrap() {
                paths.push(entry.unwrap().path());
            }
        } else {
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            files = files.checked_add(1).unwrap();
        }
    }
    assert!(files > 0);
    store.close();
    for _ in 0_u32..4096 {
        disk_turn(&mut store, &mut kernel);
        if store.closed() {
            break;
        }
    }
    assert!(store.closed());
}

fn pending(service: &mut crate::Service) -> Token {
    ask(service, bundle(1, ADMIN), None);
    for _ in 0_u32..128 {
        let engine = service.engine.as_mut().unwrap();
        scan(engine);
        crate::engine::iterate(engine, Time::ZERO, Wall::EPOCH);
        if let Some((run, _)) = engine.setup.public.first() {
            return *run;
        }
    }
    panic!("admin run admitted");
}
#[test]
fn busy_admin_connect_erases_its_unused_paste() {
    let forge = forge(true, true, false);
    let mut service = service(&forge);
    let _run = pending(&mut service);
    let engine = service.engine.as_mut().unwrap();
    let value = bundle(1, ADMIN);
    let digest = skein_lib::sha256::digest(value.expose());
    let number = records::CARRIED.checked_add(1).unwrap();
    engine.store.secrets(Token::new(901), Secrets::Keep { owner: SecretOwner::Application, number, value });
    assert!(
        (match engine.store.above.pop() {
            Some(StoreEvent::Kept { .. }) => true,
            _ => false,
        })
    );
    public::admin_connect(engine, 1, Token::new(92), None, jig_domain_core::Carried { number, digest });
    assert_eq!(engine.store.erases, 1);
    assert!(
        (match engine.store.secrets(Token::new(902), Secrets::Get { owner: SecretOwner::Application, number }) {
            Some(StoreEvent::Secret { value: None, .. }) => true,
            _ => false,
        })
    );
    assert!(engine.events.iter().any(|event| match event {
        root::Event::Forge(event)
            if match event.as_ref() {
                domain::Event::ConnectAnswered {
                    answer: domain::ConnectAnswer::Refused(domain::ForgeRefusal::Busy),
                    ..
                } => true,
                _ => false,
            } =>
            true,
        _ => false,
    }));
    assert!(admin_record(&service).is_none());
}
#[test]
fn a_failed_keep_ends_the_admin_caller_and_erases_before_the_engine_stops() {
    let forge = forge(true, true, false);
    let mut service = service(&forge);
    let run = pending(&mut service);
    let engine = service.engine.as_mut().unwrap();
    lifecycle::keep(engine, 1, Item::Origin, b"placeholder-origin", Purpose::Host(run));
    let Some(StoreEvent::Kept { owner }) = engine.store.above.pop() else { panic!("pending keep") };
    answer(engine, StoreEvent::Corrupt { owner });
    assert!(engine.end.is_some());
    assert!(engine.setup.public.is_empty());
    assert!(engine.setup.runs.is_empty());
    assert!(engine.setup.writes.is_empty());
    assert_eq!(engine.store.erases, 1);
    scan(engine);
    assert!(engine.events.iter().any(|event| match event {
        root::Event::Forge(event)
            if match event.as_ref() {
                domain::Event::ConnectEnded { connected: false, .. } => true,
                _ => false,
            } =>
            true,
        _ => false,
    }));
    assert!(
        (match engine
            .store
            .secrets(Token::new(902), Secrets::Get { owner: SecretOwner::Application, number: records::CARRIED })
        {
            Some(StoreEvent::Secret { value: None, .. }) => true,
            _ => false,
        })
    );
    crate::engine::iterate(engine, Time::ZERO, Wall::EPOCH);
    assert!(admin_record(&service).is_none());
}
#[test]
fn an_ended_forge_finishes_the_admin_run_without_keeping_the_token() {
    let forge = forge(true, true, false);
    let mut service = service(&forge);
    let run = pending(&mut service);
    let engine = service.engine.as_mut().unwrap();
    engine.demo = Some(crate::settings::Demo {
        project: 1,
        forge: 1,
        provider: 1,
        owner: b"owner".as_slice().into(),
        name: b"ended".as_slice().into(),
        cues: Box::new([]),
        landing: b"main".as_slice().into(),
        key_file: Box::new([]),
        state: Some(domain::DemoState::Ended),
    });
    // Replace the fake host's pending read with its terminal, so no stale Need follows Done.
    while engine.setup.above.pop().is_some() {}
    engine.setup.above.push(host::Event::Done { run, outcome: reconcile::Outcome::Nothing });
    advance(engine);
    assert!(engine.setup.public.is_empty());
    assert!(engine.setup.runs.is_empty());
    scan(engine);
    // Run the real root's end and erase decision; the fake host still has no HTTP reply.
    for _ in 0_u32..64 {
        crate::engine::iterate(engine, Time::ZERO, Wall::EPOCH);
    }
    erased(&mut service, false);
}
#[test]
fn a_github_app_paste_cannot_decode_an_admin_bundle() {
    let mut service = routing_service(4, Duration::from_secs(60));
    let engine = service.engine.as_mut().unwrap();
    let value = bundle(1, ADMIN);
    let digest = skein_lib::sha256::digest(value.expose());
    engine
        .store
        .secrets(Token::new(901), Secrets::Keep { owner: SecretOwner::Application, number: records::CARRIED, value });
    assert!(
        (match engine.store.above.pop() {
            Some(StoreEvent::Kept { .. }) => true,
            _ => false,
        })
    );
    public::connect(
        engine,
        1,
        Token::new(90),
        core::num::NonZeroU64::new(7).unwrap(),
        jig_domain_core::Carried { number: records::CARRIED, digest },
    );
    assert!(engine.setup.public.is_empty());
    assert!(engine.setup.runs.is_empty());
    scan(engine);
    assert!(engine.events.iter().any(|event| match event {
        root::Event::Forge(event)
            if match event.as_ref() {
                domain::Event::ConnectEnded { connected: false, why: Some(why), .. }
                    if why.as_ref() == b"the pasted value is not a key" =>
                    true,
                _ => false,
            } =>
            true,
        _ => false,
    }));
}

#[test]
fn reconcile_and_create_use_the_paste_instead_of_an_older_admin_record() {
    let mut forge = forge(true, true, false);
    forge.service.setup_mut().org(b"work", b"elsewhere-owner");
    let mut service = service(&forge);
    row_placeholder(service.engine.as_mut().unwrap());
    ask(&mut service, bundle(1, ADMIN), Some(first(b"docker")));
    settle(&mut service, &mut forge);
    assert_eq!(admin_record(&service), Some(ADMIN));
    assert_eq!(
        service.engine.as_ref().unwrap().domain.forges().rows[0].first.as_ref().unwrap().state,
        domain::FirstState::Made { id: 1 }
    );
    erased(&mut service, true);
}

#[test]
fn string_managed_serves_no_listener_even_when_owned_forges_have_webhooks() {
    let forge = forge(true, true, false);
    let mut config = config();
    config.webhooks =
        Some(skein_forge_connection::hooks::Webhooks { listener: skein_io::Addr::Ip(([127, 0, 0, 1], 7101).into()) });
    let service = local_service(config, store(&forge));
    let engine = service.engine.as_ref().unwrap();
    let (settings, _) = engine.setup.pending.as_ref().unwrap();
    let host::Kind::Forgejo(given) = &settings.forges.get(0).unwrap().kind else { panic!("Forgejo") };
    assert!(given.listener.is_none());
    assert!(given.orgs.is_empty());
    assert!(given.org_owner.is_none());
    assert!(!engine.setup.waiting_for_webhooks);
    assert!(!engine.setup.lifecycle.local(1).unwrap().webhooks);
}

#[test]
fn admin_scope_refusals_reach_the_row_at_the_failed_stage() {
    for (scopes, missing_bot, stage) in [
        (b"read:issue".as_slice(), false, domain::SetupStage::Admin),
        (b"read:user".as_slice(), true, domain::SetupStage::Bot),
    ] {
        let mut forge = forge(true, true, false);
        let setup = forge.service.setup_mut();
        setup.revoke(ADMIN);
        let user = setup.find(b"elsewhere-owner").unwrap().id;
        setup.token(user, b"limited", &[scopes], Secret::new(ADMIN.into()));
        let mut service = service(&forge);
        if missing_bot {
            forge.service.setup_mut().delete_account(b"temper");
        }
        ask(&mut service, bundle(1, ADMIN), None);
        settle(&mut service, &mut forge);
        erased(&mut service, false);
        assert_eq!(
            service.engine.as_ref().unwrap().domain.forges().rows[0].ended_stop,
            Some(domain::AdminStop::Refused { status: 403, stage })
        );
    }
}

#[test]
fn admin_connect_refuses_a_first_organization_named_as_the_bot() {
    let mut forge = forge(true, true, false);
    let mut service = service(&forge);
    let mut first = first(b"docker");
    first.org = b"TeMpEr".as_slice().into();
    ask(&mut service, bundle(1, ADMIN), Some(first));
    // An admitted connect would be Connecting; admission's CarriedUnused goes to jig, not the store.
    assert_eq!(service.engine.as_ref().unwrap().domain.forge_connection(1), domain::Connection::NotConnected);
    for tick in 1..64 {
        turn(&mut service, &mut forge, tick);
        let engine = service.engine.as_ref().unwrap();
        assert!(engine.setup.staged.is_empty() && engine.setup.public.is_empty() && engine.setup.runs.is_empty());
    }
    let engine = service.engine.as_ref().unwrap();
    assert_eq!(engine.domain.forge_connection(1), domain::Connection::NotConnected);
    assert!(engine.domain.forges().rows[0].first.is_none());
    assert!(forge.service.setup().organization(b"TeMpEr").is_none());
    assert!(admin_record(&service).is_none());
}

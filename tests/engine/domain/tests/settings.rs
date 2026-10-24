//! Settings cross the root's journal and real store, including cold restoration.
use jig_domain_core::{self as core, settings as s};
use jig_domain_people as people;
use skein_lib::{Duration, ReplyTo, Token};
use temper_engine_domain::{self as engine, Delivery, Event, Key, PartyInput, Record, Write};
use temper_engine_domain_world::{direct::Driver, store::Store, walking};

const SEED: u64 = 9761;
// Restoration keeps two archived revisions; pruning keeps one (jig's domain/engine.md, 17.7).
const HISTORY: u32 = 2;
const PRUNED_HISTORY: u32 = 1;
const OWNER_PERSON: u64 = 1;
const OTHER: u64 = 8;

fn started(history: u32) -> Driver {
    let mut limits = walking::limits();
    limits.settings.history = history;
    let mut driver = Driver::cold(Store::new(), walking::config(SEED), &limits);
    driver.settle();
    driver.sign_in();
    driver.settle();
    driver
}
fn rule_value(minimum: u64) -> s::Value {
    let mut rules = walking::config(SEED).core.defaults.rules;
    rules.minimum_run_spend = minimum;
    s::Value::Rules(Box::new(rules))
}
fn rules(minimum: u64) -> s::Item {
    s::Item { setting: s::Setting::Rules, edit: s::Edit::Set(rule_value(minimum)) }
}
fn change(driver: &mut Driver, against: u32, key: u8, item: s::Item) {
    driver.send(Event::Party(PartyInput::ChangeSettings {
        reply_to: ReplyTo::new(Token::new(u64::from(key))),
        sign_in: driver.session(),
        key: [key; 16],
        digest: [key; 32],
        change: Box::new(s::Change { against, items: Box::new([item]), running: s::Running::Stays }),
    }));
    driver.settle();
}
fn head(driver: &Driver) -> &s::Head {
    driver
        .store
        .rows
        .values()
        .find_map(|row| {
            if let Record::Core(core::Record::Core(core::CoreRecord::Settings(s::Record::Revision {
                history: false,
                revision,
            }))) = row
            {
                Some(&revision.head)
            } else {
                None
            }
        })
        .expect("newest held revision")
}
fn read_rules(driver: &mut Driver, revision: u32) -> core::reads::SettingDocument {
    driver.send(Event::Party(PartyInput::Read {
        reply_to: ReplyTo::new(Token::new(990)),
        sign_in: driver.session(),
        read: core::reads::Read::Setting { setting: s::Setting::Rules, revision },
    }));
    driver.settle();
    driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            if let Delivery::Read { answer: core::reads::Answer::Setting(document), .. } = delivery {
                Some(*document.clone())
            } else {
                None
            }
        })
        .expect("current setting read")
}
#[test]
fn an_owner_change_commits_and_the_next_read_serves_it() {
    let mut driver = started(HISTORY);
    change(&mut driver, 1, 1, rules(2));
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply { reply: people::Reply::Outcome(people::Outcome::SettingsChanged { revision: 2 }), .. }
    )));
    assert_eq!(head(&driver).by, s::Decider::Party(OWNER_PERSON)); // First issued person, independent of their identity subject.
    assert_eq!(read_rules(&mut driver, 2).value, rule_value(2));
}
#[test]
fn a_nonowner_cannot_change_the_deployments_settings() {
    let mut driver = started(HISTORY);
    driver.delivered.clear();
    driver.send(Event::Party(PartyInput::SignedIn {
        reply_to: ReplyTo::new(Token::new(991)),
        held: None,
        identity: people::Identity {
            key: people::IdentityKey { provider: 0, subject: OTHER.to_be_bytes().into() },
            login: Box::new([]),
            name: Box::new([]),
        },
    }));
    driver.settle();
    change(&mut driver, 1, 2, rules(2));
    assert_eq!(head(&driver).number, 1);
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply {
            reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Role { .. }))
                | people::Reply::Refused(people::Refusal::Role { .. }),
            ..
        }
    )));
}
#[test]
fn a_stale_owner_change_names_the_newest_revision() {
    let mut driver = started(HISTORY);
    change(&mut driver, 1, 3, rules(2));
    change(&mut driver, 1, 4, rules(3));
    assert!(driver.delivered.iter().any(|delivery| matches!(
        delivery,
        Delivery::WebReply {
            reply: people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Stale { newest: 2 })),
            ..
        }
    )));
    assert_eq!(head(&driver).number, 2);
}
#[test]
fn changed_settings_restore_before_accounts_and_survive_a_restart() {
    let mut driver = started(HISTORY);
    change(&mut driver, 1, 5, rules(2));
    let mut restarted = Driver::cold(driver.store, walking::config(SEED), &walking::limits());
    restarted.settle();
    restarted.sign_in();
    restarted.settle();
    let mut ranges: Vec<_> = restarted.loads.iter().map(|(_, range)| range.clone()).collect();
    ranges.dedup(); // Settings can span several pages before the next range begins.
    assert_eq!(
        ranges.into_iter().take(3).collect::<Vec<_>>(),
        vec![
            engine::Range::Core(engine::CoreRange::Deployment),
            engine::Range::Core(engine::CoreRange::Settings),
            engine::Range::Core(engine::CoreRange::Accounts)
        ]
    );
    assert_eq!(read_rules(&mut restarted, 2).value, rule_value(2));
}
#[test]
fn changed_release_defaults_record_a_revision_once() {
    let driver = started(HISTORY);
    let mut config = walking::config(SEED);
    config.core.defaults.rules.minimum_run_spend = 2;
    let mut released = Driver::cold(driver.store, config, &walking::limits());
    released.settle();
    assert_eq!((head(&released).number, head(&released).by), (2, s::Decider::Release));
    let mut config = walking::config(SEED);
    config.core.defaults.rules.minimum_run_spend = 2;
    let mut unchanged = Driver::cold(released.store, config, &walking::limits());
    unchanged.settle();
    assert_eq!((head(&unchanged).number, head(&unchanged).by), (2, s::Decider::Release));
    assert!(
        !unchanged.transactions.iter().flatten().any(|write| matches!(
            write,
            Write::Save(Record::Core(core::Record::Core(core::CoreRecord::Settings(s::Record::Revision { .. }))))
        )),
        "an unchanged start records no revision"
    );
}
#[test]
fn a_revision_copy_precedes_its_erase_in_the_same_commit() {
    let mut driver = started(PRUNED_HISTORY);
    change(&mut driver, 1, 6, rules(2));
    change(&mut driver, 2, 7, rules(3));
    let commit = driver
        .transactions
        .iter()
        .find(|writes| writes.contains(&Write::CopySettingsRevision(2)))
        .expect("second revision archive");
    let copy =
        commit.iter().position(|write| *write == Write::CopySettingsRevision(2)).expect("copy in the same commit");
    let erase = commit
        .iter()
        .position(|write| *write == Write::Erase(Key::Core(core::Key::Core(core::CoreKey::Settings(s::Key::Held(2))))))
        .expect("held erase in the same commit");
    assert!(copy < erase);
    assert_eq!(
        Write::CopySettingsRevision(2).key(),
        Key::Core(core::Key::Core(core::CoreKey::Settings(s::Key::History(2))))
    );
    assert!(driver.store.rows.contains_key(&Key::Core(core::Key::Core(core::CoreKey::Settings(s::Key::History(2))))));
    assert!(!driver.store.rows.contains_key(&Key::Core(core::Key::Core(core::CoreKey::Settings(s::Key::History(1))))));
}
#[test]
fn a_stored_change_that_no_longer_checks_refuses_the_start() {
    let mut driver = started(HISTORY);
    let mut template = walking::config(SEED).core.defaults.templates[0].clone();
    template.grace = Some(Duration::from_secs(4));
    change(
        &mut driver,
        1,
        8,
        s::Item { setting: s::Setting::Template(1), edit: s::Edit::Set(s::Value::Template(Box::new(template))) },
    );
    let mut limits = walking::limits();
    limits.tasks.grace = Duration::from_secs(3);
    let mut restarted = Driver::cold(driver.store, walking::config(SEED), &limits);
    restarted.pump();
    assert!(restarted.stopped);
    assert_eq!(
        restarted.root.settings_failure(),
        Some(core::RestartRequest::Settings {
            failure: s::Failure { setting: s::Setting::Template(1), fault: s::Fault::Grace },
            changed: true
        })
    );
}

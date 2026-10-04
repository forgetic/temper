//! The referee rejects duplicate side effects across engine restarts.

use skein_lib::{Duration, Time};
use temper_fake_forge_domain::Observation;
use temper_fake_forge_domain::api::Kind;
use temper_legacy_engine_domain::{Outcome, Posted};
use temper_legacy_engine_domain_world::codec;
use temper_legacy_engine_domain_world::deployment::{ENGINE, REPOSITORIES};
use temper_legacy_engine_domain_world::referee::{Bounds, Engine, Seen};
use temper_world::{Referee, Verdict};

fn referee() -> Referee<Engine> {
    let bounds = Bounds { story: Duration::from_secs(60), message: Duration::from_secs(60) };
    Referee::new(Engine::new(bounds, 0))
}

fn opened(number: u64) -> Observation {
    Observation::Opened {
        repository: REPOSITORIES[0].into(),
        number,
        kind: Kind::Issue,
        title: b"task".as_slice().into(),
        body: temper_legacy_engine_forge_world::translate::keyed(b"key", b"task").into_boxed_slice(),
        labels: Box::new([]),
        branches: None,
        by: ENGINE,
    }
}

#[test]
fn a_restart_cannot_make_a_duplicate_issue_acceptable() {
    let mut referee = referee();
    let mut out = Vec::new();
    referee.observe(Time::ZERO, Seen::Forge(opened(1)), &mut out);
    assert_eq!(referee.verdict(), Verdict::Passed);
    referee.observe(Time::ZERO, Seen::Restarted, &mut out);
    referee.observe(Time::ZERO, Seen::Forge(opened(2)), &mut out);
    let Verdict::Failed(failure) = referee.verdict() else { panic!("a second keyed issue must fail") };
    assert!(failure.why.contains("an issue is created once per key"));
}

fn duplicate_comment(body: &[u8], reason: &str) {
    let mut referee = referee();
    let mut out = Vec::new();
    referee.observe(Time::ZERO, Seen::Forge(opened(1)), &mut out);
    let commented = |id| Observation::Commented {
        repository: REPOSITORIES[0].into(),
        number: 1,
        id,
        body: body.into(),
        by: ENGINE,
    };
    referee.observe(Time::ZERO, Seen::Forge(commented(1)), &mut out);
    assert_eq!(referee.verdict(), Verdict::Passed);
    referee.observe(Time::ZERO, Seen::Restarted, &mut out);
    referee.observe(Time::ZERO, Seen::Forge(commented(2)), &mut out);
    let Verdict::Failed(failure) = referee.verdict() else { panic!("a second comment must fail") };
    assert!(failure.why.contains(reason), "{}", failure.why);
}

#[test]
fn a_restart_cannot_make_a_duplicate_keyed_comment_acceptable() {
    duplicate_comment(
        &temper_legacy_engine_forge_world::translate::keyed(b"message", b"hello"),
        "a keyed comment is posted once",
    );
}

#[test]
fn a_restart_cannot_make_a_duplicate_outcome_acceptable() {
    let posted = Posted { attempt: 1, outcome: Outcome::Finished { text: b"done".as_slice().into() }, head: None };
    duplicate_comment(
        &temper_legacy_engine_forge_world::translate::keyed(b"outcome", &codec::posted_block(&posted)),
        "an attempt's outcome is posted once",
    );
}

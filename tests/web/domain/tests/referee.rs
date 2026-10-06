use skein_lib::{Duration, Time};
use temper_web_domain::{Address, Key};
use temper_web_domain_world::referee::{Rules, Seen, observe_at};
use temper_world::{Referee, Verdict};

fn key(byte: u8) -> Key {
    Key([byte; 16])
}
fn failed(referee: &Referee<Rules>) -> bool {
    matches!(referee.verdict(), Verdict::Failed(_))
}

#[test]
fn rejects_visible_before_durable() {
    let mut referee = Referee::new(Rules::new());
    let mut out = Vec::new();
    referee.observe(
        Time::ZERO,
        Seen::Visible {
            address: Address::Task { number: 4, section: None },
            link_live: false,
            watch_live: false,
            refused: false,
        },
        &mut out,
    );
    assert!(failed(&referee));
}

#[test]
fn rejects_two_keys_making_one_task() {
    let mut referee = Referee::new(Rules::new());
    observe_at(&mut referee, Time::ZERO, Seen::Durable { key: key(1), task: 4 });
    let mut out = Vec::new();
    referee.observe(Time::ZERO, Seen::Durable { key: key(2), task: 4 }, &mut out);
    assert!(failed(&referee));
}

#[test]
fn rejects_lost_unsent_words() {
    let mut referee = Referee::new(Rules::new());
    let mut out = Vec::new();
    referee.observe(Time::ZERO, Seen::Reloaded { before: b"draft".to_vec(), after: Vec::new() }, &mut out);
    assert!(failed(&referee));
}

#[test]
fn rejects_silent_refusal() {
    let mut referee = Referee::new(Rules::new());
    let mut out = Vec::new();
    referee.observe(Time::ZERO, Seen::Refused { shown: false }, &mut out);
    assert!(failed(&referee));
}

#[test]
fn rejects_false_live_link() {
    let mut referee = Referee::new(Rules::new());
    let mut out = Vec::new();
    referee.observe(
        Time::ZERO,
        Seen::Visible { address: Address::Chats, link_live: true, watch_live: false, refused: false },
        &mut out,
    );
    assert!(failed(&referee));
}

#[test]
fn unanswered_press_fails_its_deadline() {
    let mut referee = Referee::new(Rules::new());
    observe_at(&mut referee, Time::ZERO, Seen::Submitted { key: key(1) });
    let mut out = Vec::new();
    referee.fire(Time::ZERO.saturating_add(Duration::from_secs(2)), &mut out);
    assert!(failed(&referee));
}

#[test]
fn unrecovered_watch_fails_its_deadline() {
    let mut referee = Referee::new(Rules::new());
    observe_at(&mut referee, Time::ZERO, Seen::WatchLost);
    let mut out = Vec::new();
    referee.fire(Time::ZERO.saturating_add(Duration::from_secs(2)), &mut out);
    assert!(failed(&referee));
}

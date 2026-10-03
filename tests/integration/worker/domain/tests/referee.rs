//! The referee of the whole worker's world, fed observations by hand as the
//! world would feed them, fails a run that breaks an expectation, and says
//! why; and passes one that keeps them.

use temper_engine_domain::work::{Class, Failures, Lifecycle, Phase};
use temper_lib::{Duration, Time, Token};
use temper_worker_domain_tests::protocol::Names;
use temper_worker_domain_tests::referee::{Hosting, Seen, Stimulus};
use temper_world::{Referee, Verdict};

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn referee() -> Referee<Hosting> {
    Referee::new(Hosting::new(Duration::from_secs(60), vec![Duration::from_secs(30)], 1))
}

fn see(referee: &mut Referee<Hosting>, secs: u64, seen: Seen) {
    referee.observe(at(secs), seen, &mut Vec::new());
}

fn why(referee: &Referee<Hosting>) -> String {
    let Verdict::Failed(failure) = referee.verdict() else { panic!("the referee failed the run") };
    failure.why
}

const NAMES: Names = (Token::new(1), Token::new(3));

/// The item 1's record, at its attempt `attempts`, with `failures`.
fn recorded(phase: Phase, attempts: u64, failures: Failures) -> Seen {
    Seen::Recorded { repository: b"ai/one".to_vec(), number: 1, lifecycle: Lifecycle { phase, attempts, failures } }
}

#[test]
fn an_answer_taken_and_acknowledged_passes() {
    let mut referee = referee();
    see(&mut referee, 1, Seen::Answered { names: NAMES, refused: false });
    see(&mut referee, 2, Seen::Answered { names: NAMES, refused: false });
    see(&mut referee, 3, Seen::Acknowledged { names: NAMES });
    referee.assert_passed(0);
}

#[test]
fn an_answer_not_acknowledged_in_time_fails() {
    let mut referee = referee();
    see(&mut referee, 1, Seen::Answered { names: NAMES, refused: false });
    referee.fire(at(62), &mut Vec::new());
    assert!(why(&referee).contains("was not met"), "{}", why(&referee));
}

#[test]
fn an_acknowledgement_for_an_answer_that_never_came_fails() {
    let mut referee = referee();
    see(&mut referee, 1, Seen::Acknowledged { names: NAMES });
    assert!(why(&referee).contains("only an answer that reached it"), "{}", why(&referee));
}

/// A refusal that came twice, an assignment sent again and refused again,
/// the engine may acknowledge as an answer for an attempt it has finished
/// with.
#[test]
fn an_acknowledged_refusal_passes() {
    let mut referee = referee();
    see(&mut referee, 1, Seen::Answered { names: NAMES, refused: true });
    see(&mut referee, 1, Seen::Answered { names: NAMES, refused: true });
    see(&mut referee, 2, Seen::Acknowledged { names: NAMES });
    referee.assert_passed(0);
}

#[test]
fn a_refusal_is_not_acknowledged() {
    let mut referee = referee();
    see(&mut referee, 1, Seen::Answered { names: NAMES, refused: true });
    referee.fire(at(120), &mut Vec::new());
    referee.assert_passed(0);
}

/// The channel lost, or the engine restarting, withdraws the bound; the
/// next hello that lists the answer arms it again.
#[test]
fn an_answer_is_acknowledged_within_the_bound_of_the_hello_that_lists_it_again() {
    for loss in [Seen::Lost, Seen::Restarted] {
        let mut referee = referee();
        see(&mut referee, 1, Seen::Answered { names: NAMES, refused: false });
        see(&mut referee, 30, loss);
        referee.fire(at(300), &mut Vec::new());
        see(&mut referee, 300, Seen::Hello { answered: vec![NAMES] });
        see(&mut referee, 340, Seen::Acknowledged { names: NAMES });
        referee.assert_passed(0);
    }
}

#[test]
fn an_answer_listed_again_and_not_acknowledged_in_time_fails() {
    let mut referee = referee();
    see(&mut referee, 1, Seen::Answered { names: NAMES, refused: false });
    see(&mut referee, 30, Seen::Lost);
    see(&mut referee, 300, Seen::Hello { answered: vec![NAMES] });
    referee.fire(at(361), &mut Vec::new());
    assert!(why(&referee).contains("was not met"), "{}", why(&referee));
}

#[test]
fn an_answer_the_worker_gave_up_is_not_acknowledged() {
    let mut referee = referee();
    see(&mut referee, 1, Seen::Answered { names: NAMES, refused: false });
    see(&mut referee, 30, Seen::Stopped { given_up: vec![NAMES] });
    referee.fire(at(300), &mut Vec::new());
    referee.assert_passed(0);
}

/// A channel the world says drops drops as long after it opened as the next
/// life drawn says; the others, and those past the lives drawn, stay open.
#[test]
fn a_channel_that_drops_drops_a_life_after_it_opened() {
    let mut referee = referee();
    let mut stimuli = Vec::new();
    see(&mut referee, 1, Seen::Opened { epoch: 1, drops: false });
    see(&mut referee, 2, Seen::Opened { epoch: 2, drops: true });
    see(&mut referee, 40, Seen::Opened { epoch: 3, drops: true });
    referee.fire(at(31), &mut stimuli);
    assert!(stimuli.is_empty());
    referee.fire(at(32), &mut stimuli);
    assert_eq!(stimuli, [Stimulus::Drop { epoch: 2 }]);
    referee.fire(at(600), &mut stimuli);
    assert_eq!(stimuli.len(), 1, "no life is left for the third channel");
    referee.assert_passed(0);
}

#[test]
fn failures_counted_once_per_attempt_pass() {
    let mut referee = referee();
    let once = Failures::NONE.and(Class::Agent);
    see(&mut referee, 0, recorded(Phase::Claimed, 1, Failures::NONE));
    see(&mut referee, 1, recorded(Phase::Retrying(Class::Agent), 1, once));
    // Written again, as a write in doubt is.
    see(&mut referee, 2, recorded(Phase::Retrying(Class::Agent), 1, once));
    see(&mut referee, 3, recorded(Phase::Claimed, 2, once));
    see(&mut referee, 4, recorded(Phase::Retrying(Class::Lost), 2, once.and(Class::Lost)));
    // Forgiven, and failing again.
    see(&mut referee, 5, recorded(Phase::Waiting, 2, Failures::NONE));
    see(&mut referee, 6, recorded(Phase::Retrying(Class::Agent), 3, once));
    referee.assert_passed(0);
}

#[test]
fn an_attempt_failed_twice_fails() {
    let mut referee = referee();
    let once = Failures::NONE.and(Class::Run);
    see(&mut referee, 0, recorded(Phase::Retrying(Class::Run), 1, once));
    see(&mut referee, 1, recorded(Phase::Retrying(Class::Run), 1, once.and(Class::Run)));
    assert!(why(&referee).contains("an attempt fails once"), "{}", why(&referee));
}

#[test]
fn two_failures_counted_at_once_fail() {
    let mut referee = referee();
    see(&mut referee, 0, recorded(Phase::Retrying(Class::Run), 1, Failures::NONE.and(Class::Run).and(Class::Lost)));
    assert!(why(&referee).contains("one failure at a time"), "{}", why(&referee));
}

#[test]
fn a_call_relayed_and_answered_once_passes() {
    let mut referee = referee();
    see(&mut referee, 0, Seen::Relay { names: NAMES, call: Token::new(9) });
    see(&mut referee, 1, Seen::Relayed { names: NAMES, call: Token::new(9) });
    see(&mut referee, 2, Seen::Relay { names: NAMES, call: Token::new(10) });
    referee.assert_passed(0);
}

#[test]
fn a_call_answered_twice_fails() {
    let mut referee = referee();
    see(&mut referee, 0, Seen::Relay { names: NAMES, call: Token::new(9) });
    see(&mut referee, 1, Seen::Relayed { names: NAMES, call: Token::new(9) });
    see(&mut referee, 2, Seen::Relayed { names: NAMES, call: Token::new(9) });
    assert!(why(&referee).contains("and once"), "{}", why(&referee));
}

#[test]
fn a_call_answered_for_another_attempt_fails() {
    let mut referee = referee();
    see(&mut referee, 0, Seen::Relay { names: NAMES, call: Token::new(9) });
    see(&mut referee, 1, Seen::Relayed { names: (NAMES.0, Token::new(4)), call: Token::new(9) });
    assert!(why(&referee).contains("only a call relayed to it"), "{}", why(&referee));
}

#[test]
fn a_call_relayed_twice_fails() {
    let mut referee = referee();
    see(&mut referee, 0, Seen::Relay { names: NAMES, call: Token::new(9) });
    see(&mut referee, 1, Seen::Relay { names: NAMES, call: Token::new(9) });
    assert!(why(&referee).contains("reaches the engine once"), "{}", why(&referee));
}

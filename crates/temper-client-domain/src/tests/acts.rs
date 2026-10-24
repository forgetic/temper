use super::fixtures::{self, acts as f};
use crate::{acts, pages::Act};
use jig_client::contract::{self as c, ActKind, Admission, ObjectKey};
const FIRST_CARD: u32 = 0;
const OTHER_PROPOSAL: u64 = 4;
const PROPOSAL: ObjectKey = ObjectKey::Proposal(f::PROPOSAL);
const HELD: ObjectKey = ObjectKey::Escalation(f::TASK);
const PERSON: ObjectKey = ObjectKey::PersonTask(f::TASK);
#[test]
fn accepting_sends_nothing_until_the_confirmation_is_confirmed() {
    let mut pages = f::pages(PROPOSAL, f::proposal(ActKind::Accept, Admission::Allowed));
    assert_eq!(f::sent(&mut f::act(&mut pages, f::decide(PROPOSAL, ActKind::Accept))).len(), 0);
    assert_eq!(
        f::sent(&mut f::act(&mut pages, Act::Confirm)).as_ref(),
        [c::Ask::Decide {
            project: fixtures::PROJECT,
            on: c::DecideOn::Proposal { proposal: f::PROPOSAL, decision: c::Decision::Accepted, running: None }
        }]
    );
}
#[test]
fn a_double_press_of_confirm_sends_one_request() {
    let mut pages = f::pages(PROPOSAL, f::proposal(ActKind::Accept, Admission::Allowed));
    f::act(&mut pages, f::decide(PROPOSAL, ActKind::Accept));
    assert_eq!(f::sent(&mut f::act(&mut pages, Act::Confirm)).len(), 1);
    assert_eq!(f::sent(&mut f::act(&mut pages, Act::Confirm)).len(), 0);
    assert_eq!(f::sent(&mut f::act(&mut pages, f::decide(PROPOSAL, ActKind::Accept))).len(), 0);
}
#[test]
fn a_page_holds_at_most_one_open_confirmation() {
    let mut pages = f::pages(PROPOSAL, f::proposal(ActKind::Accept, Admission::Allowed));
    f::act(&mut pages, f::decide(PROPOSAL, ActKind::Accept));
    let other = ObjectKey::Proposal(c::ProposalKey { proposer: c::Proposer::Task(f::TASK), number: OTHER_PROPOSAL });
    let mut second = pages.inbox().expect("inbox").cards.get(FIRST_CARD).expect("first card").clone();
    second.object = other;
    pages.inbox.as_mut().expect("inbox").cards.push(second).expect("second card fits");
    f::act(&mut pages, f::decide(other, ActKind::Accept));
    assert_eq!(
        pages.confirmation(),
        Some(&acts::Confirm { object: other, act: ActKind::Accept, revision: Some(f::REVISION), again: None })
    );
}
#[test]
fn a_decision_names_the_revision_its_card_showed() {
    let mut pages = f::pages(HELD, f::held(ActKind::Release));
    f::act(&mut pages, f::decide(HELD, ActKind::Release));
    assert_eq!(
        f::sent(&mut f::act(&mut pages, Act::Confirm)).as_ref(),
        [c::Ask::Release { project: fixtures::PROJECT, task: f::TASK, revision: f::REVISION, again: None }]
    );
}
#[derive(Clone, Copy)]
enum Direct {
    Answer,
    LeaveHeld,
    Take,
    HandBack,
}
impl Direct {
    const fn act(self) -> ActKind {
        match self {
            Self::Answer => ActKind::Answer,
            Self::LeaveHeld => ActKind::LeaveHeld,
            Self::Take => ActKind::Take,
            Self::HandBack => ActKind::HandBack,
        }
    }
}
const CASES: [(Direct, ObjectKey); 4] =
    [(Direct::Answer, f::QUESTION), (Direct::LeaveHeld, HELD), (Direct::Take, PERSON), (Direct::HandBack, PERSON)];
#[test]
fn each_inbox_act_is_one_keyed_request() {
    for (case, object) in CASES {
        let act = case.act();
        let page = match case {
            Direct::Answer => f::question(),
            Direct::LeaveHeld => f::held(act),
            Direct::Take | Direct::HandBack => f::person(act),
        };
        let mut pages = f::pages(object, page);
        f::act(&mut pages, Act::CardWords { object, words: Box::from(*b"Because") });
        let expected = match case {
            Direct::Answer => c::Ask::Answer {
                project: fixtures::PROJECT,
                on: c::AnswerOn::Question { task: f::TASK, message: f::MESSAGE, words: f::text(b"Because") },
            },
            Direct::LeaveHeld => c::Ask::Decide {
                project: fixtures::PROJECT,
                on: c::DecideOn::Escalation {
                    task: f::TASK,
                    revision: f::REVISION,
                    decision: c::Decision::LeftHeld { reason: f::text(b"Because") },
                },
            },
            Direct::Take => c::Ask::Take { project: fixtures::PROJECT, task: f::TASK },
            Direct::HandBack => c::Ask::HandBack { project: fixtures::PROJECT, task: f::TASK },
        };
        let mut out = f::act(&mut pages, f::decide(object, act));
        assert_eq!(f::sent(&mut out).as_ref(), [expected], "{act:?}");
        assert_eq!(f::sent(&mut f::act(&mut pages, f::decide(object, act))).len(), 0, "{act:?}");
    }
}
#[test]
fn an_act_made_offline_shows_parked_until_the_link_is_live() {
    let mut client = f::live_client();
    let mut out = skein_lib::Queue::with_capacity(jig_client::max_out(&fixtures::limits().client));
    client.step(skein_lib::Time::ZERO, skein_lib::Wall::EPOCH, jig_client::Event::StreamGone, &mut out);
    assert_eq!(client.link().state, jig_client::Link::Offline);
    let mut pages = f::pages(PROPOSAL, f::proposal(ActKind::Accept, Admission::Allowed));
    f::act(&mut pages, f::decide(PROPOSAL, ActKind::Accept));
    let mut asks = f::act(&mut pages, Act::Confirm);
    while let Some(request) = asks.pop() {
        if let crate::pages::Request::Client(event) = request {
            client.step(skein_lib::Time::ZERO, skein_lib::Wall::EPOCH, event, &mut out);
        }
    }
    let card = pages.inbox().expect("inbox").cards.get(FIRST_CARD).expect("card");
    assert_eq!(acts::pending(card, &client), Some(jig_client::Progress::Parked));
}
fn terminal(outcome: jig_client::Outcome) -> crate::pages::Pages {
    let mut pages = f::pages(PROPOSAL, f::proposal(ActKind::Accept, Admission::Allowed));
    f::act(&mut pages, f::decide(PROPOSAL, ActKind::Accept));
    f::act(&mut pages, Act::Confirm);
    let key = pages.inbox().expect("inbox").cards.get(FIRST_CARD).expect("card").key.expect("key");
    assert!(acts::told(&mut pages, &jig_client::Told::Answered { key, outcome }));
    pages
}
#[test]
fn a_request_not_stored_says_not_sent_on_its_card() {
    let pages = terminal(jig_client::Outcome::NotKept);
    assert_eq!(
        pages.inbox().expect("inbox").cards.get(FIRST_CARD).expect("card").outcome,
        Some(jig_client::Outcome::NotKept)
    );
}
#[test]
fn an_answer_unknown_past_the_retention_says_so_on_the_card() {
    let pages = terminal(jig_client::Outcome::Unknown);
    assert_eq!(
        pages.inbox().expect("inbox").cards.get(FIRST_CARD).expect("card").outcome,
        Some(jig_client::Outcome::Unknown)
    );
}
#[test]
fn a_refused_act_shows_jigs_words_and_sends_nothing_more() {
    let outcome = jig_client::Outcome::Answered(jig_client::Answered::Jig(c::Answer::Refused {
        refusal: c::Refusal::NotOffered,
        at: None,
    }));
    let mut pages = terminal(outcome.clone());
    assert_eq!(pages.inbox().expect("inbox").cards.get(FIRST_CARD).expect("card").outcome, Some(outcome));
    assert_eq!(f::sent(&mut f::act(&mut pages, f::decide(PROPOSAL, ActKind::Accept))).len(), 0);
}
#[test]
fn a_deciding_card_survives_its_removal_from_inbox() {
    let mut pages = f::pages(HELD, f::held(ActKind::Release));
    f::act(&mut pages, f::decide(HELD, ActKind::Release));
    f::act(&mut pages, Act::Confirm);
    let entries = pages.inbox().expect("inbox").entries;
    let limits = fixtures::limits();
    let mut out = skein_lib::Queue::with_capacity(crate::pages::inbox::max_out(&limits));
    crate::pages::step(
        &mut pages,
        &fixtures::env(&limits),
        crate::unlinked(&limits, fixtures::SEED).client(),
        crate::pages::Event::Told { told: jig_client::Told::Changed { handle: entries } },
        &mut out,
    );
    assert_eq!(pages.inbox().expect("inbox").cards.len(), 1);
}

#[test]
fn cancelling_drops_the_pages_open_confirmation_without_sending() {
    let mut pages = f::pages(PROPOSAL, f::proposal(ActKind::Accept, Admission::Allowed));
    f::act(&mut pages, f::decide(PROPOSAL, ActKind::Accept));
    assert!(f::sent(&mut f::act(&mut pages, Act::Cancel)).is_empty());
    assert_eq!(pages.confirmation(), None);
}

#[test]
fn card_words_keep_the_send_bound_when_receiving_grows() {
    let mut limits = fixtures::limits();
    limits.client.domain.text = 2048;
    limits.client.domain.turn_text = 2048;
    let mut pages = f::pages(PROPOSAL, f::proposal(ActKind::Reject, Admission::Allowed));
    let mut out = skein_lib::Queue::with_capacity(32);
    let bound = usize::try_from(temper_client_protocol_wire::CUE_BYTES).unwrap();
    let mut words = skein_lib::bytes::zeroed(bound);
    words.fill(b'a');
    let mut over = skein_lib::bytes::zeroed(bound.checked_add(1).unwrap());
    over.fill(b'b');
    acts::step(
        &mut pages,
        &fixtures::env(&limits),
        Act::CardWords { object: PROPOSAL, words: words.clone() },
        &mut out,
    );
    acts::step(&mut pages, &fixtures::env(&limits), Act::CardWords { object: PROPOSAL, words: over }, &mut out);
    assert_eq!(pages.inbox().unwrap().cards.get(FIRST_CARD).unwrap().words.as_ref().unwrap().bytes(), &*words);
}

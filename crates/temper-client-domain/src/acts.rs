//! Inbox acts retain the document revision that the person acted on (client/README.md, 3.6).
use crate::{
    Limits,
    owner::Owner,
    pages::{Act, Pages, Request, inbox::Card},
};
use alloc::boxed::Box;
use jig_client::{
    Client, Key, Told,
    contract::{self, ActKind, Admission, Ask, DecideOn, Decision, ObjectKey, Offered, ReadValue, Text},
};
use skein_lib::{Env, Queue};
/// The page's one open confirmation. Its object and revision come from the binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirm {
    pub object: ObjectKey,
    pub revision: Option<contract::Revision>,
    pub act: ActKind,
    pub again: Option<bool>,
}
#[must_use]
pub fn offers(page: &ReadValue) -> &[Offered] {
    match page {
        ReadValue::Task(v) => v.acts.as_slice(),
        ReadValue::Proposal(v) => v.acts.as_slice(),
        ReadValue::OwnerProposal(v) => v.acts.as_slice(),
        ReadValue::Question(v) => v.acts.as_slice(),
        ReadValue::Escalation(v) => v.acts.as_slice(),
        ReadValue::OwnerEscalation(v) => v.acts.as_slice(),
        ReadValue::PersonTask(v) => v.acts.as_slice(),
        ReadValue::OwnerPersonTask(v) => v.acts.as_slice(),
        ReadValue::Settings(_)
        | ReadValue::Setting(_)
        | ReadValue::Revision(_)
        | ReadValue::Revisions(_)
        | ReadValue::Tasks(_)
        | ReadValue::OwnerProject(_)
        | ReadValue::Historical(_)
        | ReadValue::Me(_)
        | ReadValue::Result(_)
        | ReadValue::Ended(_)
        | ReadValue::OwnerEnded(_)
        | ReadValue::History(_)
        | ReadValue::Turn(_)
        | ReadValue::Activity(_)
        | ReadValue::Inbox(_)
        | ReadValue::OwnerInbox(_)
        | ReadValue::Spend(_)
        | ReadValue::OwnerSpend(_)
        | ReadValue::Project(_)
        | ReadValue::Notes(_)
        | ReadValue::Deployment(_)
        | ReadValue::OwnerDeployment(_) => &[],
    }
}
#[must_use]
#[expect(clippy::manual_map, reason = "step code keeps optional owner facts explicit")]
pub fn revision(page: &ReadValue) -> Option<contract::Revision> {
    match page {
        ReadValue::Task(v) => match &v.hold {
            Some(hold) => Some(hold.revision()),
            None => None,
        },
        ReadValue::Escalation(v) => Some(v.hold.revision),
        ReadValue::OwnerEscalation(v) => match &v.task.hold {
            Some(hold) => Some(hold.revision()),
            None => None,
        },
        ReadValue::PersonTask(v) => Some(v.revision),
        ReadValue::OwnerPersonTask(v) => Some(v.revision),
        ReadValue::Settings(_)
        | ReadValue::Setting(_)
        | ReadValue::Revision(_)
        | ReadValue::Revisions(_)
        | ReadValue::Tasks(_)
        | ReadValue::OwnerProject(_)
        | ReadValue::Historical(_)
        | ReadValue::Me(_)
        | ReadValue::Result(_)
        | ReadValue::Ended(_)
        | ReadValue::OwnerEnded(_)
        | ReadValue::History(_)
        | ReadValue::Turn(_)
        | ReadValue::Activity(_)
        | ReadValue::Inbox(_)
        | ReadValue::OwnerInbox(_)
        | ReadValue::Spend(_)
        | ReadValue::OwnerSpend(_)
        | ReadValue::Project(_)
        | ReadValue::Notes(_)
        | ReadValue::Deployment(_)
        | ReadValue::OwnerDeployment(_)
        | ReadValue::Proposal(_)
        | ReadValue::OwnerProposal(_)
        | ReadValue::Question(_) => None,
    }
}
fn find_card(pages: &mut Pages, object: ObjectKey) -> Option<&mut Card> {
    let cards = pages.cards_mut()?;
    for index in 0..cards.len() {
        if cards.get(index)?.object == object {
            return cards.get_mut(index);
        }
    }
    None
}
pub(super) fn step(pages: &mut Pages, _env: &Env<Limits>, act: Act, out: &mut Queue<Request>) {
    match act {
        Act::CardWords { object, words } => {
            if let Ok(words) = Text::new(words, temper_client_protocol_wire::CUE_BYTES)
                && let Some(card) = find_card(pages, object)
            {
                card.words = Some(words);
            }
        }
        Act::Decide { object, revision, act } => {
            let Some(card) = find_card(pages, object) else { return };
            if card.key.is_some() {
                return;
            }
            let Some(page) = &card.page else { return };
            if !allowed(page, act) {
                return;
            }
            let confirm = Confirm { object, revision, act, again: None };
            if jig_client::facts::dangerous(act) || (act == ActKind::Accept || act == ActKind::Release) {
                pages.confirm = Some(confirm);
            } else {
                send(pages, confirm, out);
            }
        }
        Act::Again { again } => {
            if let Some(open) = &mut pages.confirm {
                open.again = Some(again);
            }
        }
        Act::Confirm => {
            if let Some(open) = pages.confirm.take() {
                send(pages, open, out);
            }
        }
        Act::Cancel => pages.confirm = None,
        Act::MessageWords { .. }
        | Act::Send
        | Act::Stop
        | Act::Close
        | Act::DelegateFate { .. }
        | Act::Older
        | Act::ChatWords { .. }
        | Act::StartChat
        | Act::RetryDemo
        | Act::Playground
        | Act::Suggest { .. }
        | Act::ChatFilter { .. }
        | Act::PlanFilter { .. }
        | Act::ClosedChats => unreachable!("chat acts routed to chats"),
        Act::Choice(_)
        | Act::Accounts(_)
        | Act::ConnectModel(_)
        | Act::OpenProject { .. }
        | Act::Project { .. }
        | Act::SignOut
        | Act::Filter { .. }
        | Act::OpenResult { .. }
        | Act::Amend
        | Act::AmendWords { .. }
        | Act::AmendTracked
        | Act::AmendPriority { .. }
        | Act::DiscardAmendment
        | Act::SendAmendment
        | Act::AmendAgain
        | Act::Go { .. } => {
            unreachable!("only deciding acts routed here")
        }
    }
}
fn send(pages: &mut Pages, open: Confirm, out: &mut Queue<Request>) {
    let Some(card) = find_card(pages, open.object) else { return };
    if card.key.is_some() {
        return;
    }
    let Some(project) = card.project else { return };
    let Some(page) = &card.page else { return };
    if !allowed(page, open.act) {
        return;
    }
    let words = card.words.clone();
    let ask = match (open.object, open.act) {
        (ObjectKey::Proposal(proposal), ActKind::Accept | ActKind::Reject) => Ask::Decide {
            project,
            on: DecideOn::Proposal {
                proposal,
                running: None,
                decision: if open.act == ActKind::Accept {
                    Decision::Accepted
                } else {
                    let Some(reason) = words else {
                        pages.confirm = Some(open);
                        return;
                    };
                    Decision::Rejected { reason }
                },
            },
        },
        (ObjectKey::Escalation(task) | ObjectKey::Task(task), ActKind::Release) => {
            if jig_client::facts::effect(offers(page), open.act)
                == Some(&contract::Effect::Release(contract::ReleaseEffect::Uncertain))
                && open.again.is_none()
            {
                pages.confirm = Some(open);
                return;
            }
            let Some(revision) = open.revision else { return };
            Ask::Release { project, task, revision, again: open.again }
        }
        (ObjectKey::Escalation(task), ActKind::LeaveHeld) => {
            let (Some(revision), Some(reason)) = (open.revision, words) else { return };
            Ask::Decide {
                project,
                on: DecideOn::Escalation { task, revision, decision: Decision::LeftHeld { reason } },
            }
        }
        (ObjectKey::Question { task, message }, ActKind::Answer) => {
            let Some(words) = words else { return };
            Ask::Answer { project, on: contract::AnswerOn::Question { task, message, words } }
        }
        (ObjectKey::PersonTask(task), ActKind::Take) => Ask::Take { project, task },
        (ObjectKey::PersonTask(task), ActKind::HandBack) => Ask::HandBack { project, task },
        _ => return,
    };
    pages.serial = pages.serial.checked_add(1).expect("pages keys fit");
    let key = Key { asker: Owner::Pages.asker(), number: pages.serial };
    find_card(pages, open.object).expect("card retained").key = Some(key);
    out.push(Request::Client(jig_client::Event::Send {
        key,
        name: None,
        request: jig_client::Ask::Jig(Box::new(ask)),
    }));
}
pub(super) fn told(pages: &mut Pages, told: &Told) -> bool {
    if let Told::Answered { key, outcome } = told
        && let Some(cards) = pages.cards_mut()
    {
        for index in 0..cards.len() {
            let card = cards.get_mut(index).expect("bounded index");
            if card.key == Some(*key) {
                card.outcome = Some(outcome.clone());
                return true;
            }
        }
    }
    match told {
        Told::Pending { .. } => true,
        Told::Link { .. }
        | Told::Answered { .. }
        | Told::Kept { .. }
        | Told::Dropped { .. }
        | Told::Changed { .. }
        | Told::Text { .. }
        | Told::Ended { .. }
        | Told::Page { .. } => false,
    }
}
#[must_use]
pub fn pending(card: &Card, client: &Client) -> Option<jig_client::Progress> {
    Some(client.pending(card.key?)?.progress)
}

fn allowed(page: &ReadValue, act: ActKind) -> bool {
    match jig_client::facts::offer(offers(page), act) {
        Some(Admission::Allowed | Admission::Bounded { .. }) => true,
        Some(Admission::Refused(_) | Admission::Proposed { .. }) | None => false,
    }
}

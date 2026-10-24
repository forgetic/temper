//! Temper's cards read delivered objects and jig's public decision facts.
pub(crate) mod delegation;
mod proposal;
pub mod words;
use crate::words::Words;
use alloc::boxed::Box;
use jig_client::contract as c;
use skein_ui::{Builder, Role, Run, phrases::Hole};
pub use words::CardWord as P;
pub fn text(b: &mut Builder, role: Role, value: &[u8]) {
    b.open(role);
    b.name(value);
    b.text(Run::Plain, value);
    b.close();
}
pub fn phrase(b: &mut Builder, words: &Words, word: P, holes: &[Hole]) {
    text(b, Role::Paragraph, &words::say(words, word, holes));
}
pub fn status(b: &mut Builder, words: &Words, word: P, holes: &[Hole]) {
    text(b, Role::Status, &words::say(words, word, holes));
}
pub fn refusal(b: &mut Builder, words: &Words, value: &c::Refusal) {
    let hole = match value {
        c::Refusal::Account { account, .. } => Some(Hole::Number(u64::from(*account))),
        c::Refusal::Taken { by } => Some(Hole::Number(*by)),
        c::Refusal::Llm { model, .. } => Some(Hole::Name(words.model_name(model.bytes()))),
        c::Refusal::Stale(_)
        | c::Refusal::Full(_)
        | c::Refusal::AccountMove { .. }
        | c::Refusal::SignInRedirect { .. }
        | c::Refusal::NoEscalationHolder
        | c::Refusal::NameTaken
        | c::Refusal::Malformed { .. }
        | c::Refusal::Role { .. }
        | c::Refusal::NotWaiting
        | c::Refusal::NoFurther
        | c::Refusal::Authority { .. }
        | c::Refusal::Limit { .. }
        | c::Refusal::Moved { .. }
        | c::Refusal::Ended { .. }
        | c::Refusal::Unknown
        | c::Refusal::NotOffered
        | c::Refusal::KeyConflict
        | c::Refusal::Circle { .. }
        | c::Refusal::Dependency { .. }
        | c::Refusal::Kept { .. }
        | c::Refusal::Provider => None,
    };
    let name = words.say_word(jig_client::words::Word::refusal(value), hole.as_slice());
    b.open(Role::Status);
    b.style(crate::look::REFUSAL);
    b.name(&name);
    b.text(Run::Plain, &name);
    b.close();
    if let c::Refusal::Role { held: None } = value {
        phrase(b, words, P::NoHeldRole, &[]);
    }
}
pub fn decision(b: &mut Builder, words: &Words, page: Option<&c::ReadValue>, act: c::ActKind) {
    let facts = jig_client::facts::decision(page, act);
    for fact in &jig_client::words::decision(&facts, &words.book, words.language) {
        text(b, Role::Paragraph, fact);
    }
}
pub fn progress(b: &mut Builder, words: &Words, value: jig_client::Progress) {
    status(
        b,
        words,
        match value {
            jig_client::Progress::InFlight => P::InFlight,
            jig_client::Progress::Retrying { .. } => P::Retrying,
            jig_client::Progress::Parked | jig_client::Progress::NotServed => P::Parked,
            jig_client::Progress::Unknown => P::Unknown,
        },
        &[],
    );
}
pub fn outcome(b: &mut Builder, words: &Words, value: &c::Outcome) {
    // The outcome names the engine's answer; it is never inferred from local state.
    let phrase = match value {
        c::Outcome::Owner(value) => return owner_outcome(b, words, value),
        c::Outcome::Decided { how, .. } | c::Outcome::DecidedFirst { how, .. } => return decided(b, words, how),
        c::Outcome::SettingsChanged(_) | c::Outcome::Made { .. } | c::Outcome::Changed => P::Applied,
        c::Outcome::Proposed { .. } => P::ProposeInstead,
    };
    status(b, words, phrase, &[]);
}
fn decided(b: &mut Builder, words: &Words, how: &c::Decision) {
    status(
        b,
        words,
        match how {
            c::Decision::Accepted => P::Accepted,
            c::Decision::Rejected { .. } => P::Rejected,
            c::Decision::PassedUp => P::ProposeInstead,
            c::Decision::Released { .. } => P::Released,
            c::Decision::LeftHeld { .. } => P::LeftHeld,
        },
        &[],
    );
}
fn owner_outcome(b: &mut Builder, words: &Words, value: &c::OwnerOutcome) {
    use c::OwnerOutcome as O;
    let word = match value {
        O::Cancelled { .. } => P::Cancelled,
        O::Released { .. } => P::Released,
        O::PersonTaken { .. } => P::Taken,
        O::PersonHandedBack { .. } => P::HandedBack,
        O::PersonAnswered { .. } | O::QuestionAnswered { .. } => P::Answered,
        O::ProposalDecided { choice, .. } => match choice {
            c::OwnerProposalChoice::Accepted => P::Accepted,
            c::OwnerProposalChoice::Rejected => P::Rejected,
            c::OwnerProposalChoice::Passed => P::ProposeInstead,
            c::OwnerProposalChoice::Withdrawn | c::OwnerProposalChoice::Stale => P::Cancelled,
        },
        O::EscalationDecided { choice, .. } => match choice {
            c::OwnerEscalationChoice::Released => P::Released,
            c::OwnerEscalationChoice::Rejected => P::LeftHeld,
            c::OwnerEscalationChoice::Passed => P::ProposeInstead,
        },
        O::AccountMovedIn { .. }
        | O::Created { .. }
        | O::Compound { .. }
        | O::MarkedSeen { .. }
        | O::NoteEdited { .. }
        | O::ProjectMade { .. }
        | O::Renamed { .. }
        | O::ServiceMade { .. }
        | O::Adopted { .. }
        | O::Stopped { .. }
        | O::Moved { .. }
        | O::Said { .. }
        | O::Amended { .. }
        | O::Proposed { .. }
        | O::Withdrawn { .. }
        | O::RolesSet { .. }
        | O::PolicyChanged { .. }
        | O::PoolSet { .. }
        | O::AccountKeyKept { .. }
        | O::AccountFinishing { .. }
        | O::AccountChanged
        | O::AccountRemoved { .. } => P::Applied,
        O::AccountProgramDeclared { name: declared, .. } => {
            status(b, words, P::Applied, &[]);
            b.text(Run::Plain, declared);
            return;
        }
        O::AccountSignInStarted { .. } => P::AccountSignInStarted,
        O::AccountSignedOut { .. } => P::AccountSignedOut,
    };
    status(b, words, word, &[]);
}

pub fn document(b: &mut Builder, value: &c::DocumentText) {
    skein_ui::markdown::read(value.bytes(), 3, b);
}
pub fn specification(b: &mut Builder, value: &c::SpecificationDocument, words: &Words) {
    document(b, &value.specification.words);
    specification_facts(b, value, words);
}
pub(super) fn specification_facts(b: &mut Builder, value: &c::SpecificationDocument, words: &Words) {
    for parameter in &value.specification.parameters {
        match parameter {
            c::ParameterDocument::Number { name, value } => {
                phrase(b, words, P::ParameterNumber, &[Hole::Number(u64::from(*name)), Hole::Number(*value)]);
            }
            c::ParameterDocument::Bytes { name, value } => {
                phrase(b, words, P::ParameterBytes, &[Hole::Number(u64::from(*name)), self::name(value.bytes())]);
            }
            c::ParameterDocument::Resource { name, connector, resource } => phrase(
                b,
                words,
                P::ParameterResource,
                &[Hole::Number(u64::from(*name)), Hole::Number(*resource), Hole::Number(u64::from(*connector))],
            ),
        }
    }
    for task in &value.specification.inputs {
        phrase(b, words, P::Input, &[Hole::Number(*task)]);
    }
    if value.has_loss() {
        status(b, words, P::Cut, &[]);
    }
}
pub fn recipient(b: &mut Builder, words: &Words, to: &c::Recipient) {
    match to {
        c::Recipient::Party(number) => phrase(b, words, P::ToPerson, &[Hole::Number(*number)]),
        c::Recipient::Role { project, role } => {
            phrase(b, words, P::ToRole, &[Hole::Number(u64::from(*role)), Hole::Number(u64::from(*project))]);
        }
        c::Recipient::Roles { project, roles } => {
            for role in roles {
                phrase(b, words, P::ToRole, &[Hole::Number(u64::from(*role)), Hole::Number(u64::from(*project))]);
            }
        }
        c::Recipient::Task(number) => phrase(b, words, P::ToTask, &[Hole::Number(*number)]),
    }
}
pub fn value(b: &mut Builder, result: &c::ResultValue, _words: &Words) {
    match result {
        c::ResultValue::Report(text)
        | c::ResultValue::Failure(text)
        | c::ResultValue::Words(text)
        | c::ResultValue::OwnerVerdict { words: text, .. }
        | c::ResultValue::Verdict { words: text, .. }
        | c::ResultValue::Connector { summary: text, .. }
        | c::ResultValue::Change { words: text, .. }
        | c::ResultValue::Choice { reason: text, .. } => document(b, text),
        c::ResultValue::Approved { words: text, .. } => {
            if let Some(text) = text {
                document(b, text);
            }
        }
    }
}
pub fn result(b: &mut Builder, result: &c::Result, words: &Words) {
    text(b, Role::Heading { level: 3 }, result.task.title.bytes());
    ending(b, &result.ending, words);
    if let Some(result) = &result.value {
        value(b, result, words);
    }
}
pub fn ending(b: &mut Builder, ending: &c::Ending, words: &Words) {
    let word = match ending {
        c::Ending::Task(value) => match value.as_ref() {
            c::OwnerChipEnding::Done(result) => {
                value_nodes(b, result, words);
                P::Done
            }
            c::OwnerChipEnding::Failed(reason) => {
                document(b, reason);
                P::Failed
            }
            c::OwnerChipEnding::Cancelled { reason, result, .. } => {
                document(b, reason);
                if let Some(result) = result {
                    value_nodes(b, result, words);
                }
                P::Cancelled
            }
        },
        c::Ending::Done => P::Done,
        c::Ending::Failed { .. } => P::Failed,
        c::Ending::Cancelled { .. } | c::Ending::Withdrawn { .. } | c::Ending::Deleted => P::Cancelled,
        c::Ending::Decided { how, .. } => {
            if let Some(how) = how {
                decided(b, words, how);
            }
            return;
        }
    };
    status(b, words, word, &[]);
}
pub fn value_nodes(b: &mut Builder, result: &c::ResultValue, words: &Words) {
    value(b, result, words);
}
#[must_use]
pub fn name(value: &[u8]) -> Hole {
    Hole::Name(skein_lib::bytes::copy_of(value))
}
impl Words {
    #[must_use]
    pub fn say_card(&self, word: P, holes: &[Hole]) -> Box<[u8]> {
        words::say(self, word, holes)
    }
    pub fn card_text(&self, b: &mut Builder, word: P, holes: &[Hole]) {
        phrase(b, self, word, holes);
    }
    pub fn status(&self, b: &mut Builder, word: P, holes: &[Hole]) {
        status(b, self, word, holes);
    }
    pub fn refusal(&self, b: &mut Builder, value: &c::Refusal, _text: u32) {
        refusal(b, self, value);
    }
    pub fn outcome(&self, b: &mut Builder, value: &c::Outcome) {
        outcome(b, self, value);
    }
    pub fn ending(&self, b: &mut Builder, value: &c::Ending) {
        ending(b, value, self);
    }
}
/// Where a proposal card is drawn. A chat's card heads its title at level 2 and
/// says its kind above it; an inbox card heads it at level 3 under a tag that
/// says the kind already.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Chat,
    Inbox,
}
impl Place {
    pub(crate) const fn level(self) -> u8 {
        match self {
            Self::Chat => 2,
            Self::Inbox => 3,
        }
    }
    pub(crate) const fn kicker(self) -> bool {
        match self {
            Self::Chat => true,
            Self::Inbox => false,
        }
    }
}
pub fn proposal(b: &mut Builder, page: &c::ReadValue, words: &Words, rows: u32) {
    proposal_at(b, page, words, rows, Place::Chat);
}

/// Project the same proposal where its caller draws it.
pub fn proposal_at(b: &mut Builder, page: &c::ReadValue, words: &Words, rows: u32, place: Place) {
    proposal::reason(b, page);
    proposal::facts(b, page, words, rows, place);
}
pub(crate) fn proposal_reason(b: &mut Builder, page: &c::ReadValue) {
    proposal::reason(b, page);
}
pub(crate) fn proposal_facts(b: &mut Builder, page: &c::ReadValue, words: &Words, rows: u32, place: Place) {
    proposal::facts(b, page, words, rows, place);
}

pub fn hold(b: &mut Builder, value: &c::HoldCategory, words: &Words) {
    text(b, Role::Paragraph, &hold_phrase(value, words));
}

pub(crate) fn hold_phrase(value: &c::HoldCategory, words: &Words) -> Box<[u8]> {
    let number = match value {
        c::HoldCategory::Exceeded(value) => match value.bound {
            c::Bound::History => None,
            c::Bound::Reasoning
            | c::Bound::Metadata
            | c::Bound::Items
            | c::Bound::Answer
            | c::Bound::Tools
            | c::Bound::HistoryItems
            | c::Bound::Window
            | c::Bound::Output => Some(Hole::Number(value.value)),
        },
        c::HoldCategory::Dependency { task } => Some(Hole::Number(*task)),
        c::HoldCategory::Uncertain { entry } => Some(Hole::Number(*entry)),
        c::HoldCategory::PoolLost { pool } => Some(Hole::Number(*pool)),
        c::HoldCategory::Failures { .. }
        | c::HoldCategory::Stalled
        | c::HoldCategory::Stopped
        | c::HoldCategory::StoppedBy { .. }
        | c::HoldCategory::Drift
        | c::HoldCategory::Effects
        | c::HoldCategory::EffectFailed
        | c::HoldCategory::Budget
        | c::HoldCategory::Deadline
        | c::HoldCategory::HoldsWaited
        | c::HoldCategory::ResourceUnavailable
        | c::HoldCategory::ResourceAccess
        | c::HoldCategory::Circle
        | c::HoldCategory::Procedure => None,
    };
    words.say_word(jig_client::words::Word::hold(value), number.as_slice())
}

pub(crate) fn proposal_has_rows(page: &c::ReadValue) -> bool {
    proposal::has_rows(page)
}

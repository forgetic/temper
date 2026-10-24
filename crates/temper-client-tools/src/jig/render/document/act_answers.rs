//! Complete durable answer records, sharing the document codecs.
use skein_json::writer::Encoder;
pub(super) fn encode_answer(value: &jig_client::contract::Answer, out: &mut Encoder, budget: &mut super::Cut) {
    match value {
        jig_client::contract::Answer::Done(field_0) => {
            out.object_start();
            out.key(b"done");
            encode_outcome(field_0, out, budget);
            out.object_end();
        }
        jig_client::contract::Answer::Refused { refusal, at } => {
            out.object_start();
            out.key(b"refused");
            out.object_start();
            out.key(b"refusal");
            super::encode_refusal(refusal, out, budget);
            out.key(b"at");
            match at {
                Some(item_1) => {
                    out.unsigned(u64::from(*item_1));
                }
                None => out.null(),
            }
            out.object_end();
            out.object_end();
        }
    }
}

pub(super) fn encode_created_given(
    value: &jig_client::contract::CreatedGiven,
    out: &mut Encoder,
    _budget: &mut super::Cut,
) {
    out.object_start();
    out.key(b"task");
    out.unsigned(value.task);
    out.key(b"amount");
    out.unsigned(value.amount);
    out.object_end();
}

pub(super) fn encode_made(value: &jig_client::contract::Made, out: &mut Encoder, budget: &mut super::Cut) {
    match value {
        jig_client::contract::Made::MovingOut { account, number, .. } => {
            // A moved record is never part of a printed answer.
            out.object_start();
            out.key(b"moving_out");
            out.object_start();
            out.key(b"account");
            out.unsigned(u64::from(*account));
            out.key(b"number");
            out.unsigned(u64::from(*number));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Made::Tasks { tasks } => {
            out.object_start();
            out.key(b"tasks");
            out.object_start();
            out.key(b"tasks");
            out.array_start();
            for item_1 in tasks {
                out.unsigned(*(item_1));
            }
            out.array_end();
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Made::Service { party } => {
            out.object_start();
            out.key(b"service");
            out.object_start();
            out.key(b"party");
            out.unsigned(*(party));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Made::PersonResult { task } => {
            out.object_start();
            out.key(b"person_result");
            out.object_start();
            out.key(b"task");
            out.unsigned(*(task));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Made::Note { note, scope } => {
            out.object_start();
            out.key(b"note");
            out.object_start();
            out.key(b"note");
            out.unsigned(*(note));
            out.key(b"scope");
            encode_note_scope(scope, out, budget);
            out.object_end();
            out.object_end();
        }
    }
}

pub(super) fn encode_note_scope(value: &jig_client::contract::NoteScope, out: &mut Encoder, budget: &mut super::Cut) {
    match value {
        jig_client::contract::NoteScope::Deployment => out.string(b"deployment"),
        jig_client::contract::NoteScope::Project(field_0) => {
            out.object_start();
            out.key(b"project");
            out.unsigned(u64::from(*field_0));
            out.object_end();
        }
        jig_client::contract::NoteScope::Resource { project, connector, pattern } => {
            out.object_start();
            out.key(b"resource");
            out.object_start();
            out.key(b"project");
            out.unsigned(u64::from(*project));
            out.key(b"connector");
            out.unsigned(u64::from(*connector));
            out.key(b"pattern");
            super::encode_owner_pattern(pattern, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::NoteScope::Task { project, task } => {
            out.object_start();
            out.key(b"task");
            out.object_start();
            out.key(b"project");
            out.unsigned(u64::from(*project));
            out.key(b"task");
            out.unsigned(*(task));
            out.object_end();
            out.object_end();
        }
    }
}

pub(super) fn encode_outcome(value: &jig_client::contract::Outcome, out: &mut Encoder, budget: &mut super::Cut) {
    match value {
        jig_client::contract::Outcome::SettingsChanged(_) => unreachable!("the tools do not request settings"),
        jig_client::contract::Outcome::Owner(field_0) => {
            out.object_start();
            out.key(b"owner");
            encode_owner_outcome(field_0, out, budget);
            out.object_end();
        }
        jig_client::contract::Outcome::Made { what } => {
            out.object_start();
            out.key(b"made");
            out.object_start();
            out.key(b"what");
            encode_made(what, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Outcome::Proposed { proposal } => {
            out.object_start();
            out.key(b"proposed");
            out.object_start();
            out.key(b"proposal");
            super::encode_proposal_key(proposal, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Outcome::Decided { by, how, at } => {
            out.object_start();
            out.key(b"decided");
            out.object_start();
            out.key(b"by");
            super::encode_actor(by, out, budget);
            out.key(b"how");
            super::encode_decision(how, out, budget);
            out.key(b"at");
            out.unsigned((at).as_nanos());
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Outcome::DecidedFirst { by, how, at } => {
            out.object_start();
            out.key(b"decided_first");
            out.object_start();
            out.key(b"by");
            super::encode_party(by, out, budget);
            out.key(b"how");
            super::encode_decision(how, out, budget);
            out.key(b"at");
            out.unsigned((at).as_nanos());
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Outcome::Changed => out.string(b"changed"),
    }
}

pub(super) fn encode_owner_escalation_choice(
    value: jig_client::contract::OwnerEscalationChoice,
    out: &mut Encoder,
    _budget: &mut super::Cut,
) {
    match value {
        jig_client::contract::OwnerEscalationChoice::Released => out.string(b"released"),
        jig_client::contract::OwnerEscalationChoice::Rejected => out.string(b"rejected"),
        jig_client::contract::OwnerEscalationChoice::Passed => out.string(b"passed"),
    }
}

pub(super) fn encode_owner_outcome(
    value: &jig_client::contract::OwnerOutcome,
    out: &mut Encoder,
    budget: &mut super::Cut,
) {
    match value {
        jig_client::contract::OwnerOutcome::AccountMovedIn { account, renewed } => {
            encode_moved_in(*account, *renewed, out);
        }
        jig_client::contract::OwnerOutcome::AccountKeyKept { source } => {
            out.object_start();
            out.key(b"account_key_kept");
            out.object_start();
            out.key(b"source");
            out.unsigned(u64::from(*source));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerOutcome::AccountFinishing { source } => encode_account_finishing(*source, out),
        jig_client::contract::OwnerOutcome::AccountProgramDeclared { source, name } => {
            out.object_start();
            out.key(b"account_program_declared");
            out.object_start();
            out.key(b"source");
            out.unsigned(u64::from(*source));
            out.key(b"name");
            budget.words(name, out);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerOutcome::ProjectMade { project } => {
            out.object_start();
            out.key(b"project_made");
            out.object_start();
            out.key(b"project");
            out.unsigned(u64::from(*project));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerOutcome::Renamed { project } => {
            out.object_start();
            out.key(b"renamed");
            out.object_start();
            out.key(b"project");
            out.unsigned(u64::from(*project));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerOutcome::AccountChanged => out.string(b"account_changed"),
        jig_client::contract::OwnerOutcome::AccountRemoved { confirmed, failed_hosts } => {
            out.object_start();
            out.key(b"account_removed");
            out.object_start();
            out.key(b"confirmed");
            out.boolean(*confirmed);
            encode_failed_hosts(failed_hosts, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerOutcome::AccountSignInStarted { source, address } => {
            out.object_start();
            out.key(b"account_sign_in_started");
            out.object_start();
            out.key(b"source");
            out.unsigned(u64::from(*source));
            out.key(b"address");
            out.string(address);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerOutcome::AccountSignedOut { confirmed, failed_hosts } => {
            out.object_start();
            out.key(b"account_signed_out");
            out.object_start();
            out.key(b"confirmed");
            out.boolean(*confirmed);
            encode_failed_hosts(failed_hosts, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerOutcome::Created { .. } => owner_created(value, out, budget),
        jig_client::contract::OwnerOutcome::Compound { .. } => owner_compound(value, out, budget),
        jig_client::contract::OwnerOutcome::MarkedSeen { .. } => owner_marked_seen(value, out, budget),
        jig_client::contract::OwnerOutcome::NoteEdited { .. } => owner_note_edited(value, out, budget),
        jig_client::contract::OwnerOutcome::ServiceMade { .. } => owner_service_made(value, out, budget),
        jig_client::contract::OwnerOutcome::Adopted { .. } => owner_adopted(value, out, budget),
        jig_client::contract::OwnerOutcome::Stopped { .. } => owner_stopped(value, out, budget),
        jig_client::contract::OwnerOutcome::Cancelled { .. } => owner_cancelled(value, out, budget),
        jig_client::contract::OwnerOutcome::Released { .. } => owner_released(value, out, budget),
        jig_client::contract::OwnerOutcome::PersonTaken { .. } => owner_person_taken(value, out, budget),
        jig_client::contract::OwnerOutcome::PersonHandedBack { .. } => owner_person_handed_back(value, out, budget),
        jig_client::contract::OwnerOutcome::PersonAnswered { .. } => owner_person_answered(value, out, budget),
        jig_client::contract::OwnerOutcome::Moved { .. } => owner_moved(value, out, budget),
        jig_client::contract::OwnerOutcome::ProposalDecided { .. } => owner_proposal_decided(value, out, budget),
        jig_client::contract::OwnerOutcome::Said { .. } => owner_said(value, out, budget),
        jig_client::contract::OwnerOutcome::QuestionAnswered { .. } => owner_question_answered(value, out, budget),
        jig_client::contract::OwnerOutcome::Amended { .. } => owner_amended(value, out, budget),
        jig_client::contract::OwnerOutcome::Proposed { .. } => owner_proposed(value, out, budget),
        jig_client::contract::OwnerOutcome::Withdrawn { .. } => owner_withdrawn(value, out, budget),
        jig_client::contract::OwnerOutcome::RolesSet { .. } => owner_roles_set(value, out, budget),
        jig_client::contract::OwnerOutcome::PolicyChanged { .. } => owner_policy_changed(value, out, budget),
        jig_client::contract::OwnerOutcome::PoolSet { .. } => owner_pool_set(value, out, budget),
        jig_client::contract::OwnerOutcome::EscalationDecided { .. } => owner_escalation_decided(value, out, budget),
    }
}
fn encode_account_finishing(source: u32, out: &mut Encoder) {
    out.object_start();
    out.key(b"account_finishing");
    out.object_start();
    out.key(b"source");
    out.unsigned(u64::from(source));
    out.object_end();
    out.object_end();
}
fn owner_created(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Created { first, last, given } = value else {
        unreachable!("dispatched outcome")
    };

    out.object_start();
    out.key(b"created");
    out.object_start();
    out.key(b"first");
    out.unsigned(*(first));
    out.key(b"last");
    out.unsigned(*(last));
    out.key(b"given");
    match given {
        Some(item_1) => {
            encode_created_given(item_1, out, budget);
        }
        None => out.null(),
    }
    out.object_end();
    out.object_end();
}
fn owner_compound(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Compound { project, operations, first_message } = value else {
        unreachable!("dispatched outcome")
    };

    out.object_start();
    out.key(b"compound");
    out.object_start();
    out.key(b"project");
    out.unsigned(u64::from(*project));
    out.key(b"operations");
    out.unsigned(u64::from(*operations));
    out.key(b"first_message");
    out.unsigned(*(first_message));
    out.object_end();
    out.object_end();
}
fn owner_marked_seen(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::MarkedSeen { seen } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"marked_seen");
    out.object_start();
    out.key(b"seen");
    encode_seen_through(seen, out, budget);
    out.object_end();
    out.object_end();
}
fn owner_note_edited(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::NoteEdited { name } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"note_edited");
    out.object_start();
    out.key(b"name");
    out.unsigned(*(name));
    out.object_end();
    out.object_end();
}
fn owner_service_made(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::ServiceMade { person } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"service_made");
    out.object_start();
    out.key(b"person");
    out.unsigned(*(person));
    out.object_end();
    out.object_end();
}
fn owner_adopted(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Adopted { project } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"adopted");
    out.object_start();
    out.key(b"project");
    out.unsigned(u64::from(*project));
    out.object_end();
    out.object_end();
}
fn owner_stopped(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Stopped { task } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"stopped");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.object_end();
    out.object_end();
}
fn owner_cancelled(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Cancelled { task } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"cancelled");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.object_end();
    out.object_end();
}
fn owner_released(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Released { task } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"released");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.object_end();
    out.object_end();
}
fn owner_person_taken(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::PersonTaken { task } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"person_taken");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.object_end();
    out.object_end();
}
fn owner_person_handed_back(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::PersonHandedBack { task } = value else {
        unreachable!("dispatched outcome")
    };

    out.object_start();
    out.key(b"person_handed_back");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.object_end();
    out.object_end();
}
fn owner_person_answered(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::PersonAnswered { task } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"person_answered");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.object_end();
    out.object_end();
}
fn owner_moved(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Moved { task } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"moved");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.object_end();
    out.object_end();
}
fn owner_proposal_decided(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::ProposalDecided { proposal, by, at, choice } = value else {
        unreachable!("dispatched outcome")
    };

    out.object_start();
    out.key(b"proposal_decided");
    out.object_start();
    out.key(b"proposal");
    super::encode_proposal_key(proposal, out, budget);
    out.key(b"by");
    out.unsigned(*(by));
    out.key(b"at");
    out.unsigned((at).as_nanos());
    out.key(b"choice");
    encode_owner_proposal_choice(*choice, out, budget);
    out.object_end();
    out.object_end();
}
fn owner_said(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Said { task, message } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"said");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.key(b"message");
    out.unsigned(*(message));
    out.object_end();
    out.object_end();
}
fn owner_question_answered(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::QuestionAnswered { task, question, message } = value else {
        unreachable!("dispatched outcome")
    };

    out.object_start();
    out.key(b"question_answered");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.key(b"question");
    out.unsigned(*(question));
    out.key(b"message");
    out.unsigned(*(message));
    out.object_end();
    out.object_end();
}
fn owner_amended(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Amended { task } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"amended");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.object_end();
    out.object_end();
}
fn owner_proposed(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Proposed { proposal } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"proposed");
    out.object_start();
    out.key(b"proposal");
    out.unsigned(*(proposal));
    out.object_end();
    out.object_end();
}
fn owner_withdrawn(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::Withdrawn { proposal } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"withdrawn");
    out.object_start();
    out.key(b"proposal");
    out.unsigned(*(proposal));
    out.object_end();
    out.object_end();
}
fn owner_roles_set(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::RolesSet { project } = value else { unreachable!("dispatched outcome") };

    out.object_start();
    out.key(b"roles_set");
    out.object_start();
    out.key(b"project");
    out.unsigned(u64::from(*project));
    out.object_end();
    out.object_end();
}
fn owner_policy_changed(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::PolicyChanged { project } = value else {
        unreachable!("dispatched outcome")
    };

    out.object_start();
    out.key(b"policy_changed");
    out.object_start();
    out.key(b"project");
    out.unsigned(u64::from(*project));
    out.object_end();
    out.object_end();
}
fn owner_pool_set(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, _budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::PoolSet { project, person } = value else {
        unreachable!("dispatched outcome")
    };

    out.object_start();
    out.key(b"pool_set");
    out.object_start();
    out.key(b"project");
    out.unsigned(u64::from(*project));
    out.key(b"person");
    out.unsigned(*(person));
    out.object_end();
    out.object_end();
}
fn owner_escalation_decided(value: &jig_client::contract::OwnerOutcome, out: &mut Encoder, budget: &mut super::Cut) {
    let jig_client::contract::OwnerOutcome::EscalationDecided { task, revision, by, at, choice } = value else {
        unreachable!("dispatched outcome")
    };

    out.object_start();
    out.key(b"escalation_decided");
    out.object_start();
    out.key(b"task");
    out.unsigned(*(task));
    out.key(b"revision");
    super::encode_revision(*revision, out, budget);
    out.key(b"by");
    out.unsigned(*(by));
    out.key(b"at");
    out.unsigned((at).as_nanos());
    out.key(b"choice");
    encode_owner_escalation_choice(*choice, out, budget);
    out.object_end();
    out.object_end();
}
pub(super) fn encode_owner_proposal_choice(
    value: jig_client::contract::OwnerProposalChoice,
    out: &mut Encoder,
    _budget: &mut super::Cut,
) {
    match value {
        jig_client::contract::OwnerProposalChoice::Accepted => out.string(b"accepted"),
        jig_client::contract::OwnerProposalChoice::Rejected => out.string(b"rejected"),
        jig_client::contract::OwnerProposalChoice::Passed => out.string(b"passed"),
        jig_client::contract::OwnerProposalChoice::Withdrawn => out.string(b"withdrawn"),
        jig_client::contract::OwnerProposalChoice::Stale => out.string(b"stale"),
    }
}

pub(super) fn encode_seen_through(
    value: &jig_client::contract::SeenThrough,
    out: &mut Encoder,
    _budget: &mut super::Cut,
) {
    match value {
        jig_client::contract::SeenThrough::Results { through } => {
            out.object_start();
            out.key(b"results");
            out.object_start();
            out.key(b"through");
            out.unsigned(*(through));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::SeenThrough::Conversation { task, through } => {
            out.object_start();
            out.key(b"conversation");
            out.object_start();
            out.key(b"task");
            out.unsigned(*(task));
            out.key(b"through");
            out.unsigned(*(through));
            out.object_end();
            out.object_end();
        }
    }
}

fn encode_failed_hosts(hosts: &[jig_client::contract::AccountHostFailure], out: &mut Encoder, budget: &mut super::Cut) {
    out.key(b"failed_hosts");
    out.array_start();
    for host in hosts {
        out.object_start();
        out.key(b"name");
        budget.words(&host.name, out);
        out.key(b"why");
        out.string(match host.why {
            jig_client::contract::AccountHostWhy::NotInContact => b"not_in_contact",
            jig_client::contract::AccountHostWhy::Lost => b"lost",
            jig_client::contract::AccountHostWhy::Failed => b"failed",
        });
        out.object_end();
    }
    out.array_end();
}

fn encode_moved_in(account: u32, renewed: bool, out: &mut Encoder) {
    out.object_start();
    out.key(b"account_moved_in");
    out.object_start();
    out.key(b"account");
    out.unsigned(u64::from(account));
    out.key(b"renewed");
    out.boolean(renewed);
    out.object_end();
    out.object_end();
}

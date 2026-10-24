//! Integer JSON for supplied read facts.
use skein_json::writer::Encoder;
#[expect(clippy::too_many_lines, reason = "one exhaustive encoding of jig's owner message vocabulary")]
pub(super) fn encode_owner_message_kind(
    value: &jig_client::contract::OwnerMessageKind,
    out: &mut Encoder,
    budget: &mut super::Cut,
) {
    match value {
        jig_client::contract::OwnerMessageKind::Escalation { task, revision } => {
            out.object_start();
            out.key(b"escalation");
            out.object_start();
            out.key(b"task");
            out.unsigned(*(task));
            out.key(b"revision");
            out.unsigned(*(revision));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerMessageKind::Proposal { proposer, proposal, kind } => {
            encode_message_proposal(*proposer, *proposal, *kind, out, budget);
        }
        jig_client::contract::OwnerMessageKind::PersonStalled { task } => {
            out.object_start();
            out.key(b"person_stalled");
            out.object_start();
            out.key(b"task");
            out.unsigned(*(task));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerMessageKind::ProposalDecision { proposal, accepted } => {
            out.object_start();
            out.key(b"proposal_decision");
            out.object_start();
            out.key(b"proposal");
            out.unsigned(*(proposal));
            out.key(b"accepted");
            out.boolean(*(accepted));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerMessageKind::Words => out.string(b"words"),
        jig_client::contract::OwnerMessageKind::EffectNotMade { entry } => {
            out.object_start();
            out.key(b"effect_not_made");
            out.object_start();
            out.key(b"entry");
            out.unsigned(*(entry));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerMessageKind::Amendment { revision } => {
            out.object_start();
            out.key(b"amendment");
            out.object_start();
            out.key(b"revision");
            out.unsigned(*(revision));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerMessageKind::Met { dependency, removed } => {
            out.object_start();
            out.key(b"met");
            out.object_start();
            out.key(b"dependency");
            out.unsigned(*dependency);
            out.key(b"removed");
            out.boolean(*removed);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerMessageKind::Question => out.string(b"question"),
        jig_client::contract::OwnerMessageKind::Answer { question } => {
            out.object_start();
            out.key(b"answer");
            out.object_start();
            out.key(b"question");
            out.unsigned(*(question));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerMessageKind::Notice { subscription, target, state } => {
            encode_message_notice(*subscription, *target, *state, out, budget);
        }
        jig_client::contract::OwnerMessageKind::Timer { subscription } => {
            out.object_start();
            out.key(b"timer");
            out.object_start();
            out.key(b"subscription");
            out.unsigned(*(subscription));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerMessageKind::News { subscription, class } => {
            out.object_start();
            out.key(b"news");
            out.object_start();
            out.key(b"subscription");
            out.unsigned(*(subscription));
            out.key(b"class");
            encode_owner_news_class(*class, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerMessageKind::Result(field_0) => {
            out.object_start();
            out.key(b"result");
            encode_owner_word_result(field_0, out, budget);
            out.object_end();
        }
    }
}

pub(super) fn encode_owner_message_proposal_kind(
    value: jig_client::contract::OwnerMessageProposalKind,
    out: &mut Encoder,
    _budget: &mut super::Cut,
) {
    let value = &value;
    match value {
        jig_client::contract::OwnerMessageProposalKind::Settings => out.string(b"settings"),
        jig_client::contract::OwnerMessageProposalKind::Batch => out.string(b"batch"),
        jig_client::contract::OwnerMessageProposalKind::Effect => out.string(b"effect"),
        jig_client::contract::OwnerMessageProposalKind::Amend => out.string(b"amend"),
        jig_client::contract::OwnerMessageProposalKind::Widen => out.string(b"widen"),
        jig_client::contract::OwnerMessageProposalKind::Release => out.string(b"release"),
    }
}

pub(super) fn encode_owner_news_class(
    value: jig_client::contract::OwnerNewsClass,
    out: &mut Encoder,
    _budget: &mut super::Cut,
) {
    let value = &value;
    match value {
        jig_client::contract::OwnerNewsClass::Wakes => out.string(b"wakes"),
        jig_client::contract::OwnerNewsClass::Kept => out.string(b"kept"),
        jig_client::contract::OwnerNewsClass::Dropped => out.string(b"dropped"),
    }
}

pub(super) fn encode_owner_notice_state(
    value: jig_client::contract::OwnerNoticeState,
    out: &mut Encoder,
    _budget: &mut super::Cut,
) {
    let value = &value;
    match value {
        jig_client::contract::OwnerNoticeState::Held => out.string(b"held"),
        jig_client::contract::OwnerNoticeState::Done => out.string(b"done"),
        jig_client::contract::OwnerNoticeState::Failed => out.string(b"failed"),
        jig_client::contract::OwnerNoticeState::Cancelled => out.string(b"cancelled"),
    }
}

pub(super) fn encode_owner_word_result(
    value: &jig_client::contract::OwnerWordResult,
    out: &mut Encoder,
    budget: &mut super::Cut,
) {
    match value {
        jig_client::contract::OwnerWordResult::Approved { state, words_present } => {
            out.object_start();
            out.key(b"approved");
            out.object_start();
            out.key(b"state");
            budget.words((state).bytes(), out);
            out.key(b"words_present");
            out.boolean(*(words_present));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerWordResult::Report => out.string(b"report"),
        jig_client::contract::OwnerWordResult::Verdict { code, fields } => {
            out.object_start();
            out.key(b"verdict");
            out.object_start();
            out.key(b"code");
            out.unsigned(u64::from(*(code)));
            out.key(b"fields");
            out.array_start();
            for item_1 in fields {
                super::encode_result_field(item_1, out, budget);
            }
            out.array_end();
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerWordResult::Change { connector, kind, resource } => {
            out.object_start();
            out.key(b"change");
            out.object_start();
            out.key(b"connector");
            out.unsigned(u64::from(*(connector)));
            out.key(b"kind");
            out.unsigned(u64::from(*(kind)));
            out.key(b"resource");
            out.unsigned(*(resource));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::OwnerWordResult::Failed => out.string(b"failed"),
        jig_client::contract::OwnerWordResult::Cancelled => out.string(b"cancelled"),
    }
}

fn encode_message_proposal(
    proposer: u64,
    proposal: u64,
    kind: jig_client::contract::OwnerMessageProposalKind,
    out: &mut Encoder,
    budget: &mut super::Cut,
) {
    out.object_start();
    out.key(b"proposal");
    out.object_start();
    out.key(b"proposer");
    out.unsigned(proposer);
    out.key(b"proposal");
    out.unsigned(proposal);
    out.key(b"kind");
    encode_owner_message_proposal_kind(kind, out, budget);
    out.object_end();
    out.object_end();
}

fn encode_message_notice(
    subscription: u64,
    target: u64,
    state: jig_client::contract::OwnerNoticeState,
    out: &mut Encoder,
    budget: &mut super::Cut,
) {
    out.object_start();
    out.key(b"notice");
    out.object_start();
    out.key(b"subscription");
    out.unsigned(subscription);
    out.key(b"target");
    out.unsigned(target);
    out.key(b"state");
    encode_owner_notice_state(state, out, budget);
    out.object_end();
    out.object_end();
}

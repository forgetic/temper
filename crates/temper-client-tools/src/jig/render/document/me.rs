//! Concrete integer JSON for the client contract (contract.md, 5).
use skein_json::writer::Encoder;
pub(super) fn encode_live_delegate(
    value: &jig_client::contract::LiveDelegate,
    out: &mut Encoder,
    budget: &mut super::Cut,
) {
    out.object_start();
    out.key(b"chip");
    super::encode_chip(&value.chip, out, budget);
    out.key(b"takeover_budget");
    out.unsigned(value.takeover_budget);
    out.object_end();
}

pub(super) fn encode_malformed(value: jig_client::contract::Malformed, out: &mut Encoder, _budget: &mut super::Cut) {
    let value = &value;
    match value {
        jig_client::contract::Malformed::Settings(_) | jig_client::contract::Malformed::SettingsUnchanged => {
            unreachable!("the tools do not request settings")
        }
        jig_client::contract::Malformed::ProjectMismatch => out.string(b"project_mismatch"),
        jig_client::contract::Malformed::TwoAsAllowed => out.string(b"two_as_allowed"),
        jig_client::contract::Malformed::ConversationNotAgent => out.string(b"conversation_not_agent"),
        jig_client::contract::Malformed::Name(rule) => {
            out.object_start();
            out.key(b"name");
            out.string(match rule {
                jig_client::contract::NameRule::Empty => b"empty",
                jig_client::contract::NameRule::Long => b"long",
                jig_client::contract::NameRule::Control => b"control",
                jig_client::contract::NameRule::Space => b"space",
                jig_client::contract::NameRule::Digits => b"digits",
            });
            out.object_end();
        }
        jig_client::contract::Malformed::Running => out.string(b"running"),
        jig_client::contract::Malformed::Cycle => out.string(b"cycle"),
        jig_client::contract::Malformed::EndedBy => out.string(b"ended_by"),
        jig_client::contract::Malformed::ProposedAsAllowed => out.string(b"proposed_as_allowed"),
    }
}

pub(super) fn encode_me(
    value: &jig_client::contract::Me,
    out: &mut Encoder,
    budget: &mut super::Cut,
    limits: &super::Limits,
) {
    let digest = budget.digest.take();
    out.object_start();
    out.key(b"party");
    encode_me_party(&value.party, out, budget);
    out.key(b"projects");
    out.array_start();
    for item_1 in &value.projects {
        encode_me_project(item_1, out, budget);
    }
    out.array_end();
    out.key(b"needs");
    encode_needs(&value.needs, out, budget);
    out.key(b"needs_total");
    out.unsigned(u64::from(value.needs_total));
    out.key(b"updates");
    super::encode_count(value.updates, out, budget);
    out.key(b"results_seen");
    out.unsigned(value.results_seen);
    let mut window = super::offers::me_window(limits);
    super::offers::fields(&value.providers, value.providers_cut, value.key_bytes, out, budget, &mut window);
    out.key(b"cut");
    out.boolean(value.cut || window.cut || budget.left > 0);
    out.key(b"retention");
    out.unsigned(value.retention.as_nanos());
    out.key(b"deployment_role");
    match &value.deployment_role {
        Some(item_2) => {
            super::encode_held_role(*(item_2), out, budget);
        }
        None => out.null(),
    }
    out.key(b"bytes_left");
    out.unsigned(budget.left);
    super::subject_end(digest, out);
}

pub(super) fn encode_me_party(value: &jig_client::contract::MeParty, out: &mut Encoder, budget: &mut super::Cut) {
    out.object_start();
    out.key(b"number");
    out.unsigned(value.number);
    out.key(b"kind");
    super::encode_party_kind(value.kind, out, budget);
    out.key(b"login");
    super::encode_document_text(&value.login, out, budget);
    out.key(b"name");
    super::encode_document_text(&value.name, out, budget);
    out.object_end();
}

pub(super) fn encode_me_project(value: &jig_client::contract::MeProject, out: &mut Encoder, budget: &mut super::Cut) {
    out.object_start();
    out.key(b"project");
    out.unsigned(u64::from(value.project));
    out.key(b"name");
    super::encode_document_text(&value.name, out, budget);
    out.key(b"role");
    super::encode_held_role(value.role, out, budget);
    out.key(b"pool");
    match &value.pool {
        Some(item_2) => {
            super::encode_pool_ledger(item_2, out, budget);
        }
        None => out.null(),
    }
    out.key(b"templates");
    out.array_start();
    for item_3 in &value.templates {
        super::encode_template(item_3, out, budget);
    }
    out.array_end();
    out.key(b"creating");
    match &value.creating {
        Some(item_4) => {
            super::encode_creating(item_4, out, budget);
        }
        None => out.null(),
    }
    out.object_end();
}

pub(super) fn encode_moved(value: &jig_client::contract::Moved, out: &mut Encoder, budget: &mut super::Cut) {
    match value {
        jig_client::contract::Moved::Hold { reason } => {
            out.object_start();
            out.key(b"hold");
            out.object_start();
            out.key(b"reason");
            match reason {
                Some(item_1) => {
                    super::encode_current_hold(item_1, out, budget);
                }
                None => out.null(),
            }
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Moved::Spec => out.string(b"spec"),
        jig_client::contract::Moved::PersonTask => out.string(b"person_task"),
        jig_client::contract::Moved::Note => out.string(b"note"),
        jig_client::contract::Moved::Waiting => out.string(b"waiting"),
    }
}

pub(super) fn encode_needs(value: &jig_client::contract::Needs, out: &mut Encoder, _budget: &mut super::Cut) {
    out.object_start();
    out.key(b"questions");
    out.unsigned(u64::from(value.questions));
    out.key(b"proposals");
    out.unsigned(u64::from(value.proposals));
    out.key(b"escalations");
    out.unsigned(u64::from(value.escalations));
    out.key(b"person_tasks");
    out.unsigned(u64::from(value.person_tasks));
    out.object_end();
}

pub(super) fn encode_offered(value: &jig_client::contract::Offered, out: &mut Encoder, budget: &mut super::Cut) {
    out.object_start();
    out.key(b"effect");
    match &value.effect {
        Some(item_1) => {
            super::encode_effect(item_1, out, budget);
        }
        None => out.null(),
    }
    out.key(b"act");
    super::encode_act_kind(value.act, out, budget);
    out.key(b"admission");
    super::encode_admission(&value.admission, out, budget);
    out.object_end();
}

pub(super) fn encode_owner_authority(
    value: &jig_client::contract::OwnerAuthority,
    out: &mut Encoder,
    budget: &mut super::Cut,
) {
    out.object_start();
    out.key(b"tools");
    out.unsigned(value.tools);
    out.key(b"grants");
    out.array_start();
    for item_1 in &value.grants {
        super::encode_owner_grant(item_1, out, budget);
    }
    out.array_end();
    out.key(b"delegation");
    super::encode_owner_delegation(&value.delegation, out, budget);
    out.key(b"spend");
    out.unsigned(value.spend);
    out.key(b"deadline");
    match &value.deadline {
        Some(item_2) => {
            out.unsigned((item_2).as_nanos());
        }
        None => out.null(),
    }
    out.key(b"notes");
    out.unsigned(u64::from(value.notes));
    out.key(b"note_resources");
    out.array_start();
    for item_3 in &value.note_resources {
        super::encode_owner_resource_scope(item_3, out, budget);
    }
    out.array_end();
    out.object_end();
}

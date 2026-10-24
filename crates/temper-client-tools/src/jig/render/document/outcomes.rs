//! Concrete integer JSON for the client contract (contract.md, 5).
use skein_json::writer::Encoder;
pub(super) fn encode_proposer(value: &jig_client::contract::Proposer, out: &mut Encoder, _budget: &mut super::Cut) {
    match value {
        jig_client::contract::Proposer::Task(field_0) => {
            out.object_start();
            out.key(b"task");
            out.unsigned(*(field_0));
            out.object_end();
        }
        jig_client::contract::Proposer::Party(field_0) => {
            out.object_start();
            out.key(b"party");
            out.unsigned(*(field_0));
            out.object_end();
        }
    }
}

pub(super) fn encode_recipient(value: &jig_client::contract::Recipient, out: &mut Encoder, _budget: &mut super::Cut) {
    match value {
        jig_client::contract::Recipient::Task(field_0) => {
            out.object_start();
            out.key(b"task");
            out.unsigned(*(field_0));
            out.object_end();
        }
        jig_client::contract::Recipient::Party(field_0) => {
            out.object_start();
            out.key(b"party");
            out.unsigned(*(field_0));
            out.object_end();
        }
        jig_client::contract::Recipient::Role { project, role } => {
            out.object_start();
            out.key(b"role");
            out.object_start();
            out.key(b"project");
            out.unsigned(u64::from(*(project)));
            out.key(b"role");
            out.unsigned(u64::from(*(role)));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Recipient::Roles { project, roles } => {
            out.object_start();
            out.key(b"roles");
            out.object_start();
            out.key(b"project");
            out.unsigned(u64::from(*(project)));
            out.key(b"roles");
            out.array_start();
            for item_1 in roles {
                out.unsigned(u64::from(*(item_1)));
            }
            out.array_end();
            out.object_end();
            out.object_end();
        }
    }
}

#[expect(clippy::too_many_lines, reason = "one exhaustive encoding of jig's refusal vocabulary")]
pub(super) fn encode_refusal(value: &jig_client::contract::Refusal, out: &mut Encoder, budget: &mut super::Cut) {
    match value {
        jig_client::contract::Refusal::Stale(_) | jig_client::contract::Refusal::Full(_) => {
            unreachable!("the tools do not request settings")
        }
        jig_client::contract::Refusal::AccountMove { why } => {
            use jig_client::contract::AccountMoveWhy;
            out.object_start();
            out.key(b"account_move");
            out.object_start();
            out.key(b"why");
            out.string(match why {
                AccountMoveWhy::NotSignedIn => b"not_signed_in",
                AccountMoveWhy::NotOurs => b"not_ours",
                AccountMoveWhy::Held => b"held",
                AccountMoveWhy::Cancelled => b"cancelled",
                AccountMoveWhy::Moving => b"moving",
            });
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::SignInRedirect { why } => {
            use jig_client::contract::SignInRedirectWhy;
            out.object_start();
            out.key(b"sign_in_redirect");
            out.object_start();
            out.key(b"why");
            out.string(match why {
                SignInRedirectWhy::NotWaiting => b"not_waiting",
                SignInRedirectWhy::Address => b"address",
                SignInRedirectWhy::State => b"state",
                SignInRedirectWhy::TooMany => b"too_many",
            });
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::NoEscalationHolder => out.string(b"no_escalation_holder"),
        jig_client::contract::Refusal::Malformed { at, why } => {
            out.object_start();
            out.key(b"malformed");
            out.object_start();
            out.key(b"at");
            out.unsigned(u64::from(*(at)));
            out.key(b"why");
            super::encode_malformed(*(why), out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Role { held } => {
            out.object_start();
            out.key(b"role");
            out.object_start();
            out.key(b"held");
            match held {
                Some(item_1) => super::encode_held_role(*(item_1), out, budget),
                None => out.null(),
            }
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Taken { by } => {
            out.object_start();
            out.key(b"taken");
            out.object_start();
            out.key(b"by");
            out.unsigned(*(by));
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::NotWaiting => out.string(b"not_waiting"),
        jig_client::contract::Refusal::NoFurther => out.string(b"no_further"),
        jig_client::contract::Refusal::Authority { lacking, proposable } => {
            out.object_start();
            out.key(b"authority");
            out.object_start();
            out.key(b"lacking");
            super::encode_lacking(lacking, out, budget);
            out.key(b"proposable");
            match proposable {
                Some(item_2) => {
                    out.boolean(*(item_2));
                }
                None => out.null(),
            }
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Limit { which } => {
            out.object_start();
            out.key(b"limit");
            out.object_start();
            out.key(b"which");
            super::encode_limit(which, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Moved { now, what } => {
            out.object_start();
            out.key(b"moved");
            out.object_start();
            out.key(b"now");
            super::encode_revision(*(now), out, budget);
            out.key(b"what");
            super::encode_moved(what, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Ended { how } => {
            out.object_start();
            out.key(b"ended");
            out.object_start();
            out.key(b"how");
            super::encode_ending(how, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Account { account, why } => super::llm::refusal(*account, *why, out),
        jig_client::contract::Refusal::Llm { model, why } => {
            out.object_start();
            out.key(b"llm");
            out.object_start();
            out.key(b"model");
            out.string(model.bytes());
            out.key(b"why");
            encode_llm_why(*(why), out);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Circle { tasks, cut } => {
            out.object_start();
            out.key(b"circle");
            out.object_start();
            out.key(b"tasks");
            out.array_start();
            for task in tasks {
                out.unsigned(*task);
            }
            out.array_end();
            out.key(b"cut");
            out.boolean(*cut);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Dependency { dependency, how } => {
            out.object_start();
            out.key(b"dependency");
            out.object_start();
            out.key(b"dependency");
            out.unsigned(*dependency);
            out.key(b"how");
            super::owner_links::encode_owner_task_status(*how, out, budget);
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Kept { pending } => {
            out.object_start();
            out.key(b"kept");
            out.object_start();
            out.key(b"pending");
            out.array_start();
            for task in pending {
                out.unsigned(*task);
            }
            out.array_end();
            out.object_end();
            out.object_end();
        }
        jig_client::contract::Refusal::Unknown => out.string(b"unknown"),
        jig_client::contract::Refusal::NotOffered => out.string(b"not_offered"),
        jig_client::contract::Refusal::NameTaken => out.string(b"name_taken"),
        jig_client::contract::Refusal::Provider => out.string(b"provider"),
        jig_client::contract::Refusal::KeyConflict => out.string(b"key_conflict"),
    }
}

fn encode_llm_why(value: jig_client::contract::LlmWhy, out: &mut Encoder) {
    match value {
        jig_client::contract::LlmWhy::Unconfigured => out.string(b"unconfigured"),
        jig_client::contract::LlmWhy::Effort => out.string(b"effort"),
        jig_client::contract::LlmWhy::Charter => out.string(b"charter"),
        jig_client::contract::LlmWhy::Authority => out.string(b"authority"),
        jig_client::contract::LlmWhy::Procedure => out.string(b"procedure"),
        jig_client::contract::LlmWhy::Repeated => out.string(b"repeated"),
    }
}

pub(super) fn encode_release_effect(
    value: jig_client::contract::ReleaseEffect,
    out: &mut Encoder,
    _budget: &mut super::Cut,
) {
    let value = &value;
    match value {
        jig_client::contract::ReleaseEffect::Judged => out.string(b"judged"),
        jig_client::contract::ReleaseEffect::RunAgain => out.string(b"run_again"),
        jig_client::contract::ReleaseEffect::Uncertain => out.string(b"uncertain"),
    }
}

pub(super) fn encode_result(value: &jig_client::contract::Result, out: &mut Encoder, budget: &mut super::Cut) {
    out.object_start();
    out.key(b"task");
    super::encode_chip(&value.task, out, budget);
    out.key(b"ending");
    super::encode_ending(&value.ending, out, budget);
    out.key(b"value");
    match &value.value {
        Some(item_1) => {
            super::encode_result_value(item_1, out, budget);
        }
        None => out.null(),
    }
    out.key(b"at");
    match &value.at {
        Some(item_2) => {
            out.unsigned((item_2).as_nanos());
        }
        None => out.null(),
    }
    out.key(b"details");
    match &value.details {
        Some(item_3) => {
            super::encode_result_details(item_3, out, budget);
        }
        None => out.null(),
    }
    out.key(b"acts");
    out.array_start();
    for item_4 in &value.acts {
        super::encode_offered(item_4, out, budget);
    }
    out.array_end();
    out.object_end();
}

//! Offers keep the engine's order and admission facts.
use super::Text;
use jig_client::{
    contract::{
        ActKind, Admission, Beyond, Bounds, Me, OwnerAuthorityDocument, OwnerAuthorityExecutor, OwnerGrant,
        OwnerPattern, OwnerPatternEnd, Recipient, Refusal, Task,
    },
    words::Word,
};
use skein_ui::phrases::{Hole, Phrasebook};
/// The closed act vocabulary in the tools' words.
#[must_use]
pub fn name(act: ActKind) -> &'static [u8] {
    match act {
        ActKind::Message => b"message",
        ActKind::Answer => b"answer",
        ActKind::Take => b"take",
        ActKind::HandBack => b"hand-back",
        ActKind::Accept => b"accept",
        ActKind::Reject => b"reject",
        ActKind::PassUp => b"pass-up",
        ActKind::Release => b"release",
        ActKind::LeaveHeld => b"leave-held",
        ActKind::Amend => b"amend",
        ActKind::Stop => b"stop",
        ActKind::Cancel => b"cancel",
        ActKind::Move => b"move",
        ActKind::Reorder => b"reorder",
        ActKind::Seen => b"seen",
        ActKind::EditNote => b"edit-note",
        ActKind::DeleteNote => b"delete-note",
        ActKind::Adopt => b"adopt",
        ActKind::ChangePolicy => b"change-policy",
        ActKind::SetRoles => b"set-roles",
        ActKind::MakeService => b"make-service",
        ActKind::Rename => b"rename",
        ActKind::SetPool => b"set-pool",
        ActKind::Withdraw => b"withdraw",
    }
}
pub fn text(task: &Task, me: Option<&Me>, book: &Phrasebook, out: &mut Text) {
    out.put(b"acts:\n");
    for offer in &task.acts {
        out.put(b"  ");
        if !has_tool(task, offer.act) {
            out.put(name(offer.act));
            out.put(b" (no tool)\n");
            continue;
        }
        match &offer.admission {
            Admission::Refused(why) => {
                out.put(name(offer.act));
                out.put(b": ");
                refusal(why, book, out);
            }
            Admission::Allowed => call(task, offer.act, out),
            Admission::Proposed { to } => {
                call(task, offer.act, out);
                proposed(to, me, out);
            }
            Admission::Bounded { within, beyond } => {
                call(task, offer.act, out);
                out.put(b" within ");
                bounds(within, out);
                match beyond {
                    Beyond::Proposed { to } => proposed(to, me, out),
                    Beyond::Refused(why) => {
                        out.put(b"; refused beyond them: ");
                        refusal(why, book, out);
                    }
                }
            }
        }
        out.put(b"\n");
    }
}
fn has_tool(_task: &Task, act: ActKind) -> bool {
    match act {
        ActKind::Message
        | ActKind::Answer
        | ActKind::Accept
        | ActKind::Reject
        | ActKind::PassUp
        | ActKind::Release
        | ActKind::LeaveHeld
        | ActKind::Stop
        | ActKind::Cancel
        | ActKind::Adopt
        | ActKind::Amend => true,
        ActKind::Take
        | ActKind::HandBack
        | ActKind::Move
        | ActKind::Reorder
        | ActKind::Seen
        | ActKind::EditNote
        | ActKind::DeleteNote
        | ActKind::ChangePolicy
        | ActKind::SetRoles
        | ActKind::Rename
        | ActKind::MakeService
        | ActKind::SetPool
        | ActKind::Withdraw => false,
    }
}
fn call(task: &Task, act: ActKind, out: &mut Text) {
    match act {
        ActKind::Message | ActKind::Answer | ActKind::Stop | ActKind::Cancel | ActKind::Release => {
            out.put(name(act));
            out.put(b" ");
            out.number(task.chip.task);
            match act {
                ActKind::Message => out.put(b" TEXT"),
                ActKind::Answer => out.put(b" Q TEXT"),
                ActKind::Cancel => out.put(b" REASON"),
                ActKind::Release => hold(task, out),
                ActKind::Take
                | ActKind::HandBack
                | ActKind::Accept
                | ActKind::Reject
                | ActKind::PassUp
                | ActKind::LeaveHeld
                | ActKind::Amend
                | ActKind::Stop
                | ActKind::Move
                | ActKind::Reorder
                | ActKind::Seen
                | ActKind::EditNote
                | ActKind::DeleteNote
                | ActKind::Adopt
                | ActKind::ChangePolicy
                | ActKind::SetRoles
                | ActKind::Rename
                | ActKind::MakeService
                | ActKind::SetPool
                | ActKind::Withdraw => {}
            }
        }
        ActKind::Accept | ActKind::Reject => {
            proposal(task, out);
            out.put(if act == ActKind::Accept { b" accept" } else { b" reject REASON" });
        }
        ActKind::PassUp if task.hold.is_none() => {
            proposal(task, out);
            out.put(b" pass-up REASON");
        }
        ActKind::PassUp | ActKind::LeaveHeld => {
            out.put(b"decide ");
            out.number(task.chip.task);
            hold(task, out);
            out.put(if act == ActKind::PassUp { b" pass-up REASON" } else { b" leave REASON" });
        }
        ActKind::Adopt => {
            out.put(b"adopt --project ");
            out.number(u64::from(task.project));
            out.put(b" --owner O --name N --landing B");
        }
        ActKind::Amend => {
            out.put(b"amend ");
            out.number(task.chip.task);
            out.put(b" --rev ");
            out.number(task.spec_revision.value);
            out.put(b" CHANGE... REASON");
        }
        // Non-tool offers have already been printed without their admission.
        ActKind::Take
        | ActKind::HandBack
        | ActKind::Move
        | ActKind::Reorder
        | ActKind::Seen
        | ActKind::EditNote
        | ActKind::DeleteNote
        | ActKind::ChangePolicy
        | ActKind::SetRoles
        | ActKind::Rename
        | ActKind::MakeService
        | ActKind::SetPool
        | ActKind::Withdraw => return,
    }
    out.put(b" --key NAME");
}
fn hold(task: &Task, out: &mut Text) {
    out.put(b" --hold ");
    match &task.hold {
        Some(hold) => out.number(hold.revision().value),
        None => out.put(b"R"),
    }
}
fn proposal(task: &Task, out: &mut Text) {
    out.put(b"decide ");
    match task.proposals.get(0) {
        Some(key) => {
            match key.proposer {
                jig_client::contract::Proposer::Task(number) => {
                    out.put(b"t");
                    out.number(number);
                }
                jig_client::contract::Proposer::Party(number) => {
                    out.put(b"p");
                    out.number(number);
                }
            }
            out.put(b"/");
            out.number(key.number);
        }
        None => {
            out.put(b"t");
            out.number(task.chip.task);
            out.put(b"/K");
        }
    }
}
pub(super) fn proposed(to: &Recipient, me: Option<&Me>, out: &mut Text) {
    out.put(b" (proposed to ");
    recipient(to, me, out);
    out.put(b")");
}
pub fn recipient(to: &Recipient, me: Option<&Me>, out: &mut Text) {
    match to {
        Recipient::Party(number) => party(*number, me, out),
        Recipient::Task(number) => {
            out.put(b"task ");
            out.number(*number);
        }
        Recipient::Role { project, role } => {
            out.put(b"role ");
            out.number(u64::from(*role));
            out.put(b" in project ");
            out.number(u64::from(*project));
        }
        Recipient::Roles { project, roles } => {
            out.put(b"roles ");
            for (index, role) in roles.iter().enumerate() {
                if index > 0 {
                    out.put(b", ");
                }
                out.number(u64::from(*role));
            }
            out.put(b" in project ");
            out.number(u64::from(*project));
        }
    }
}
pub fn party(number: u64, me: Option<&Me>, out: &mut Text) {
    if let Some(me) = me
        && me.party.number == number
    {
        out.put(b"you");
        return;
    }
    out.put(b"party ");
    out.number(number);
}
pub fn refusal(why: &Refusal, book: &Phrasebook, out: &mut Text) {
    let word = Word::refusal(why);
    let text = match why {
        Refusal::Stale(_) | Refusal::Full(_) => {
            unreachable!("the tools do not request settings")
        }
        Refusal::Account { account, .. } => book.say(0, word.id(), &[Hole::Number(u64::from(*account))]),
        Refusal::Taken { by } => book.say(0, word.id(), &[Hole::Number(*by)]),
        Refusal::Llm { model, .. } => book.say(0, word.id(), &[Hole::Name(model.bytes().into())]),
        Refusal::NoEscalationHolder
        | Refusal::Malformed { .. }
        | Refusal::Role { .. }
        | Refusal::NotWaiting
        | Refusal::NoFurther
        | Refusal::Authority { .. }
        | Refusal::Limit { .. }
        | Refusal::Moved { .. }
        | Refusal::Ended { .. }
        | Refusal::Unknown
        | Refusal::NotOffered
        | Refusal::KeyConflict
        | Refusal::Circle { .. }
        | Refusal::Dependency { .. }
        | Refusal::Kept { .. }
        | Refusal::NameTaken
        | Refusal::AccountMove { .. }
        | Refusal::SignInRedirect { .. }
        | Refusal::Provider => book.say(0, word.id(), &[]),
    }
    .expect("checked refusal phrase")
    .text;
    out.put(&text);
}
pub fn bounds(value: &Bounds, out: &mut Text) {
    match value {
        Bounds::Create(bound) => {
            create_bounds(bound, out);
        }
        Bounds::Amend { budget_left, grants, implications, authority: within, ceiling } => {
            authority(within, out);
            out.put(b"; budget left ");
            out.number(*budget_left);
            out.put(b"; held grants");
            for grant in grants {
                out.put(b" ");
                grant_text(grant, out);
            }
            out.put(b"; implications ");
            for implication in implications {
                out.number(u64::from(implication.connector));
                out.put(b"/");
                out.number(u64::from(implication.kind));
                out.put(b"->");
                out.number(u64::from(implication.implies));
                out.put(b" ");
            }
            out.put(b"; ceiling ");
            authority(ceiling, out);
        }
    }
}
pub(super) fn create_bounds(bound: &jig_client::contract::CreateBounds, out: &mut Text) {
    authority(&bound.authority, out);
    out.put(b"; pool left ");
    out.number(bound.pool_left);
    out.put(b"; budget ");
    out.number(bound.budget);
    out.put(b"; maximum budget ");
    out.number(bound.maximum_budget);
    out.put(b"; charters");
    for charter in &bound.charters {
        out.put(b" ");
        out.number(u64::from(*charter));
    }
    out.put(b"; procedures");
    for procedure in &bound.procedures {
        out.put(b" ");
        out.number(u64::from(procedure.connector));
        out.put(b"/");
        out.number(u64::from(procedure.code));
    }
    out.put(b"; latest ");
    wall(bound.latest, out);
    out.put(b"; maximum deadline ");
    wall(bound.maximum_deadline, out);
}
pub fn authority(document: &OwnerAuthorityDocument, out: &mut Text) {
    let value = &document.authority;
    out.put(b"authority { tools ");
    out.number(value.tools);
    out.put(b"; grants");
    for grant in &value.grants {
        out.put(b" ");
        grant_text(grant, out);
    }
    out.put(b"; delegation { kinds");
    for kind in &value.delegation.kinds {
        out.put(b" ");
        match kind {
            OwnerAuthorityExecutor::Charter(n) => {
                out.put(b"charter ");
                out.number(u64::from(*n));
            }
            OwnerAuthorityExecutor::Procedure(n) => {
                out.put(b"procedure ");
                out.number(u64::from(*n));
            }
            OwnerAuthorityExecutor::Role(n) => {
                out.put(b"role ");
                out.number(u64::from(*n));
            }
        }
    }
    out.put(b"; tasks ");
    out.number(u64::from(value.delegation.tasks));
    out.put(b"; depth ");
    out.number(u64::from(value.delegation.depth));
    out.put(b" }; spend ");
    out.number(value.spend);
    out.put(b"; deadline ");
    wall(value.deadline, out);
    out.put(b"; notes ");
    out.number(u64::from(value.notes));
    out.put(b"; note resources");
    for scope in &value.note_resources {
        out.put(b" ");
        out.number(u64::from(scope.connector));
        out.put(b":");
        pattern(&scope.pattern, out);
    }
    out.put(b" }");
    if document.cut {
        out.put(b" (cut by owner)");
    }
}
fn grant_text(grant: &OwnerGrant, out: &mut Text) {
    out.number(u64::from(grant.connector));
    out.put(b"/");
    out.number(u64::from(grant.kind));
    out.put(b":");
    pattern(&grant.pattern, out);
}
fn pattern(pattern: &OwnerPattern, out: &mut Text) {
    for segment in &pattern.segments {
        out.put(segment);
        out.put(b"/");
    }
    match &pattern.last {
        OwnerPatternEnd::Exact(value) => out.put(value),
        OwnerPatternEnd::Open(value) => {
            out.put(value);
            out.put(b"*");
        }
    }
}
pub fn wall(wall: Option<skein_lib::Wall>, out: &mut Text) {
    match wall {
        Some(wall) => out.number(wall.as_nanos()),
        None => out.put(b"none"),
    }
}

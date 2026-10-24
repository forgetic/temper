//! The proposal's task specifications, executors and authority are delivered facts.
use super::{P, phrase, specification, status, text};
use crate::words::{Word, Words};
use jig_client::contract as c;
use skein_ui::{Builder, Role, phrases::Hole};
fn batch(page: &c::ReadValue) -> Option<&skein_lib::List<c::ProposedTask>> {
    match page {
        c::ReadValue::OwnerProposal(proposal) => match &proposal.action {
            c::OwnerProposalAction::Batch(tasks) => Some(tasks),
            c::OwnerProposalAction::Settings(_)
            | c::OwnerProposalAction::Effect { .. }
            | c::OwnerProposalAction::ProcedureEffect { .. }
            | c::OwnerProposalAction::Amend { .. }
            | c::OwnerProposalAction::Widen { .. }
            | c::OwnerProposalAction::Release { .. } => None,
        },
        c::ReadValue::Proposal(proposal) => match &proposal.action {
            c::ProposalAction::Batch { tasks, .. } => Some(tasks),
            c::ProposalAction::Effect { .. } | c::ProposalAction::Amend { .. } | c::ProposalAction::Widen { .. } => {
                None
            }
        },
        c::ReadValue::Task(_)
        | c::ReadValue::Settings(_)
        | c::ReadValue::Setting(_)
        | c::ReadValue::Revision(_)
        | c::ReadValue::Revisions(_)
        | c::ReadValue::Tasks(_)
        | c::ReadValue::OwnerProject(_)
        | c::ReadValue::Historical(_)
        | c::ReadValue::Me(_)
        | c::ReadValue::Question(_)
        | c::ReadValue::Escalation(_)
        | c::ReadValue::OwnerEscalation(_)
        | c::ReadValue::PersonTask(_)
        | c::ReadValue::OwnerPersonTask(_)
        | c::ReadValue::Result(_)
        | c::ReadValue::Ended(_)
        | c::ReadValue::OwnerEnded(_)
        | c::ReadValue::History(_)
        | c::ReadValue::Turn(_)
        | c::ReadValue::Activity(_)
        | c::ReadValue::Inbox(_)
        | c::ReadValue::OwnerInbox(_)
        | c::ReadValue::Spend(_)
        | c::ReadValue::OwnerSpend(_)
        | c::ReadValue::Project(_)
        | c::ReadValue::Notes(_)
        | c::ReadValue::Deployment(_)
        | c::ReadValue::OwnerDeployment(_) => None,
    }
}
pub(super) fn reason(b: &mut Builder, page: &c::ReadValue) {
    // Batch headings need the phrasebook; facts draws them before its rows.
    if batch(page).is_none() {
        why(b, page);
    }
}
/// The place is the caller's: it sets the title's level and whether a kicker says the kind.
fn title(b: &mut Builder, tasks: &skein_lib::List<c::ProposedTask>, words: &Words, place: super::Place) {
    let level = place.level();
    if place.kicker() {
        b.open(Role::Paragraph);
        b.style(crate::look::PROPOSAL_KICKER);
        let kicker = words.say(Word::Proposal, &[]);
        b.name(&kicker);
        b.text(skein_ui::Run::Plain, &kicker);
        b.close();
    }
    for task in tasks {
        let written = task.spec.specification.words.bytes();
        let end = skein_lib::bytes::find(written, b"\n").unwrap_or(written.len());
        text(b, Role::Heading { level }, written.get(..end).expect("title prefix"));
        if end < written.len() {
            skein_ui::markdown::read(
                written.get(end..).expect("spec body"),
                level.checked_add(1).expect("proposal heading fits"),
                b,
            );
        }
    }
}
fn why(b: &mut Builder, page: &c::ReadValue) {
    match page {
        c::ReadValue::OwnerProposal(proposal) => super::document(b, &proposal.reason),
        c::ReadValue::Proposal(proposal) => text(b, Role::Paragraph, proposal.why.bytes()),
        c::ReadValue::Task(_)
        | c::ReadValue::Settings(_)
        | c::ReadValue::Setting(_)
        | c::ReadValue::Revision(_)
        | c::ReadValue::Revisions(_)
        | c::ReadValue::Tasks(_)
        | c::ReadValue::OwnerProject(_)
        | c::ReadValue::Historical(_)
        | c::ReadValue::Me(_)
        | c::ReadValue::Question(_)
        | c::ReadValue::Escalation(_)
        | c::ReadValue::OwnerEscalation(_)
        | c::ReadValue::PersonTask(_)
        | c::ReadValue::OwnerPersonTask(_)
        | c::ReadValue::Result(_)
        | c::ReadValue::Ended(_)
        | c::ReadValue::OwnerEnded(_)
        | c::ReadValue::History(_)
        | c::ReadValue::Turn(_)
        | c::ReadValue::Activity(_)
        | c::ReadValue::Inbox(_)
        | c::ReadValue::OwnerInbox(_)
        | c::ReadValue::Spend(_)
        | c::ReadValue::OwnerSpend(_)
        | c::ReadValue::Project(_)
        | c::ReadValue::Notes(_)
        | c::ReadValue::Deployment(_)
        | c::ReadValue::OwnerDeployment(_) => {}
    }
}
pub(super) fn facts(b: &mut Builder, page: &c::ReadValue, words: &Words, rows: u32, place: super::Place) {
    if let Some(tasks) = batch(page) {
        title(b, tasks, words, place);
        batch_facts(b, page, tasks, words, rows);
        return;
    }
    match page {
        c::ReadValue::OwnerProposal(proposal) => match &proposal.action {
            c::OwnerProposalAction::Batch(tasks) => proposed(b, tasks, words, rows),
            c::OwnerProposalAction::Settings(_) => {}
            c::OwnerProposalAction::Effect { part, authority: value, .. }
            | c::OwnerProposalAction::ProcedureEffect { part, authority: value, .. } => {
                crate::forge::owner_part(b, part, words);
                authority(b, value, words, rows);
            }
            c::OwnerProposalAction::Amend { task, before, after, .. } => {
                phrase(b, words, P::ProposalAmend, &[Hole::Number(task.number())]);
                specification(b, &after.spec, words);
                authority_change(b, &before.authority, &after.authority, words, rows);
            }
            c::OwnerProposalAction::Widen { task, before, after } => {
                phrase(b, words, P::ProposalTask, &[Hole::Number(task.number())]);
                authority_change(b, before, after, words, rows);
            }
            c::OwnerProposalAction::Release { task, hold } => {
                phrase(b, words, P::ProposalTask, &[Hole::Number(task.number())]);
                if let Some(hold) = hold {
                    held(b, hold, words, rows);
                }
            }
        },
        c::ReadValue::Proposal(proposal) => {
            match &proposal.action {
                c::ProposalAction::Batch { tasks, .. } => proposed(b, tasks, words, rows),
                c::ProposalAction::Effect { part } => crate::forge::drawn(b, part, words),
                c::ProposalAction::Amend { task, .. } => phrase(b, words, P::ProposalAmend, &[Hole::Number(*task)]),
                c::ProposalAction::Widen { .. } => phrase(b, words, P::Authority, &[]),
            }
            phrase(b, words, P::Spend, &[Hole::Number(proposal.needs.budget)]);
            phrase(
                b,
                words,
                P::Delegation,
                &[Hole::Number(u64::from(proposal.needs.tasks)), Hole::Number(u64::from(proposal.needs.depth))],
            );
            for grant in window(b, words, proposal.needs.grants.as_slice(), rows) {
                crate::forge::drawn(b, &c::Part::Kind { connector: grant.connector, kind: grant.kind }, words);
                pattern(b, &grant.pattern, words, rows);
            }
        }
        c::ReadValue::Task(task) => task_hold(b, task, words, rows),
        c::ReadValue::OwnerEscalation(escalation) => task_hold(b, &escalation.task, words, rows),
        c::ReadValue::Escalation(escalation) => {
            phrase(b, words, P::HoldRevision, &[Hole::Number(escalation.hold.revision.value)]);
        }
        c::ReadValue::Settings(_)
        | c::ReadValue::Setting(_)
        | c::ReadValue::Revision(_)
        | c::ReadValue::Revisions(_)
        | c::ReadValue::Tasks(_)
        | c::ReadValue::OwnerProject(_)
        | c::ReadValue::Historical(_)
        | c::ReadValue::Me(_)
        | c::ReadValue::Question(_)
        | c::ReadValue::PersonTask(_)
        | c::ReadValue::OwnerPersonTask(_)
        | c::ReadValue::Result(_)
        | c::ReadValue::Ended(_)
        | c::ReadValue::OwnerEnded(_)
        | c::ReadValue::History(_)
        | c::ReadValue::Turn(_)
        | c::ReadValue::Activity(_)
        | c::ReadValue::Inbox(_)
        | c::ReadValue::OwnerInbox(_)
        | c::ReadValue::Spend(_)
        | c::ReadValue::OwnerSpend(_)
        | c::ReadValue::Project(_)
        | c::ReadValue::Notes(_)
        | c::ReadValue::Deployment(_)
        | c::ReadValue::OwnerDeployment(_) => {}
    }
}
fn batch_facts(
    b: &mut Builder,
    page: &c::ReadValue,
    tasks: &skein_lib::List<c::ProposedTask>,
    words: &Words,
    rows: u32,
) {
    b.open(Role::Slot);
    b.style(crate::look::PROPOSAL_FACTS);
    text(b, Role::Term, &words.say(Word::ProposalWhy, &[]));
    b.open(Role::Description);
    why(b, page);
    b.close();
    proposed(b, tasks, words, rows);
    text(b, Role::Term, &words.say(Word::ProposalBudget, &[]));
    b.open(Role::Description);
    b.open(Role::Paragraph);
    let facts = jig_client::facts::decision(Some(page), c::ActKind::Accept);
    let mut first = true;
    for fact in &jig_client::words::decision(&facts, &words.book, words.language) {
        if !first {
            separator(b);
        }
        b.text(skein_ui::Run::Plain, fact);
        first = false;
    }
    if let c::ReadValue::Proposal(proposal) = page {
        fact(b, words, P::Spend, &[Hole::Number(proposal.needs.budget)], true);
        fact(
            b,
            words,
            P::Delegation,
            &[Hole::Number(u64::from(proposal.needs.tasks)), Hole::Number(u64::from(proposal.needs.depth))],
            true,
        );
        for grant in window_facts(b, words, proposal.needs.grants.as_slice(), rows, true) {
            separator(b);
            crate::forge::drawn(b, &c::Part::Kind { connector: grant.connector, kind: grant.kind }, words);
            pattern_facts(b, &grant.pattern, words, rows, true);
        }
    }
    b.close();
    b.close();
    b.close();
}

fn task_hold(b: &mut Builder, task: &c::Task, words: &Words, rows: u32) {
    match &task.hold {
        Some(c::TaskHold::Owner(hold)) => held(b, hold, words, rows),
        Some(c::TaskHold::Document(hold)) => phrase(b, words, P::HoldRevision, &[Hole::Number(hold.revision.value)]),
        None => {}
    }
}
fn held(b: &mut Builder, hold: &c::OwnerChipHold, words: &Words, _rows: u32) {
    super::hold(b, &hold.reason, words);
    phrase(b, words, P::HoldRevision, &[Hole::Number(hold.revision)]);
}
fn proposed(b: &mut Builder, tasks: &skein_lib::List<c::ProposedTask>, words: &Words, rows: u32) {
    for task in window(b, words, tasks.as_slice(), rows) {
        text(b, Role::Term, &words.say(Word::Goal, &[]));
        b.open(Role::Description);
        b.open(Role::Paragraph);
        b.text(skein_ui::Run::Plain, &words.say_card(P::ProposalTask, &[Hole::Number(task.number)]));
        specification_facts(b, &task.spec, words);
        match task.executor {
            c::OwnerExecutor::Agent { charter } => {
                fact(b, words, P::Charter, &[Hole::Number(u64::from(charter))], true);
            }
            c::OwnerExecutor::Procedure { code, .. } => {
                fact(b, words, P::ProcedureCode, &[Hole::Number(u64::from(code))], true);
            }
            c::OwnerExecutor::Person(_) => {
                separator(b);
                b.text(skein_ui::Run::Plain, &words.say(Word::PersonTask, &[]));
            }
        }
        authority_facts(b, &task.authority, words, rows, true);
        fact(b, words, P::ProposalBudget, &[Hole::Number(task.budget)], true);
        for dependency in &task.dependencies {
            fact(b, words, P::Input, &[Hole::Number(*dependency)], true);
        }
        if task.cut {
            fact(b, words, P::Cut, &[], true);
        }
        b.close();
        b.close();
        text(b, Role::Term, &words.say(Word::ProposalLanding, &[]));
        b.open(Role::Description);
        b.open(Role::Paragraph);
        for (at, resource) in window_facts(b, words, task.resources.as_slice(), rows, true).iter().enumerate() {
            if at != 0 {
                separator(b);
            }
            crate::forge::owner_part(b, resource, words);
        }
        if let Some(priority) = task.priority {
            if !task.resources.is_empty() {
                separator(b);
            }
            b.text(skein_ui::Run::Plain, &words.say(Word::Priority, &[Hole::Number(u64::from(priority))]));
        }
        b.close();
        b.close();
    }
}
pub(super) fn has_rows(page: &c::ReadValue) -> bool {
    batch(page).is_some()
}
fn authority_change(
    b: &mut Builder,
    before: &c::OwnerAuthorityDocument,
    after: &c::OwnerAuthorityDocument,
    words: &Words,
    rows: u32,
) {
    phrase(b, words, P::AuthorityBefore, &[]);
    authority(b, before, words, rows);
    phrase(b, words, P::AuthorityAfter, &[]);
    authority(b, after, words, rows);
}
fn authority(b: &mut Builder, document: &c::OwnerAuthorityDocument, words: &Words, rows: u32) {
    authority_facts(b, document, words, rows, false);
}
fn authority_facts(b: &mut Builder, document: &c::OwnerAuthorityDocument, words: &Words, rows: u32, inline: bool) {
    let value = &document.authority;
    fact(b, words, P::Spend, &[Hole::Number(value.spend)], inline);
    fact(b, words, P::Tools, &[Hole::Number(value.tools)], inline);
    fact(b, words, P::NoteBits, &[Hole::Number(u64::from(value.notes))], inline);
    fact(
        b,
        words,
        P::Delegation,
        &[Hole::Number(u64::from(value.delegation.tasks)), Hole::Number(u64::from(value.delegation.depth))],
        inline,
    );
    if let Some(deadline) = value.deadline {
        if inline {
            separator(b);
        }
        b.text(skein_ui::Run::Plain, &words.deadline(deadline));
    }
    for kind in window_facts(b, words, value.delegation.kinds.as_slice(), rows, inline) {
        let (word, number) = match kind {
            c::OwnerAuthorityExecutor::Charter(number) => (P::Charter, *number),
            c::OwnerAuthorityExecutor::Procedure(number) => (P::ProcedureCode, *number),
            c::OwnerAuthorityExecutor::Role(number) => (P::PersonRole, *number),
        };
        fact(b, words, word, &[Hole::Number(u64::from(number))], inline);
    }
    for grant in window_facts(b, words, value.grants.as_slice(), rows, inline) {
        if inline {
            separator(b);
        }
        crate::forge::drawn(b, &c::Part::Kind { connector: grant.connector, kind: grant.kind }, words);
        pattern_facts(b, &grant.pattern, words, rows, inline);
    }
    for scope in window_facts(b, words, value.note_resources.as_slice(), rows, inline) {
        pattern_facts(b, &scope.pattern, words, rows, inline);
    }
    if document.cut {
        if inline {
            fact(b, words, P::Cut, &[], true);
        } else {
            status(b, words, P::Cut, &[]);
        }
    }
}
fn pattern(b: &mut Builder, pattern: &c::OwnerPattern, words: &Words, rows: u32) {
    pattern_facts(b, pattern, words, rows, false);
}
fn pattern_facts(b: &mut Builder, pattern: &c::OwnerPattern, words: &Words, _rows: u32, inline: bool) {
    for segment in &pattern.segments {
        fact(b, words, P::PatternSegment, &[super::name(segment)], inline);
    }
    let (word, bytes) = match &pattern.last {
        c::OwnerPatternEnd::Exact(bytes) => (P::PatternExact, bytes),
        c::OwnerPatternEnd::Open(bytes) => (P::PatternOpen, bytes),
    };
    fact(b, words, word, &[super::name(bytes)], inline);
}

fn window<'a, T>(b: &mut Builder, words: &Words, values: &'a [T], rows: u32) -> &'a [T] {
    window_facts(b, words, values, rows, false)
}
fn window_facts<'a, T>(b: &mut Builder, words: &Words, values: &'a [T], rows: u32, inline: bool) -> &'a [T] {
    let count = values.len().min(usize::try_from(rows).expect("row bound fits"));
    if count < values.len() {
        if inline {
            fact(b, words, P::Cut, &[], true);
        } else {
            status(b, words, P::Cut, &[]);
        }
    }
    values.get(..count).expect("bounded prefix")
}

fn separator(b: &mut Builder) {
    b.text(skein_ui::Run::Plain, " · ".as_bytes());
}
fn fact(b: &mut Builder, words: &Words, word: P, holes: &[Hole], inline: bool) {
    if inline {
        separator(b);
        b.text(skein_ui::Run::Plain, &words.say_card(word, holes));
    } else {
        phrase(b, words, word, holes);
    }
}
fn specification_facts(b: &mut Builder, document: &c::SpecificationDocument, words: &Words) {
    for parameter in &document.specification.parameters {
        match parameter {
            c::ParameterDocument::Number { name, value } => {
                fact(b, words, P::ParameterNumber, &[Hole::Number(u64::from(*name)), Hole::Number(*value)], true);
            }
            c::ParameterDocument::Bytes { name, value } => {
                fact(b, words, P::ParameterBytes, &[Hole::Number(u64::from(*name)), super::name(value.bytes())], true);
            }
            c::ParameterDocument::Resource { name, connector, resource } => fact(
                b,
                words,
                P::ParameterResource,
                &[Hole::Number(u64::from(*name)), Hole::Number(*resource), Hole::Number(u64::from(*connector))],
                true,
            ),
        }
    }
    for task in &document.specification.inputs {
        fact(b, words, P::Input, &[Hole::Number(*task)], true);
    }
    if document.has_loss() {
        fact(b, words, P::Cut, &[], true);
    }
}

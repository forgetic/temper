//! One card function per first-slice object, wherever that object appears.

use skein_lib::Id;
use temper_web_domain::{Address, Body, Card, Domain, EndKind, Escalation, Intent, Object, TaskPhase, TaskResult};

use crate::binding::Binding;
use crate::builder::Builder;
use crate::limits::Limits;
use crate::markdown;
use crate::tree::{Class, Element, Level, NodeKey, Role};
use crate::words;

pub(crate) fn chip(builder: &mut Builder, object: &Object) {
    let Body::Chip(chip) = &object.body else {
        return;
    };
    builder.open(Element::Article);
    builder.key(NodeKey::Object(object.key));
    builder.class(Class::Card);
    builder.role(Role::Region);
    builder.name(&chip.title);
    builder.open(Element::Link);
    let address = Address::Task { number: chip.task, section: None };
    builder.href(address);
    builder.bind(Binding::Go(address));
    builder.name(&chip.title);
    builder.open(Element::Heading(Level::Three));
    builder.text(&chip.title);
    builder.close();
    builder.close();
    builder.open(Element::Paragraph);
    builder.text(words::phase(chip.phase));
    builder.close();
    builder.close();
}

pub(crate) fn escalation(builder: &mut Builder, domain: &Domain, id: Id<Object>, object: &Object, held: &Escalation) {
    builder.open(Element::Article);
    builder.key(NodeKey::Object(object.key));
    builder.class(Class::Card);
    builder.role(Role::Region);
    let name = words::numbered(b"Held task T", held.task);
    builder.name(&name);
    builder.open(Element::Heading(Level::Two));
    builder.text(words::hold(held.reason));
    builder.close();
    builder.open(Element::Paragraph);
    builder.text(words::release_does(held.reason));
    builder.close();
    match &object.card {
        Card::Open => actions(builder, id, held),
        Card::Deciding { .. } => status(builder, b"Being decided"),
        Card::Refused { refusal } => {
            status(builder, words::refusal(*refusal));
            actions(builder, id, held);
        }
        Card::Leaving { why, .. } => {
            let message = words::leaving(why, domain.offset());
            status(builder, &message);
        }
    }
    builder.close();
}

fn actions(builder: &mut Builder, id: Id<Object>, held: &Escalation) {
    builder.open(Element::Div);
    builder.class(Class::ButtonRow);
    if held.offers.release {
        button(builder, id, Intent::Release, b"Release task");
    }
    if held.offers.leave_held {
        button(builder, id, Intent::LeaveHeld, b"Leave held");
    }
    if held.offers.pass_up {
        button(builder, id, Intent::PassUp, b"Pass up");
    }
    builder.close();
}

fn button(builder: &mut Builder, id: Id<Object>, intent: Intent, label: &[u8]) {
    builder.open(Element::Button);
    builder.name(label);
    builder.bind(Binding::Intend { intent, object: id });
    builder.text(label);
    builder.close();
}

pub(crate) fn result(builder: &mut Builder, object: &Object, report: &TaskResult, limits: &Limits) {
    builder.open(Element::Article);
    builder.key(NodeKey::Object(object.key));
    builder.class(Class::Card);
    builder.role(Role::Region);
    let name = words::numbered(b"Result for T", report.task);
    builder.name(&name);
    builder.open(Element::Heading(Level::Two));
    builder.text(match report.kind {
        EndKind::Done => b"Report",
        EndKind::Failed => b"Failed",
        EndKind::Cancelled => b"Cancelled",
    });
    builder.close();
    markdown::read(&report.words, limits, builder);
    builder.close();
}

pub(crate) fn status(builder: &mut Builder, message: &[u8]) {
    builder.open(Element::Paragraph);
    builder.role(Role::Status);
    builder.text(message);
    builder.close();
}

pub(crate) fn phase_line(builder: &mut Builder, phase: TaskPhase) {
    builder.open(Element::Span);
    builder.class(Class::Tag);
    builder.role(Role::Status);
    builder.name(b"Task phase");
    builder.text(words::phase(phase));
    builder.close();
}

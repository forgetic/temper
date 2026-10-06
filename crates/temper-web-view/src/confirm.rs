//! The first-slice decision confirmation, read from domain state.

use temper_web_domain::{Domain, FieldRef, Intent, Problem};

use crate::binding::Binding;
use crate::builder::Builder;
use crate::tree::{Class, Element, Level, Role};

pub(crate) fn build(builder: &mut Builder, domain: &Domain) {
    let Some(confirming) = domain.confirming() else {
        return;
    };
    builder.open(Element::Dialog);
    builder.role(Role::Dialog);
    builder.name(b"Confirm decision");
    builder.open(Element::Heading(Level::Two));
    builder.text(match confirming.intent {
        Intent::Release => b"Release task?",
        Intent::LeaveHeld => b"Leave task held?",
        Intent::PassUp => b"Pass this up?",
    });
    builder.close();
    builder.open(Element::Paragraph);
    builder.text(match confirming.intent {
        Intent::Release => b"The task can try again. Its tries count from zero.",
        Intent::LeaveHeld => b"The task stays held, with your reason.",
        Intent::PassUp => b"The next holder will decide this task.",
    });
    builder.close();
    if confirming.intent == Intent::LeaveHeld {
        builder.open(Element::TextArea);
        builder.role(Role::TextBox);
        builder.name(b"Reason");
        builder.bind(Binding::Edit(FieldRef::Reason));
        if let Some(field) = domain.field(FieldRef::Reason) {
            builder.value(field);
        }
        builder.close();
    }
    if confirming.changed {
        problem(builder, b"The task changed while this was open.");
    }
    if let Some(problem_kind) = confirming.problem {
        match problem_kind {
            Problem::ReasonMissing => problem(builder, b"Write a reason first."),
            Problem::Changed => problem(builder, b"The task changed while this was open."),
            Problem::NotOffered => problem(builder, b"This decision is no longer offered."),
        }
    }
    builder.open(Element::Div);
    builder.class(Class::ButtonRow);
    builder.open(Element::Button);
    builder.name(b"Cancel decision");
    builder.bind(Binding::Dismiss);
    builder.text(b"Cancel");
    builder.close();
    builder.open(Element::Button);
    builder.class(Class::Primary);
    builder.name(b"Confirm decision");
    builder.bind(Binding::Confirm);
    builder.text(b"Confirm");
    builder.close();
    builder.close();
    builder.close();
}

fn problem(builder: &mut Builder, message: &[u8]) {
    builder.open(Element::Paragraph);
    builder.role(Role::Alert);
    builder.text(message);
    builder.close();
}

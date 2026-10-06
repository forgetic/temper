//! A chat's first task page: phase, opening words, held card and report.

use temper_web_domain::{Body, Domain, TaskPage};

use crate::builder::Builder;
use crate::cards;
use crate::limits::Limits;
use crate::tree::{Class, Element, Level, Role};
use crate::words;

pub(super) fn build(builder: &mut Builder, domain: &Domain, page: &TaskPage, limits: &Limits) {
    builder.class(Class::Narrow);
    builder.open(Element::Header);
    builder.open(Element::Heading(Level::One));
    let number = words::numbered(b"T", page.number);
    builder.text(&number);
    if let Some(id) = page.chip
        && let Some(object) = domain.object(id)
        && let Body::Chip(chip) = &object.body
    {
        builder.text(b" ");
        builder.text(&chip.title);
    }
    builder.close();
    if let Some(id) = page.chip
        && let Some(object) = domain.object(id)
        && let Body::Chip(chip) = &object.body
    {
        cards::phase_line(builder, chip.phase);
        builder.open(Element::Paragraph);
        let spend = words::numbered(b"Spent ", chip.spent);
        builder.text(&spend);
        let budget = words::numbered(b" of ", chip.budget);
        builder.text(&budget);
        builder.close();
    }
    builder.close();
    if let Some(id) = page.chip
        && let Some(object) = domain.object(id)
    {
        cards::chip(builder, object);
    }
    if let Some(words) = &page.first_words {
        builder.open(Element::Section);
        builder.role(Role::Log);
        builder.name(b"Conversation");
        builder.open(Element::Article);
        builder.role(Role::Region);
        builder.name(b"Your first words");
        builder.open(Element::Paragraph);
        builder.text(words);
        builder.close();
        builder.close();
        builder.close();
    }
    if let Some(id) = page.escalation
        && let Some(object) = domain.object(id)
        && let Body::Escalation(held) = &object.body
    {
        cards::escalation(builder, domain, id, object, held);
    }
    if let Some(id) = page.result
        && let Some(object) = domain.object(id)
        && let Body::Ended(report) = &object.body
    {
        cards::result(builder, object, report, limits);
    }
    if page.loading {
        cards::status(builder, b"Loading task");
    }
}

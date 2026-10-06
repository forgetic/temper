//! The navigation and link state on every page (web/ux/README.md, 4 and 7).

use temper_web_domain::{Address, Domain};

use crate::binding::Binding;
use crate::builder::Builder;
use crate::tree::{Class, Element, Role, State};
use crate::words;

pub fn build(builder: &mut Builder, domain: &Domain) {
    builder.open(Element::Header);
    builder.class(Class::Topbar);
    builder.open(Element::Link);
    builder.class(Class::Brand);
    builder.name(b"temper chats");
    builder.href(Address::Chats);
    builder.bind(Binding::Go(Address::Chats));
    builder.text(b"temper");
    builder.close();

    if let Some(project_number) = domain.frame().project {
        for project in &domain.frame().projects {
            if project.number == project_number {
                builder.open(Element::Span);
                builder.class(Class::Project);
                builder.text(&project.name);
                builder.close();
            }
        }
    }

    builder.open(Element::Nav);
    builder.class(Class::Navigation);
    builder.role(Role::Navigation);
    builder.name(b"Main navigation");
    link(builder, b"Inbox", Address::Inbox, domain);
    link(builder, b"Chats", Address::Chats, domain);
    builder.close();

    builder.open(Element::Span);
    builder.class(Class::LinkState);
    builder.role(Role::Status);
    builder.name(b"Connection status");
    builder.text(words::link(domain.link()));
    builder.close();

    if let Some(person) = &domain.frame().person {
        builder.open(Element::Span);
        builder.class(Class::User);
        builder.text(&person.name);
        builder.close();
    }
    builder.close();
}

fn link(builder: &mut Builder, name: &[u8], address: Address, domain: &Domain) {
    builder.open(Element::Link);
    if address == Address::Inbox {
        let counted = words::numbered(b"Inbox ", u64::from(domain.frame().inbox_count));
        builder.name(&counted);
    } else {
        builder.name(name);
    }
    builder.href(address);
    builder.bind(Binding::Go(address));
    if domain.address() == address {
        builder.state(State::Current);
    }
    builder.text(name);
    if address == Address::Inbox {
        builder.open(Element::Span);
        builder.class(Class::Count);
        let count = words::numbered(b"", u64::from(domain.frame().inbox_count));
        builder.text(&count);
        builder.close();
    }
    builder.close();
}

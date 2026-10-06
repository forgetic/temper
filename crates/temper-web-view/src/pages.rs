//! W2 pages: starting, sign-in, missing, and the person's chats.

use temper_web_domain::{Address, Chats, Domain, FieldRef, Form, Page};

use crate::binding::Binding;
use crate::builder::Builder;
use crate::confirm;
use crate::frame;
use crate::limits::Limits;
use crate::tree::{Class, Element, Level, NodeKey, Role, State};
use crate::words;

mod task;

pub fn build(builder: &mut Builder, domain: &Domain, limits: &Limits) {
    builder.open(Element::Div);
    frame::build(builder, domain);
    builder.open(Element::Main);
    builder.class(Class::Page);
    match domain.page() {
        Page::Starting => {
            builder.open(Element::Heading(Level::One));
            builder.text(b"Connecting to temper");
            builder.close();
        }
        Page::SignIn { .. } => signin(builder),
        Page::Missing { address } => missing(builder, *address),
        Page::Chats(chats) => chats_page(builder, domain, chats),
        Page::Task(page) => task::build(builder, domain, page, limits),
    }
    builder.close();
    confirm::build(builder, domain);
    notices(builder, domain);
    builder.close();
}

fn signin(builder: &mut Builder) {
    builder.open(Element::Heading(Level::One));
    builder.text(b"Sign in to temper");
    builder.close();
    builder.open(Element::Button);
    builder.name(b"Sign in");
    builder.bind(Binding::SignIn);
    builder.text(b"Sign in");
    builder.close();
}

fn missing(builder: &mut Builder, _address: Address) {
    builder.open(Element::Heading(Level::One));
    builder.text(b"Page unavailable");
    builder.close();
    builder.open(Element::Paragraph);
    builder.text(b"This page is not available yet.");
    builder.close();
}

fn chats_page(builder: &mut Builder, domain: &Domain, chats: &Chats) {
    builder.class(Class::Narrow);
    builder.open(Element::Section);
    builder.class(Class::Welcome);
    builder.open(Element::Heading(Level::One));
    builder.text(b"Start a chat");
    builder.close();
    builder.open(Element::Form);
    builder.class(Class::Composer);
    builder.bind(Binding::Submit(Form::NewChat));
    builder.open(Element::TextArea);
    builder.role(Role::TextBox);
    builder.name(b"Start a new chat");
    builder.state(State::SubmitOnEnter);
    builder.bind(Binding::Edit(FieldRef::NewChat));
    if let Some(field) = domain.field(FieldRef::NewChat) {
        builder.value(field);
    }
    builder.close();
    builder.open(Element::Button);
    builder.class(Class::Primary);
    builder.name(b"Start chat");
    builder.bind(Binding::Submit(Form::NewChat));
    builder.text(b"Start chat");
    builder.close();
    builder.close();
    builder.close();

    builder.open(Element::Section);
    builder.open(Element::Heading(Level::Two));
    builder.text(b"Your chats");
    builder.close();
    builder.open(Element::List);
    builder.class(Class::ChatList);
    builder.role(Role::List);
    builder.name(b"Your chats");
    for chat in &chats.rows {
        builder.open(Element::Item);
        builder.key(NodeKey::Chat(chat.task));
        builder.open(Element::Link);
        builder.class(Class::ChatPreview);
        let address = Address::Task { number: chat.task, section: None };
        builder.href(address);
        builder.bind(Binding::Go(address));
        let name = words::numbered(b"Open chat T", chat.task);
        builder.name(&name);
        builder.open(Element::Heading(Level::Three));
        builder.text(&chat.title);
        builder.close();
        builder.open(Element::Span);
        builder.class(if chat.live { Class::Live } else { Class::Closed });
        builder.text(if chat.live { b"Live" } else { b"Closed" });
        builder.close();
        builder.close();
        builder.close();
    }
    builder.close();
    if chats.loading {
        builder.open(Element::Paragraph);
        builder.role(Role::Status);
        builder.name(b"Chats loading");
        builder.text(b"Loading chats");
        builder.close();
    }
    builder.close();
}

fn notices(builder: &mut Builder, domain: &Domain) {
    for notice in domain.notices() {
        builder.open(Element::Aside);
        builder.class(Class::Notice);
        builder.role(Role::Status);
        builder.text(words::notice(notice.kind));
        builder.close();
    }
}

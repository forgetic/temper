//! Focused step tests of the builder and stable-id diff.

use alloc::boxed::Box;
use skein_lib::{Duration, Env, List, Queue, Time, Wall};
use temper_web_domain::{
    Action, Address, Backoff, ChatLine, Domain, Event, Field, FieldRef, Form, Limits as DomainLimits, Offset,
    ReadResult, Request, max_out, step,
};

use crate::binding::{DomEvent, decode};
use crate::builder::Builder;
use crate::diff::{Patch, diff};
use crate::limits::Limits;
use crate::render::{View, render};
use crate::tree::{Element, NodeKey, Tree};

fn chats(keys: &[u64]) -> Tree {
    let mut builder = Builder::new(32, 4);
    builder.open(Element::List);
    for key in keys {
        builder.open(Element::Item);
        builder.key(NodeKey::Chat(*key));
        builder.text(b"A chat");
        builder.close();
    }
    builder.close();
    builder.finish()
}

#[test]
fn builder_records_subtree_sizes() {
    let tree = chats(&[1, 2]);
    assert_eq!(tree.len(), 5);
    assert_eq!(tree.get(0).expect("root exists").size, 5);
    assert_eq!(tree.get(1).expect("first row exists").size, 2);
}

#[test]
fn keyed_rows_keep_ids_when_reordered() {
    let mut prior = Tree::new(32);
    let mut next = chats(&[1, 2]);
    let mut queue = Queue::with_capacity(100);
    let mut next_id = 0;
    diff(&prior, &mut next, 4, &mut next_id, &mut queue);
    assert_eq!(queue.len(), 1);
    let _insert = queue.pop().expect("first render inserts a tree");
    prior = next;
    let first_id = prior.get(1).expect("first row exists").id;
    let second_id = prior.get(3).expect("second row exists").id;
    let mut reordered = chats(&[2, 1]);
    diff(&prior, &mut reordered, 4, &mut next_id, &mut queue);
    assert_eq!(reordered.get(1).expect("second row exists").id, second_id);
    assert_eq!(reordered.get(3).expect("first row exists").id, first_id);
    let mut moved = false;
    for patch in &queue {
        if let Patch::Move { .. } = patch {
            moved = true;
        }
    }
    assert!(moved, "a reordered keyed row moves");
}

#[test]
fn value_patch_only_follows_domain_writes() {
    let mut old = Builder::new(4, 2);
    old.open(Element::TextArea);
    old.value(&Field { text: (&b"draft"[..]).into(), written: 1 });
    old.close();
    let mut old = old.finish();
    let mut output = Queue::with_capacity(20);
    let mut next_id = 0;
    diff(&Tree::new(4), &mut old, 2, &mut next_id, &mut output);
    while output.pop().is_some() {}

    let mut new = Builder::new(4, 2);
    new.open(Element::TextArea);
    new.value(&Field { text: (&b"typing"[..]).into(), written: 1 });
    new.close();
    let mut new = new.finish();
    diff(&old, &mut new, 2, &mut next_id, &mut output);
    assert_eq!(output.len(), 0, "typing does not trigger a value patch");
}

fn domain_limits() -> DomainLimits {
    DomainLimits {
        objects: 4,
        requests: 4,
        streams: 1,
        reads: 2,
        window: 4,
        turns: 4,
        tree: 4,
        drafts: 1,
        notices: 4,
        words: 64,
        text: 64,
        streaming: 64,
        projects: 2,
        backoff: Backoff { first: Duration::from_millis(10), most: Duration::from_secs(1) },
        heartbeat: Duration::from_secs(5),
        linger: Duration::from_secs(1),
        notice: Duration::from_secs(2),
        save: Duration::from_millis(50),
        facts: 4,
    }
}

#[test]
fn chats_render_at_window_limit_and_decode_controls() {
    let limits = domain_limits();
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut domain = Domain::new(&limits, 4);
    let mut requests = Queue::with_capacity(max_out(&limits));
    step(&mut domain, &env, Event::Start { address: Address::Chats, saved: None, offset: Offset(0) }, &mut requests);
    let mut read = None;
    while let Some(request) = requests.pop() {
        if let Request::Read { read: token, .. } = request {
            read = Some(token);
        }
    }
    let read = read.expect("start reads chats");
    let mut rows = List::with_capacity(limits.window);
    for number in 1..=limits.window {
        rows.push(ChatLine {
            task: u64::from(number),
            project: 1,
            title: Box::from(b"A chat".as_slice()),
            live: true,
            last_activity: Wall::EPOCH,
        })
        .expect("window holds its rows");
    }
    step(
        &mut domain,
        &env,
        Event::Read { read, result: ReadResult::Chats { rows: rows.into_boxed(), older: None } },
        &mut requests,
    );
    let view_limits = Limits::of(&limits).expect("domain limits imply view limits");
    let mut view = View::new(&view_limits);
    let mut patches = Queue::with_capacity(view_limits.patches);
    render(&mut view, &domain, &view_limits, &mut patches);
    assert!(view.tree().len() <= view_limits.nodes, "full chat window fits view limit");
    let mut button = None;
    let mut composer = None;
    for node in view.tree().nodes() {
        if node.name.as_deref() == Some(b"Start chat".as_slice()) {
            button = Some(node.id);
        }
        if node.name.as_deref() == Some(b"Start a new chat".as_slice()) {
            composer = Some(node.id);
        }
    }
    let action = decode(&view, DomEvent::Press { node: button.expect("start button is named") });
    assert_eq!(action, Some(Action::Submit { form: Form::NewChat }));
    let action = decode(
        &view,
        DomEvent::Input { node: composer.expect("composer is named"), text: Box::from(b"Hello".as_slice()) },
    );
    assert_eq!(action, Some(Action::Edit { field: FieldRef::NewChat, text: Box::from(b"Hello".as_slice()) }));
}

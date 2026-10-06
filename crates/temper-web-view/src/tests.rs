//! Focused step tests of the builder and stable-id diff.

use alloc::boxed::Box;
#[expect(clippy::disallowed_types, reason = "the independent patch model in tests resizes its rows")]
use alloc::vec::Vec;
use skein_lib::{Duration, Env, List, Queue, Slab, Time, Token, Wall};
use temper_web_domain::{
    Action, Address, Backoff, Body, Card, ChatLine, Chip, Choice, Domain, Escalation, Event, Field, FieldRef, Form,
    HoldReason, Intent, Limits as DomainLimits, Object, ObjectKey, Offers, Offset, Person, ReadResult, Request,
    Snapshot, StreamEvent, TaskPhase, TaskSnapshot, Watch, Why, max_out, step,
};

use crate::binding::{DomEvent, decode};
use crate::builder::Builder;
use crate::cards;
use crate::diff::{Patch, diff};
use crate::limits::Limits;
use crate::markdown;
use crate::render::{View, render};
use crate::tree::{Element, NodeId, NodeKey, Tree};
use crate::words;

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
        streams: 2,
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
fn rejected_paste_patches_browser_value_back_to_accepted_words() {
    let limits = domain_limits();
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut domain = Domain::new(&limits, 15);
    let mut requests = Queue::with_capacity(max_out(&limits));
    step(&mut domain, &env, Event::Start { address: Address::Chats, saved: None, offset: Offset(0) }, &mut requests);
    let view_limits = Limits::of(&limits).expect("valid view limits");
    let mut view = View::new(&view_limits);
    let mut patches = Queue::with_capacity(view_limits.patches);
    render(&mut view, &domain, &view_limits, &mut patches);
    while patches.pop().is_some() {}

    step(
        &mut domain,
        &env,
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from(b"hi".as_slice()) } },
        &mut requests,
    );
    render(&mut view, &domain, &view_limits, &mut patches);
    assert!(patches.is_empty(), "an accepted edit leaves the browser-owned value alone");

    step(
        &mut domain,
        &env,
        Event::Act { action: Action::Edit { field: FieldRef::NewChat, text: Box::from([b'x'; 65]) } },
        &mut requests,
    );
    render(&mut view, &domain, &view_limits, &mut patches);
    let mut restored = false;
    for patch in &patches {
        if let Patch::Value { text, .. } = patch {
            restored = text.as_ref() == b"hi";
        }
    }
    assert!(restored, "rejected paste restores the value before a submit");
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

#[test]
fn markdown_reads_supported_blocks_and_preserves_unknown_text() {
    let limits = Limits { nodes: 128, patches: 256, depth: 16, markdown_depth: 3 };
    let mut builder = Builder::new(limits.nodes, limits.depth);
    builder.open(Element::Section);
    markdown::read(b"# Report\n\n- one\n  - nested\n- two\n\n**strong** and *em* `code` [link](https://example.org)\n\n<table>raw</table>\n\nunclosed *mark", &limits, &mut builder);
    builder.close();
    let tree = builder.finish();
    let mut heading = false;
    let mut nested = false;
    let mut strong = false;
    let mut link = false;
    let mut raw = false;
    let mut unclosed = false;
    for node in tree.nodes() {
        if node.element == Element::Heading(crate::Level::Three) {
            heading = true;
        }
        if node.element == Element::List {
            nested = true;
        }
        if node.element == Element::Strong {
            strong = true;
        }
        if node.external.as_deref() == Some(b"https://example.org".as_slice()) {
            link = true;
        }
        if node.text.as_deref() == Some(b"<table>raw</table>".as_slice()) {
            raw = true;
        }
        if node.text.as_deref() == Some(b"unclosed *mark".as_slice()) {
            unclosed = true;
        }
    }
    assert!(heading && nested && strong && link && raw && unclosed, "markdown subset and fallback are visible");
}

#[test]
fn markdown_does_not_turn_unclosed_fence_into_code() {
    let limits = Limits { nodes: 16, patches: 32, depth: 4, markdown_depth: 1 };
    let mut builder = Builder::new(limits.nodes, limits.depth);
    builder.open(Element::Section);
    markdown::read(b"```rust\nlet x = 1;", &limits, &mut builder);
    builder.close();
    let tree = builder.finish();
    let mut literal = false;
    for node in tree.nodes() {
        if node.text.as_deref() == Some(b"```rust".as_slice()) {
            literal = true;
        }
    }
    assert!(literal, "unclosed fence stays visible as text");
}

#[test]
fn markdown_image_stays_literal_text() {
    let limits = Limits { nodes: 16, patches: 32, depth: 4, markdown_depth: 1 };
    let mut builder = Builder::new(limits.nodes, limits.depth);
    builder.open(Element::Section);
    markdown::read(b"![diagram](https://example.org/a.png)", &limits, &mut builder);
    builder.close();
    let tree = builder.finish();
    let mut literal = false;
    for node in tree.nodes() {
        if node.text.as_deref() == Some(b"![diagram](https://example.org/a.png)".as_slice()) {
            literal = true;
        }
        assert_ne!(node.element, Element::Link, "an image is not a link");
    }
    assert!(literal, "an image remains literal text");
}

#[test]
fn clock_uses_persons_offset_across_midnight() {
    let before = Wall::from_nanos(86_340_000_000_000);
    assert_eq!(words::clock(before, Offset(0)).as_ref(), b"23:59");
    assert_eq!(words::clock(before, Offset(3_600)).as_ref(), b"00:59");
}

fn locate(rows: &[NodeId], wanted: NodeId) -> Option<usize> {
    for (index, row) in rows.iter().enumerate() {
        if *row == wanted {
            return Some(index);
        }
    }
    None
}

#[expect(clippy::disallowed_types, reason = "an independent patch model in a test may resize its rows")]
fn assert_row_patches(prior: &Tree, next: &Tree, patches: &Queue<Patch>) {
    let mut model = Vec::new();
    let mut expected = Vec::new();
    for node in prior.nodes() {
        if node.key.is_some() {
            model.push(node.id);
        }
    }
    for node in next.nodes() {
        if node.key.is_some() {
            expected.push(node.id);
        }
    }
    for patch in patches {
        match patch {
            Patch::Insert { before, nodes, .. } => {
                for made in nodes {
                    if made.node.key.is_some() {
                        let at = match before {
                            Some(anchor) => locate(&model, *anchor).expect("anchor exists"),
                            None => model.len(),
                        };
                        model.insert(at, made.node.id);
                    }
                }
            }
            Patch::Remove { node } => {
                if let Some(at) = locate(&model, *node) {
                    model.remove(at);
                }
            }
            Patch::Move { node, before, .. } => {
                let from = locate(&model, *node).expect("moved row exists");
                let row = model.remove(from);
                let to = match before {
                    Some(anchor) => locate(&model, *anchor).expect("anchor exists"),
                    None => model.len(),
                };
                model.insert(to, row);
            }
            Patch::Text { .. } | Patch::Set { .. } | Patch::Value { .. } => {}
        }
    }
    assert_eq!(model, expected, "row patches reproduce the next keyed order");
}

#[test]
fn keyed_row_patches_reproduce_permutations_and_insertions() {
    let mut prior = chats(&[1, 2, 3]);
    let mut next_id = 0;
    let mut initial = Queue::with_capacity(100);
    diff(&Tree::new(32), &mut prior, 4, &mut next_id, &mut initial);
    let mut unchanged = chats(&[1, 2, 3]);
    let mut no_patches = Queue::with_capacity(100);
    diff(&prior, &mut unchanged, 4, &mut next_id, &mut no_patches);
    assert!(no_patches.is_empty(), "an unchanged tree emits no patch");
    for order in [[3, 2, 1], [2, 3, 1], [1, 3, 2], [2, 1, 3]] {
        let mut next = chats(&order);
        let mut patches = Queue::with_capacity(100);
        diff(&prior, &mut next, 4, &mut next_id, &mut patches);
        assert_row_patches(&prior, &next, &patches);
    }
    let mut next = chats(&[3, 4, 2, 1]);
    let mut patches = Queue::with_capacity(100);
    diff(&prior, &mut next, 4, &mut next_id, &mut patches);
    assert_row_patches(&prior, &next, &patches);
}

fn frame_children(with_project: bool) -> Tree {
    let mut builder = Builder::new(8, 2);
    builder.open(Element::Header);
    builder.open(Element::Link);
    builder.close();
    if with_project {
        builder.open(Element::Span);
        builder.close();
    }
    builder.open(Element::Nav);
    builder.close();
    builder.open(Element::Span);
    builder.close();
    builder.close();
    builder.finish()
}

#[test]
#[expect(clippy::disallowed_types, reason = "the independent patch model in a test resizes its children")]
fn unkeyed_insertion_keeps_sibling_order() {
    let mut old = frame_children(false);
    let mut next_id = 0;
    let mut initial = Queue::with_capacity(32);
    diff(&Tree::new(8), &mut old, 2, &mut next_id, &mut initial);
    let mut new = frame_children(true);
    let mut patches = Queue::with_capacity(32);
    diff(&old, &mut new, 2, &mut next_id, &mut patches);
    let mut model = Vec::new();
    for node in old.nodes().iter().skip(1) {
        model.push(node.id);
    }
    for patch in &patches {
        match patch {
            Patch::Insert { before, nodes, .. } => {
                let inserted = nodes.first().expect("insert has a node").node.id;
                let at = match before {
                    Some(anchor) => locate(&model, *anchor).expect("anchor exists"),
                    None => model.len(),
                };
                model.insert(at, inserted);
            }
            Patch::Move { node, before, .. } => {
                let from = locate(&model, *node).expect("moved node exists");
                let moved = model.remove(from);
                let at = match before {
                    Some(anchor) => locate(&model, *anchor).expect("anchor exists"),
                    None => model.len(),
                };
                model.insert(at, moved);
            }
            Patch::Remove { node } => {
                let at = locate(&model, *node).expect("removed node exists");
                let _removed = model.remove(at);
            }
            Patch::Set { .. } | Patch::Text { .. } | Patch::Value { .. } => {}
        }
    }
    let mut expected = Vec::new();
    for node in new.nodes().iter().skip(1) {
        expected.push(node.id);
    }
    assert_eq!(model, expected, "unkeyed insert and moves reproduce the frame");
}

#[test]
fn held_task_renders_card_and_confirmation_bindings() {
    let limits = domain_limits();
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut domain = Domain::new(&limits, 9);
    let mut requests = Queue::with_capacity(max_out(&limits));
    step(
        &mut domain,
        &env,
        Event::Start { address: Address::Task { number: 27, section: None }, saved: None, offset: Offset(0) },
        &mut requests,
    );
    let mut task_stream = None;
    while let Some(request) = requests.pop() {
        if let Request::Open { stream, watch: Watch::Task { number: 27 } } = request {
            task_stream = Some(stream);
        }
    }
    let stream = task_stream.expect("task page opens its watch");
    step(&mut domain, &env, Event::Opened { stream }, &mut requests);
    step(
        &mut domain,
        &env,
        Event::Streamed {
            stream,
            event: StreamEvent::Snapshot(Snapshot::Task(TaskSnapshot {
                chip: Chip {
                    task: 27,
                    project: 1,
                    title: Box::from(b"Review login".as_slice()),
                    phase: TaskPhase::Held(HoldReason::Tries),
                    spent: 3,
                    budget: 8,
                    revision: 2,
                },
                first_words: Box::from(b"Review login".as_slice()),
                escalation: Some(Escalation {
                    task: 27,
                    reason: HoldReason::Tries,
                    waiting_since: Wall::EPOCH,
                    offers: Offers { release: true, leave_held: true, pass_up: false },
                    revision: 2,
                }),
                result: None,
            })),
        },
        &mut requests,
    );
    let view_limits = Limits::of(&limits).expect("view limits fit");
    let mut view = View::new(&view_limits);
    let mut patches = Queue::with_capacity(view_limits.patches);
    render(&mut view, &domain, &view_limits, &mut patches);
    let mut release = None;
    for node in view.tree().nodes() {
        if node.name.as_deref() == Some(b"Release task".as_slice()) {
            release = Some(node.id);
        }
    }
    let action =
        decode(&view, DomEvent::Press { node: release.expect("held card offers release") }).expect("button decodes");
    let Action::Intend { intent: Intent::Release, .. } = action else { panic!("release button intends release") };
    step(&mut domain, &env, Event::Act { action }, &mut requests);
    render(&mut view, &domain, &view_limits, &mut patches);
    let mut dialog = false;
    let mut confirm = None;
    for node in view.tree().nodes() {
        if node.role == Some(crate::Role::Dialog) {
            dialog = true;
        }
        if node.name.as_deref() == Some(b"Confirm decision".as_slice()) && node.element == Element::Button {
            confirm = Some(node.id);
        }
    }
    assert!(dialog, "decision is confirmed in a named dialog");
    assert_eq!(
        decode(&view, DomEvent::Press { node: confirm.expect("confirm button is named") }),
        Some(Action::Confirm)
    );
}

fn has_name(tree: &Tree, name: &[u8]) -> bool {
    for node in tree.nodes() {
        if node.name.as_deref() == Some(name) {
            return true;
        }
    }
    false
}

fn has_text(tree: &Tree, text: &[u8]) -> bool {
    for node in tree.nodes() {
        if node.text.as_deref() == Some(text) {
            return true;
        }
    }
    false
}

#[test]
fn escalation_card_shows_pending_then_decided_elsewhere() {
    let domain = Domain::new(&domain_limits(), 7);
    let held = Escalation {
        task: 27,
        reason: HoldReason::Tries,
        waiting_since: Wall::EPOCH,
        offers: Offers { release: true, leave_held: true, pass_up: false },
        revision: 1,
    };
    let mut objects = Slab::with_capacity(1);
    let id = objects
        .insert(Object {
            key: ObjectKey::Escalation { task: 27 },
            revision: 1,
            body: Body::Escalation(held.clone()),
            card: Card::Open,
            sources: 1,
        })
        .expect("one object fits");
    let mut builder = Builder::new(40, 8);
    builder.open(Element::Section);
    cards::escalation(&mut builder, &domain, id, objects.get(id).expect("object exists"), &held);
    builder.close();
    assert!(has_name(&builder.finish(), b"Release task"), "open card offers release");

    objects.get_mut(id).expect("object exists").card = Card::Deciding { request: Token::new(3) };
    let mut builder = Builder::new(40, 8);
    builder.open(Element::Section);
    cards::escalation(&mut builder, &domain, id, objects.get(id).expect("object exists"), &held);
    builder.close();
    let pending = builder.finish();
    assert!(has_text(&pending, b"Being decided"), "pending card says it is undecided");
    assert!(!has_name(&pending, b"Release task"), "pending card has no second release button");

    objects.get_mut(id).expect("object exists").card = Card::Leaving {
        why: Why::Decided {
            by: Person { number: 2, name: Box::from(b"Ada".as_slice()) },
            choice: Choice::Released,
            at: Wall::EPOCH,
        },
        until: Time::from_nanos(1),
    };
    let mut builder = Builder::new(40, 8);
    builder.open(Element::Section);
    cards::escalation(&mut builder, &domain, id, objects.get(id).expect("object exists"), &held);
    builder.close();
    let leaving = builder.finish();
    assert!(has_text(&leaving, b"Ada released at 00:00"), "other person's decision is attributed");
}

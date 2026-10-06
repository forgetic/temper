//! Typed bindings stay in the view; the shell returns only node ids.

use alloc::boxed::Box;
use temper_web_domain::{Action, Address, FieldRef, Form};

use crate::render::View;
use crate::tree::NodeId;

/// The action template attached to a node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Binding {
    Go(Address),
    Edit(FieldRef),
    Submit(Form),
    SignIn,
}

/// A browser event expressed without DOM types.
#[derive(PartialEq, Eq, Debug)]
pub enum DomEvent {
    Press { node: NodeId },
    Input { node: NodeId, text: Box<[u8]> },
    Submit { node: NodeId },
}

/// Turn an event on a current node into a domain action; removed nodes are stale.
#[must_use]
pub fn decode(view: &View, event: DomEvent) -> Option<Action> {
    match event {
        DomEvent::Press { node } => match view.tree().find(node)?.binding? {
            Binding::Go(address) => Some(Action::Go { address }),
            Binding::Submit(form) => Some(Action::Submit { form }),
            Binding::SignIn => Some(Action::SignIn),
            Binding::Edit(_) => None,
        },
        DomEvent::Input { node, text } => match view.tree().find(node)?.binding? {
            Binding::Edit(field) => Some(Action::Edit { field, text }),
            Binding::Go(_) | Binding::Submit(_) | Binding::SignIn => None,
        },
        DomEvent::Submit { node } => match view.tree().find(node)?.binding? {
            Binding::Submit(form) => Some(Action::Submit { form }),
            Binding::Go(_) | Binding::Edit(_) | Binding::SignIn => None,
        },
    }
}

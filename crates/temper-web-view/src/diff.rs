//! Stable node ids and bounded tree patches (02-view.md, section 6).

use alloc::boxed::Box;
use skein_lib::{List, Queue, Stack};
use temper_web_domain::Address;

use crate::tree::{Classes, Node, NodeId, Role, States, Tree};

/// All attributes the shell needs on a node. Bindings remain in the view.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Attribute {
    pub role: Option<Role>,
    pub name: Option<Box<[u8]>>,
    pub classes: Classes,
    pub states: States,
    pub href: Option<Address>,
    pub external: Option<Box<[u8]>>,
    pub bound: bool,
}

impl Attribute {
    fn of(node: &Node) -> Attribute {
        Attribute {
            role: node.role,
            name: node.name.clone(),
            classes: node.classes,
            states: node.states,
            href: node.href,
            external: node.external.clone(),
            bound: node.binding.is_some(),
        }
    }
}

/// An inserted node in preorder; `node.binding` is always absent.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Made {
    pub node: Node,
}

/// An edit to the browser's tree.
#[derive(PartialEq, Eq, Debug)]
pub enum Patch {
    Insert { parent: NodeId, before: Option<NodeId>, nodes: Box<[Made]> },
    Remove { node: NodeId },
    Move { node: NodeId, parent: NodeId, before: Option<NodeId> },
    Text { node: NodeId, text: Box<[u8]> },
    Set { node: NodeId, attribute: Attribute },
    Value { node: NodeId, text: Box<[u8]> },
}

fn parents(tree: &Tree, depth: u32) -> List<Option<u32>> {
    let mut result = List::with_capacity(tree.len());
    let mut open = Stack::with_capacity(depth);
    for index in 0..tree.len() {
        while let Some((_, end)) = open.top() {
            if index < *end {
                break;
            }
            let _closed = open.pop().expect("an open node remains");
        }
        let mut parent = None;
        if let Some((value, _)) = open.top() {
            parent = Some(*value);
        }
        result.push(parent).expect("one parent per node");
        let node = tree.get(index).expect("index is in tree");
        let end = index.checked_add(node.size).expect("tree extent fits u32");
        open.push((index, end)).expect("tree fits depth limit");
    }
    result
}

fn old_match(old: &Tree, old_parent: u32, node: &Node, used: &List<Option<u32>>) -> Option<u32> {
    let parent = old.get(old_parent)?;
    let end = old_parent.checked_add(parent.size)?;
    let mut child = old_parent.checked_add(1)?;
    while child < end {
        let candidate = old.get(child)?;
        let free = used.get(child)?.is_none();
        let same = if let Some(key) = node.key {
            candidate.key == Some(key) && candidate.element == node.element
        } else {
            candidate.key.is_none() && candidate.element == node.element
        };
        if free && same {
            return Some(child);
        }
        child = child.checked_add(candidate.size)?;
    }
    None
}

fn next_sibling(tree: &Tree, parents: &List<Option<u32>>, index: u32) -> Option<u32> {
    let node = tree.get(index)?;
    let next = index.checked_add(node.size)?;
    if parents.get(next)? == parents.get(index)? { Some(next) } else { None }
}

fn first_later_id(tree: &Tree, parents: &List<Option<u32>>, index: u32) -> Option<NodeId> {
    let next = next_sibling(tree, parents, index)?;
    Some(tree.get(next)?.id)
}

fn subtree(tree: &Tree, start: u32) -> Box<[Made]> {
    let size = tree.get(start).expect("subtree root exists").size;
    let mut made = List::with_capacity(size);
    let end = start.checked_add(size).expect("subtree extent fits u32");
    for index in start..end {
        let mut node = tree.get(index).expect("subtree node exists").clone();
        node.binding = None;
        made.push(Made { node }).expect("subtree fits measured size");
    }
    made.into_boxed()
}

/// Assign ids to `new`, then emit edits from `old`. The caller reserves
/// `limits.patches` slots. Keyed siblings match by key; other siblings by
/// element and position.
pub fn diff(old: &Tree, new: &mut Tree, depth: u32, next_id: &mut u32, out: &mut Queue<Patch>) {
    let old_parents = parents(old, depth);
    let new_parents = parents(new, depth);
    let mut old_to_new = List::with_capacity(old.len());
    for _ in 0..old.len() {
        old_to_new.push(None).expect("match table fits old tree");
    }
    let mut new_to_old = List::with_capacity(new.len());
    for _ in 0..new.len() {
        new_to_old.push(None).expect("match table fits new tree");
    }

    for index in 0..new.len() {
        let matched = if index == 0 {
            match old.get(0) {
                Some(root) if root.element == new.get(0).expect("new root exists").element => Some(0),
                Some(_) | None => None,
            }
        } else {
            let parent = new_parents.get(index).copied().flatten().expect("nonroot has parent");
            let old_parent = new_to_old.get(parent).copied().flatten();
            match old_parent {
                Some(parent) => old_match(old, parent, new.get(index).expect("new node exists"), &old_to_new),
                None => None,
            }
        };
        if let Some(old_index) = matched {
            *old_to_new.get_mut(old_index).expect("old match slot exists") = Some(index);
            *new_to_old.get_mut(index).expect("new match slot exists") = Some(old_index);
            new.nodes.get_mut(index).expect("new node exists").id =
                old.get(old_index).expect("matched old node exists").id;
        } else {
            *next_id = next_id.checked_add(1).expect("node ids do not wrap");
            new.nodes.get_mut(index).expect("new node exists").id = NodeId(*next_id);
        }
    }

    // Remove only roots of unmatched old subtrees.
    for index in 0..old.len() {
        if old_to_new.get(index).copied().flatten().is_some() {
            continue;
        }
        let parent = old_parents.get(index).copied().flatten();
        let remove_root = match parent {
            Some(parent) => old_to_new.get(parent).copied().flatten().is_some(),
            None => true,
        };
        if remove_root {
            let node = old.get(index).expect("old node exists");
            out.push(Patch::Remove { node: node.id });
        }
    }

    // Insert unmatched roots in reverse order, using the next sibling as
    // the anchor. This preserves order even for adjacent new subtrees.
    for index in (0..new.len()).rev() {
        if new_to_old.get(index).copied().flatten().is_some() {
            continue;
        }
        let parent = new_parents.get(index).copied().flatten();
        if let Some(parent) = parent {
            if new_to_old.get(parent).copied().flatten().is_none() {
                continue;
            }
            let parent_id = new.get(parent).expect("parent exists").id;
            let before = first_later_id(new, &new_parents, index);
            out.push(Patch::Insert { parent: parent_id, before, nodes: subtree(new, index) });
        } else {
            out.push(Patch::Insert { parent: NodeId(0), before: None, nodes: subtree(new, index) });
        }
    }

    for index in 0..new.len() {
        let Some(old_index) = new_to_old.get(index).copied().flatten() else {
            continue;
        };
        let new_node = new.get(index).expect("new node exists");
        let old_node = old.get(old_index).expect("old node exists");
        if Attribute::of(new_node) != Attribute::of(old_node) {
            out.push(Patch::Set { node: new_node.id, attribute: Attribute::of(new_node) });
        }
        if new_node.text != old_node.text
            && let Some(text) = &new_node.text
        {
            out.push(Patch::Text { node: new_node.id, text: text.clone() });
        }
        if let Some(value) = &new_node.value {
            let changed = match &old_node.value {
                Some(old_value) => old_value.written != value.written,
                None => true,
            };
            if changed {
                out.push(Patch::Value { node: new_node.id, text: value.text.clone() });
            }
        }
    }

    move_reordered(new, &new_parents, &new_to_old, out);
}

// Reverse moves use the next sibling as an already positioned anchor. A
// parent with unchanged relative order needs no moves, including when nodes
// were merely inserted or removed. Unkeyed siblings can also change order
// when a new sibling of the same element takes an earlier positional match.
fn move_reordered(new: &Tree, new_parents: &List<Option<u32>>, new_to_old: &List<Option<u32>>, out: &mut Queue<Patch>) {
    let mut children = List::with_capacity(new.len());
    for parent in 0..new.len() {
        if new_to_old.get(parent).copied().flatten().is_none() {
            continue;
        }
        children.clear();
        let mut child = parent.checked_add(1).expect("tree index fits u32");
        let end = parent.checked_add(new.get(parent).expect("parent exists").size).expect("tree extent fits u32");
        let mut last_old = None;
        let mut reordered = false;
        while child < end {
            let node = new.get(child).expect("child exists");
            if let Some(old_index) = new_to_old.get(child).copied().flatten() {
                if let Some(last) = last_old
                    && old_index < last
                {
                    reordered = true;
                }
                last_old = Some(old_index);
                children.push(child).expect("matched children fit tree bound");
            }
            child = child.checked_add(node.size).expect("child extent fits u32");
        }
        if !reordered {
            continue;
        }
        let parent_id = new.get(parent).expect("parent exists").id;
        for index in (0..children.len()).rev() {
            let child = *children.get(index).expect("matched child exists");
            let node = new.get(child).expect("matched child exists");
            out.push(Patch::Move { node: node.id, parent: parent_id, before: first_later_id(new, new_parents, child) });
        }
    }
}

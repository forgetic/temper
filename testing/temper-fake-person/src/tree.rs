//! Immediate face over the view's preorder tree.
use crate::{Doing, Face, Find, Found, Target};
use temper_web_view::{DomEvent, Element, Node, Role, Tree};

#[derive(Debug)]
pub struct TreeFace<'a> {
    tree: &'a Tree,
}

impl<'a> TreeFace<'a> {
    #[must_use]
    pub fn new(tree: &'a Tree) -> TreeFace<'a> {
        TreeFace { tree }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    for piece in hay.windows(needle.len()) {
        if piece.eq_ignore_ascii_case(needle) {
            return true;
        }
    }
    false
}

fn matches(node: &Node, target: &Target) -> bool {
    match target {
        Target::Role { role, name } => {
            if semantic_role(node) != Some(*role) {
                return false;
            }
            match &node.name {
                Some(value) => contains(value, name),
                None => false,
            }
        }
        Target::Text { text } => match &node.text {
            Some(value) => contains(value, text),
            None => false,
        },
    }
}

fn semantic_role(node: &Node) -> Option<Role> {
    node.role.or(match node.element {
        Element::Button => Some(Role::Button),
        Element::Link => Some(Role::Link),
        Element::Nav => Some(Role::Navigation),
        Element::List => Some(Role::List),
        Element::Item => Some(Role::ListItem),
        Element::TextArea | Element::Input(_) => Some(Role::TextBox),
        Element::Dialog => Some(Role::Dialog),
        Element::Main
        | Element::Header
        | Element::Section
        | Element::Article
        | Element::Aside
        | Element::Footer
        | Element::Div
        | Element::Span
        | Element::Heading(_)
        | Element::Paragraph
        | Element::OrderedList
        | Element::Form
        | Element::Label
        | Element::Radio
        | Element::Details
        | Element::Summary
        | Element::Strong
        | Element::Emphasis
        | Element::Code
        | Element::Pre
        | Element::Quote
        | Element::Time
        | Element::Progress
        | Element::Text => None,
    })
}

impl Face for TreeFace<'_> {
    fn find(&mut self, find: &Find) -> Found {
        let nodes = self.tree.nodes();
        let mut start = 0_usize;
        let mut end = nodes.len();
        for region in &find.within {
            let mut selected = None;
            for (index, node) in nodes.iter().enumerate().take(end).skip(start) {
                if matches(node, region) {
                    selected = Some((index, node.size));
                    break;
                }
            }
            let Some((index, size)) = selected else {
                return Found::Absent;
            };
            start = index.saturating_add(1);
            end = index.saturating_add(usize::try_from(size).expect("tree size fits"));
        }
        for node in nodes.get(start..end).expect("region is in the tree") {
            if matches(node, &find.target) {
                return Found::Node(node.id);
            }
        }
        Found::Absent
    }
}

#[must_use]
pub fn dom_event(doing: Doing) -> Option<DomEvent> {
    match doing {
        Doing::Press { node } => Some(DomEvent::Press { node }),
        Doing::Type { node, words } => Some(DomEvent::Input { node, text: words }),
        Doing::Send { node } => Some(DomEvent::Submit { node }),
        Doing::Go { .. } | Doing::Reload => None,
    }
}

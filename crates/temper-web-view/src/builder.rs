//! One node per call, with a bounded stack of open parents (02-view.md, 4).

use alloc::boxed::Box;
use skein_lib::Stack;
use temper_web_domain::{Address, Field};

use crate::binding::Binding;
use crate::tree::{Class, Element, Node, NodeKey, Role, State, Tree, Value};

#[derive(Debug)]
pub struct Builder {
    tree: Tree,
    open: Stack<u32>,
    current: Option<u32>,
}

impl Builder {
    #[must_use]
    pub fn new(nodes: u32, depth: u32) -> Builder {
        Builder { tree: Tree::new(nodes), open: Stack::with_capacity(depth), current: None }
    }

    pub(crate) fn reuse(mut tree: Tree, depth: u32) -> Builder {
        tree.nodes.clear();
        Builder { tree, open: Stack::with_capacity(depth), current: None }
    }

    pub fn open(&mut self, element: Element) {
        let index = self.tree.len();
        self.tree.nodes.push(Node::new(element)).expect("page fits its node limit");
        self.open.push(index).expect("page fits its depth limit");
        self.current = Some(index);
    }

    pub fn close(&mut self) {
        let index = self.open.pop().expect("a node is open");
        let size = self.tree.len().checked_sub(index).expect("open node precedes its descendants");
        self.tree.nodes.get_mut(index).expect("open node exists").size = size;
        self.current = self.open.top().copied();
    }

    pub fn text(&mut self, text: &[u8]) {
        self.open(Element::Text);
        self.node().text = Some(Box::from(text));
        self.close();
    }

    fn node(&mut self) -> &mut Node {
        let index = self.current.expect("an attribute has an open node");
        self.tree.nodes.get_mut(index).expect("open node exists")
    }

    pub fn key(&mut self, key: NodeKey) {
        self.node().key = Some(key);
    }
    pub fn role(&mut self, role: Role) {
        self.node().role = Some(role);
    }
    pub fn name(&mut self, name: &[u8]) {
        self.node().name = Some(Box::from(name));
    }
    pub fn class(&mut self, class: Class) {
        self.node().classes.add(class);
    }
    pub fn state(&mut self, state: State) {
        self.node().states.add(state);
    }
    pub fn value(&mut self, field: &Field) {
        self.node().value = Some(Value { text: field.text.clone(), written: field.written });
    }
    pub fn href(&mut self, address: Address) {
        self.node().href = Some(address);
    }
    pub fn bind(&mut self, binding: Binding) {
        self.node().binding = Some(binding);
    }

    #[must_use]
    pub fn finish(self) -> Tree {
        assert!(self.open.is_empty(), "all view nodes are closed");
        self.tree
    }
}

//! A separate browser tree, changed only by patches from the view.
use std::collections::BTreeSet;
use temper_web_view::{Node, NodeId, Patch, Tree};

#[derive(Debug, Default)]
pub struct Model {
    nodes: Vec<Node>,
    dirty_inputs: BTreeSet<NodeId>,
}

impl Model {
    #[must_use]
    pub fn new() -> Model {
        Model::default()
    }

    fn index(&self, id: NodeId) -> usize {
        self.nodes.iter().position(|node| node.id == id).expect("patch names a model node")
    }

    fn ancestors(&self, index: usize) -> Vec<NodeId> {
        self.nodes
            .iter()
            .enumerate()
            .take(index + 1)
            .filter_map(|(i, node)| {
                let end = i.checked_add(usize::try_from(node.size).expect("size fits"))?;
                (i < index && end > index).then_some(node.id)
            })
            .collect()
    }

    fn remove(&mut self, id: NodeId) -> Vec<Node> {
        let start = self.index(id);
        let size = usize::try_from(self.nodes[start].size).expect("size fits");
        let ancestors = self.ancestors(start);
        let removed: Vec<Node> = self.nodes.drain(start..start + size).collect();
        for node in &removed {
            self.dirty_inputs.remove(&node.id);
        }
        for id in ancestors {
            let index = self.index(id);
            self.nodes[index].size -= u32::try_from(size).expect("size fits");
        }
        removed
    }

    fn insert(&mut self, parent: NodeId, before: Option<NodeId>, nodes: Vec<Node>) {
        let size = u32::try_from(nodes.len()).expect("insert fits");
        if parent == NodeId(0) {
            assert!(self.nodes.is_empty(), "a root insert replaces the empty model");
            assert!(before.is_none(), "root has no sibling");
            self.nodes = nodes;
            return;
        }
        let p = self.index(parent);
        let end = p + usize::try_from(self.nodes[p].size).expect("size fits");
        let at = before.map_or(end, |id| self.index(id));
        assert!(at > p && at <= end, "insertion is under its parent");
        let mut ancestors = self.ancestors(p);
        ancestors.push(parent);
        self.nodes.splice(at..at, nodes);
        for id in ancestors {
            let index = self.index(id);
            self.nodes[index].size += size;
        }
    }

    pub fn apply(&mut self, patch: Patch) {
        match patch {
            Patch::Insert { parent, before, nodes } => {
                self.insert(parent, before, nodes.into_vec().into_iter().map(|made| made.node).collect());
            }
            Patch::Remove { node } => {
                let _ = self.remove(node);
            }
            Patch::Move { node, parent, before } => {
                let nodes = self.remove(node);
                self.insert(parent, before, nodes);
            }
            Patch::Text { node, text } => {
                let index = self.index(node);
                self.nodes[index].text = Some(text);
            }
            Patch::Set { node, attribute } => {
                let index = self.index(node);
                let target = &mut self.nodes[index];
                target.role = attribute.role;
                target.name = attribute.name;
                target.classes = attribute.classes;
                target.states = attribute.states;
                target.href = attribute.href;
                target.external = attribute.external;
                // The browser only keeps whether an event was bound. The
                // view alone holds the binding and decodes its node id.
            }
            Patch::Value { node, text } => {
                let index = self.index(node);
                let value = self.nodes[index].value.as_mut().expect("value patch names input");
                value.text = text;
                self.dirty_inputs.remove(&node);
            }
        }
    }

    pub fn input(&mut self, node: NodeId, text: &[u8]) {
        let index = self.index(node);
        if let Some(value) = &mut self.nodes[index].value {
            value.text = Box::from(text);
            self.dirty_inputs.insert(node);
        }
    }

    pub fn check(&self, tree: &Tree) {
        assert_eq!(self.nodes.len(), tree.nodes().len(), "patch tree node count");
        for (index, (model, actual)) in self.nodes.iter().zip(tree.nodes()).enumerate() {
            let mut actual = actual.clone();
            actual.binding = None;
            let mut model = model.clone();
            model.binding = None;
            if let (Some(a), Some(m)) = (&mut actual.value, &mut model.value) {
                m.written = a.written;
                if self.dirty_inputs.contains(&model.id) {
                    a.text.clone_from(&m.text);
                }
            }
            assert_eq!(model, actual, "patch tree node {index}");
        }
    }
}

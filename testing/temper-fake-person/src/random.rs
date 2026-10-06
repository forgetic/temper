//! Seeded unplanned actions for fuzzy worlds.
use alloc::boxed::Box;
use skein_lib::Rng;
use temper_web_view::{DomEvent, NodeId, Tree};

#[derive(Debug)]
pub struct RandomPerson {
    rng: Rng,
}

impl RandomPerson {
    #[must_use]
    pub fn new(seed: u64) -> RandomPerson {
        RandomPerson { rng: Rng::new(seed) }
    }

    #[must_use]
    pub fn next(&mut self, tree: &Tree) -> Option<DomEvent> {
        let mut count = 0_u64;
        for node in tree.nodes() {
            if node.binding.is_some() {
                count = count.checked_add(1).expect("tree fits u32");
            }
        }
        let chosen = self.rng.below(count);
        let mut seen = 0_u64;
        let mut id = None;
        for node in tree.nodes() {
            if node.binding.is_some() {
                if seen == chosen {
                    id = Some(node.id);
                    break;
                }
                seen = seen.checked_add(1).expect("tree fits u32");
            }
        }
        let id: NodeId = id?;
        let node = tree.find(id)?;
        match node.element {
            temper_web_view::Element::TextArea | temper_web_view::Element::Input(_) => {
                let letter =
                    b'a'.checked_add(u8::try_from(self.rng.below(26)).expect("letter fits")).expect("letter fits");
                Some(DomEvent::Input { node: id, text: Box::from([letter]) })
            }
            temper_web_view::Element::Main
            | temper_web_view::Element::Header
            | temper_web_view::Element::Nav
            | temper_web_view::Element::Section
            | temper_web_view::Element::Article
            | temper_web_view::Element::Aside
            | temper_web_view::Element::Footer
            | temper_web_view::Element::Div
            | temper_web_view::Element::Span
            | temper_web_view::Element::Heading(_)
            | temper_web_view::Element::Paragraph
            | temper_web_view::Element::List
            | temper_web_view::Element::OrderedList
            | temper_web_view::Element::Item
            | temper_web_view::Element::Link
            | temper_web_view::Element::Button
            | temper_web_view::Element::Form
            | temper_web_view::Element::Label
            | temper_web_view::Element::Radio
            | temper_web_view::Element::Dialog
            | temper_web_view::Element::Details
            | temper_web_view::Element::Summary
            | temper_web_view::Element::Strong
            | temper_web_view::Element::Emphasis
            | temper_web_view::Element::Code
            | temper_web_view::Element::Pre
            | temper_web_view::Element::Quote
            | temper_web_view::Element::Time
            | temper_web_view::Element::Progress
            | temper_web_view::Element::Text => Some(DomEvent::Press { node: id }),
        }
    }
}

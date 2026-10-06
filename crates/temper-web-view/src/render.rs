//! Build when the domain's visible generation moves, then diff (02-view.md, 6).

use skein_lib::Queue;
use temper_web_domain::Domain;

use crate::builder::Builder;
use crate::diff::{Patch, diff};
use crate::limits::Limits;
use crate::pages;
use crate::tree::Tree;

/// The last tree, a spare allocation, and stable node ids.
#[derive(Debug)]
pub struct View {
    last: Option<Tree>,
    spare: Option<Tree>,
    built: Option<u64>,
    next_id: u32,
}

impl View {
    #[must_use]
    pub fn new(limits: &Limits) -> View {
        View { last: Some(Tree::new(limits.nodes)), spare: Some(Tree::new(limits.nodes)), built: None, next_id: 0 }
    }

    #[must_use]
    pub fn tree(&self) -> &Tree {
        self.last.as_ref().expect("view has a last tree")
    }
}

/// Build and patch a changed view. An unchanged generation emits nothing.
pub fn render(view: &mut View, domain: &Domain, limits: &Limits, out: &mut Queue<Patch>) {
    if view.built == Some(domain.shown()) {
        return;
    }
    let spare = view.spare.take().expect("view has a spare tree");
    let mut builder = Builder::reuse(spare, limits.depth);
    pages::build(&mut builder, domain);
    let mut next = builder.finish();
    let last = view.last.as_ref().expect("view has a last tree");
    diff(last, &mut next, limits.depth, &mut view.next_id, out);
    let previous = view.last.replace(next).expect("view has a last tree");
    view.spare = Some(previous);
    view.built = Some(domain.shown());
}

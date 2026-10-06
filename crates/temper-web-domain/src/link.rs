//! Whether the browser currently has a usable engine link.
use skein_lib::Wall;

/// Link state the frame shows on every page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkState {
    Starting,
    Live,
    Behind,
    Offline { since: Wall },
}

impl LinkState {
    pub(crate) const fn offline(self) -> bool {
        match self {
            LinkState::Offline { .. } => true,
            LinkState::Starting | LinkState::Live | LinkState::Behind => false,
        }
    }
}

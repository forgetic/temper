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

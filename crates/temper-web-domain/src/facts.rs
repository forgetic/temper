//! Content-free observations for diagnostic traces.

/// A content-free observation; never affects decisions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fact {
    KeyConflict,
    StaleTerminal,
    UnexpectedEvent,
}

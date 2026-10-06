//! Typed browser addresses; the shell encodes and decodes their URLs.

/// A section of a task page, reserved even while W1 shows it as missing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Section {
    Conversation,
}

/// The person's destination.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Address {
    Inbox,
    Chats,
    Task { number: u64, section: Option<Section> },
}

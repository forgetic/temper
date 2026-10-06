//! One page at a time; W1's inbox address opens the chats page.
use crate::{Address, Cursor};
use alloc::boxed::Box;
use skein_lib::{List, Wall};

/// A chat summary returned in a paged read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChatLine {
    pub task: u64,
    pub project: u32,
    pub title: Box<[u8]>,
    pub live: bool,
    pub last_activity: Wall,
}

/// The person's chats, kept within one bounded window.
#[derive(Debug)]
pub struct Chats {
    pub rows: List<ChatLine>,
    pub older: Option<Cursor>,
    pub loading: bool,
}

impl Chats {
    pub(crate) fn new(window: u32) -> Chats {
        Chats { rows: List::with_capacity(window), older: None, loading: true }
    }
}

/// The one page shown by the client domain.
#[derive(Debug)]
pub enum Page {
    Starting,
    SignIn { then: Address },
    Missing { address: Address },
    Chats(Chats),
}

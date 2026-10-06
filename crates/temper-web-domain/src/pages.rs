//! One page at a time; W1's inbox address opens the chats page.
use crate::{Address, Cursor, Object, Query};
use alloc::boxed::Box;
use skein_lib::{Id, List, Wall};

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
    Task(TaskPage),
}

/// First-slice task page: one exchange, phase, held card and result.
#[derive(Debug)]
pub struct TaskPage {
    pub number: u64,
    pub chip: Option<Id<Object>>,
    pub escalation: Option<Id<Object>>,
    pub result: Option<Id<Object>>,
    pub first_words: Option<Box<[u8]>>,
    pub loading: bool,
    pub retry: Option<Query>,
}

impl TaskPage {
    pub(crate) const fn new(number: u64) -> TaskPage {
        TaskPage { number, chip: None, escalation: None, result: None, first_words: None, loading: true, retry: None }
    }
}

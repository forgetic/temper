use alloc::boxed::Box;
use skein_lib::{ReplyTo, Token, Wall};

/// Forge and user together identify a person; neither display field is a key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct IdentityKey {
    pub forge: u32,
    pub user: u64,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Identity {
    pub key: IdentityKey,
    pub login: Box<[u8]>,
    pub name: Box<[u8]>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InitialOwner {
    pub project: u32,
    pub identity: IdentityKey,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    Owner,
    Maintainer,
    Member,
    Observer,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Holding {
    pub person: u64,
    pub role: Role,
}

/// Supported requests grow with the increments that implement them; typed
/// variants for goals, tasks, inboxes, notes and watches will be added there.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ask {
    StartChat { project: u32, words: Box<[u8]> },
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    NotReady,
    SignIn,
    Role,
    Authority,
    Unknown,
    Ended,
    Busy,
    Limit,
    KeyConflict,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Outcome {
    Started { task: u64 },
    Refused(Refusal),
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Reply {
    SignedIn { person: u64, expires: Wall },
    SignedOut,
    Outcome(Outcome),
    Refused(Refusal),
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RequestKey {
    pub person: u64,
    pub key: [u8; 16],
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    Person(u64),
    SignIn(u64),
    Roles(u32),
    Answer(RequestKey),
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Stored {
    Person { number: u64, identity: Identity },
    SignIn { number: u64, person: u64, expires: Wall },
    Roles { project: u32, holdings: Box<[Holding]> },
    Answer { key: RequestKey, ask: Ask, outcome: Outcome, at: Wall },
}
impl Stored {
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Stored::Person { number, .. } => Key::Person(*number),
            Stored::SignIn { number, .. } => Key::SignIn(*number),
            Stored::Roles { project, .. } => Key::Roles(*project),
            Stored::Answer { key, .. } => Key::Answer(*key),
        }
    }
}
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Root-issued fresh candidate: used only when this identity is new.
    /// Existing identities keep their durable number; unused candidates are gaps.
    SignedIn {
        reply_to: ReplyTo,
        person: u64,
        sign_in: u64,
        identity: Identity,
    },
    SignOut {
        reply_to: ReplyTo,
        sign_in: u64,
    },
    Roles {
        project: u32,
        holdings: Box<[Holding]>,
    },
    Ask {
        reply_to: ReplyTo,
        sign_in: u64,
        key: [u8; 16],
        ask: Ask,
    },
    Decided {
        request: Token,
        outcome: Outcome,
    },
    Restore {
        record: Stored,
    },
    Restored,
}
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    Route { request: Token, person: u64, project: u32, role: Role, ask: Ask },
    Reply { to: ReplyTo, reply: Reply },
    Save { record: Stored },
    Erase { key: Key },
    RolesRefused { project: u32, refusal: Refusal },
    RestoreRefused { key: Key, refusal: Refusal },
}

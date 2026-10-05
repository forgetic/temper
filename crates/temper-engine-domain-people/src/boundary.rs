//! Typed root/people messages and durable child records (domain/people.md, sections
//! 3–5 and 12.1). The root supplies authenticated identities, roles and keyed
//! requests; people returns bounded routes and persistence intentions. Every
//! routed flight ends with `Decided`; the root commits before releasing replies.
//! These values contain no credential secrets, task internals or protocol bytes.

use alloc::boxed::Box;
use skein_lib::{ReplyTo, Token, Wall};

/// Forge and user together identify a person; neither display field is a key.
/// Protocol-authenticated forge/user identity supplied by the root; display bytes are not keys and
/// no secret is retained. (domain/people.md, section 3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct IdentityKey {
    /// Configured forge number authenticating the user. (domain/people.md, section 3).
    pub forge: u32,
    /// Stable forge user identifier; login changes do not change this key. (domain/people.md,
    /// section 3).
    pub user: u64,
}

/// Authenticated forge identity and owned display data, admitted under the combined identity-byte
/// limit. (domain/people.md, section 3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Identity {
    /// Authenticated stable forge/user pair. (domain/people.md, section 3).
    pub key: IdentityKey,
    /// Display login bytes; combined with `name`, at most `Limits::identity_bytes`.
    /// (domain/people.md, section 3).
    pub login: Box<[u8]>,
    /// Display-name bytes; combined with `login`, at most `Limits::identity_bytes`.
    /// (domain/people.md, section 3).
    pub name: Box<[u8]>,
}

/// Root-configured first-owner grant used only when this identity's person record is first made;
/// bootstrap matches are bounded. (domain/people.md, section 3.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InitialOwner {
    /// Project whose roles must already be initialized before first sign-in. (domain/people.md,
    /// section 3.1).
    pub project: u32,
    /// Configured forge/user pair to grant `Owner` on its first person creation. (domain/people.md,
    /// section 3.1).
    pub identity: IdentityKey,
}

/// People's project membership label; the root translates it to authority policy and performs the
/// actual action checks. (domain/people.md, section 4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    /// `Owner` membership; may start chats here, while broader owner actions remain root policy.
    /// (domain/people.md, section 4).
    Owner,
    /// Maintainer membership; may start chats here, with authority checked by the root.
    /// (domain/people.md, section 4).
    Maintainer,
    /// Member membership; may start chats here within the root's policy and funding check.
    /// (domain/people.md, section 4).
    Member,
    /// Observer membership; `StartChat` is refused and its keyed role outcome is saved.
    /// (domain/people.md, section 4).
    Observer,
}

/// One person's authoritative project role; a roles record permits at most one holding per person.
/// (domain/people.md, section 4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Holding {
    /// Deployment person number, checked for restored references when restoration finishes.
    /// (domain/people.md, section 4).
    pub person: u64,
    /// Authoritative membership role for this project. (domain/people.md, section 4).
    pub role: Role,
}

/// Supported requests grow with the increments that implement them; typed
/// variants for goals, tasks, inboxes, notes and watches will be added there.
/// Typed keyed request currently supported by people; only chat creation is implemented in this
/// increment. (domain/people.md, sections 5.1 and 12.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ask {
    /// Request a chat task; admitted member-or-higher asks route once per key to the root.
    /// (domain/people.md, sections 5.1 and 12.1).
    StartChat {
        /** Project in which the root should create the chat task. (domain/people.md, sections 5.1 and 12.1). */
        project: u32,
        /** Opening words, bounded by `Limits::words` before any route or saved answer. (domain/people.md, sections 5.1 and 12.1). */
        words: Box<[u8]>,
    },
}

/// Admission or root-decision reason; `Busy`/`NotReady` decisions release the flight without saving a
/// completed key, while other decided refusals are retained. (domain/people.md, sections 5.1.1 and
/// 12.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// Restoration is unfinished/failed, or the root reports transient admission unavailability.
    /// (domain/people.md, sections 5.1.1 and 12.1).
    NotReady,
    /// Sign-in is missing, expired or incompatible with the supplied identity. (domain/people.md,
    /// sections 5.1.1 and 12.1).
    SignIn,
    /// Project membership does not permit the supported request. (domain/people.md, sections 5.1.1
    /// and 12.1).
    Role,
    /// Root reports authority insufficient for the action. (domain/people.md, sections 5.1.1 and
    /// 12.1).
    Authority,
    /// Root names an unknown target, bootstrap lacks its initialized project, or restore references
    /// an unknown person. (domain/people.md, sections 5.1.1 and 12.1).
    Unknown,
    /// Root reports the requested target has ended. (domain/people.md, sections 5.1.1 and 12.1).
    Ended,
    /// Configured capacity is unavailable or the root reports transient admission pressure; a newly
    /// refused admission may retry. (domain/people.md, sections 5.1.1 and 12.1).
    Busy,
    /// Payload, configuration-derived arithmetic or restored record validity exceeds the supported
    /// bound. (domain/people.md, sections 5.1.1 and 12.1).
    Limit,
    /// Same person-scoped key names a different typed request. (domain/people.md, sections 5.1.1
    /// and 12.1).
    KeyConflict,
}

/// Root's terminal result for one routed flight; people replies to each bounded waiter and saves
/// nontransient outcomes with the parent's task decision. (domain/people.md, sections 5.1.1 and
/// 12.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Outcome {
    /// Root created the chat task; save the keyed answer with the task in one decision.
    /// (domain/people.md, sections 5.1.1 and 12.1).
    Started {
        /** Chat task number returned by the root; the parent withholds the reply until durability and people holds no task state. (domain/people.md, sections 5.1.1 and 12.1). */
        task: u64,
    },
    /// Request ended with a reason; permanent decisions are keyed, `Busy`/`NotReady` remain retryable.
    /// (domain/people.md, sections 5.1.1 and 12.1).
    Refused(
        /** Root or role refusal; transient `Busy`/`NotReady` is not retained as a completed key. (domain/people.md, sections 5.1.1 and 12.1). */
         Refusal,
    ),
}

/// Child-to-client answer routed by the parent; state-changing answers are withheld externally
/// until their decision is durable. (domain/people.md, sections 3 and 5.1.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Reply {
    /// Sign-in succeeded or an existing active same-person sign-in was replayed. (domain/people.md,
    /// sections 3 and 5.1.1).
    SignedIn {
        /** Stable deployment person number for the authenticated forge identity. (domain/people.md, sections 3 and 5.1.1). */
        person: u64,
        /** Saved wall-time expiry for this sign-in; its monotonic deadline is retained internally. (domain/people.md, sections 3 and 5.1.1). */
        expires: Wall,
    },
    /// Sign-out completed; an already absent number is also successful once ready.
    /// (domain/people.md, sections 3 and 5.1.1).
    SignedOut,
    /// One terminal response for a keyed call or its duplicate waiter. (domain/people.md, sections
    /// 3 and 5.1.1).
    Outcome(
        /** Terminal keyed result, or retryable root admission pressure for the current call. (domain/people.md, sections 3 and 5.1.1). */
         Outcome,
    ),
    /// Entrance refused before making a new answered-key record. (domain/people.md, sections 3 and
    /// 5.1.1).
    Refused(
        /** Direct entrance refusal with no newly saved answered-key record. (domain/people.md, sections 3 and 5.1.1). */
         Refusal,
    ),
}

/// Person-scoped idempotency key shared across that person's sign-ins; a different ask under the
/// same key is a conflict. (domain/people.md, section 5.1.1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RequestKey {
    /// Stable authenticated person owning this key across sign-ins. (domain/people.md, section
    /// 5.1.1).
    pub person: u64,
    /// Opaque web-selected 16-byte request key; the domain does not parse it. (domain/people.md,
    /// section 5.1.1).
    pub key: [u8; 16],
}

/// Typed logical store key for a people record; the protocol handles bytes and secrets outside this
/// domain. (domain/people.md, sections 3–5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    /// Logical person-record key. (domain/people.md, sections 3–5).
    Person(/** Stable deployment person number. (domain/people.md, sections 3–5). */ u64),
    /// Logical secret-free sign-in-record key. (domain/people.md, sections 3–5).
    SignIn(
        /** Root-issued deployment sign-in number; cookie secrets and token digests stay below the domain. (domain/people.md, section 3; domain/engine.md, sections 4 and 5.4). */
         u64,
    ),
    /// Logical per-project roles-record key. (domain/people.md, sections 3–5).
    Roles(/** Project whose authoritative holdings are stored. (domain/people.md, sections 3–5). */ u32),
    /// Logical completed-key record. (domain/people.md, sections 3–5).
    Answer(/** Person-scoped completed request key. (domain/people.md, sections 3–5). */ RequestKey),
}

/// Owned typed durable record submitted to the parent or restored from it; capacities and payload
/// bytes are checked when admitted. (domain/people.md, sections 3–5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Stored {
    /// Persistent person identity; forge/user and person numbers must be unique on restore.
    /// (domain/people.md, sections 3–5).
    Person {
        /** Stable person number, unique among restored people. (domain/people.md, sections 3–5). */
        number: u64,
        /** Unique forge/user identity and bounded display bytes. (domain/people.md, sections 3–5). */
        identity: Identity,
    },
    /// Persistent secret-free sign-in; expired restored entries are erased when restore completes.
    /// (domain/people.md, sections 3–5).
    SignIn {
        /** Root-issued sign-in number; at most `Limits::sign_ins` sign-in records are retained. (domain/people.md, section 3; domain/engine.md, section 4). */
        number: u64,
        /** Existing person reference, validated after all restore rows arrive. (domain/people.md, sections 3–5). */
        person: u64,
        /** Persisted wall-time expiry; restoration projects it onto the supplied startup monotonic clock. (domain/people.md, sections 3–5). */
        expires: Wall,
    },
    /// Persistent complete project holdings with bounded unique people. (domain/people.md, sections
    /// 3–5).
    Roles {
        /** Project whose live role set this record replaces or restores. (domain/people.md, sections 3–5). */
        project: u32,
        /** At most `Limits::holdings` distinct people; restored references are checked at completion. (domain/people.md, sections 3–5). */
        holdings: Box<[Holding]>,
    },
    /// Persistent request and outcome; same-key same-ask replay returns it without checking current
    /// roles anew. (domain/people.md, sections 3–5).
    Answer {
        /** Person-scoped completed key; restoration validates its person reference. (domain/people.md, sections 3–5). */
        key: RequestKey,
        /** Original typed request with words bounded by `Limits::words`, compared on every retry. (domain/people.md, sections 3–5). */
        ask: Ask,
        /** Retained permanent outcome returned without remaking the decision. (domain/people.md, sections 3–5). */
        outcome: Outcome,
        /** Wall time the answer was made; this increment has capacity-based retention, not timed eviction. (domain/people.md, sections 3–5). */
        at: Wall,
    },
}

impl Stored {
    /// Pure projection of this typed record's logical key; allocates nothing, emits no request and
    /// changes no state. (domain/people.md, sections 3–5).
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

/// Root-to-child inputs; authenticated identity, authoritative roles and typed restored records
/// cross this boundary without protocol secrets. (domain/people.md, sections 3–5 and 12.1).
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Root supplies a fresh person candidate and a fresh sign-in number. Only a new identity
    /// uses the person candidate; existing identities keep their durable person number, and unused
    /// person candidates leave gaps (domain/people.md, section 3.1; domain/engine.md, section 4).
    /// Admit an authenticated sign-in after restoration; bootstrap first-owner, person and sign-in
    /// writes share one root decision. (domain/people.md, sections 3–5 and 12.1).
    SignedIn {
        /// Opaque destination for the sign-in answer; the parent holds success until durability.
        /// (domain/people.md, sections 3–5 and 12.1).
        reply_to: ReplyTo,
        /// Root-issued never-reused fresh candidate, used only for a new forge/user identity;
        /// unused candidates leave gaps. (domain/people.md, sections 3–5 and 12.1).
        person: u64,
        /// Root-issued fresh deployment sign-in number; replaying an active same-person number
        /// returns its existing expiry. Cookie secrets stay below the domain (domain/people.md,
        /// section 3; domain/engine.md, sections 4 and 5.4).
        sign_in: u64,
        /// Protocol-authenticated identity with bounded display bytes and no credential.
        /// (domain/people.md, sections 3–5 and 12.1).
        identity: Identity,
    },
    /// End one sign-in after restoration, emitting erase if present and one reply.
    /// (domain/people.md, sections 3–5 and 12.1).
    SignOut {
        /// Destination for the terminal `SignedOut` reply. (domain/people.md, sections 3–5 and 12.1).
        reply_to: ReplyTo,
        /// Root-issued sign-in number to end; absence is an idempotent success once ready.
        /// (domain/people.md, sections 3–5 and 12.1).
        sign_in: u64,
    },
    /// Apply an already authorized root roles replacement; invalid/full updates emit `RolesRefused`
    /// and preserve old holdings. Failed restoration ignores this event. (domain/people.md,
    /// sections 3–5 and 12.1).
    Roles {
        /// Project whose authoritative holdings the root replaces. (domain/people.md, sections 3–5
        /// and 12.1).
        project: u32,
        /// Complete bounded holdings, with no duplicate person; root already authorized this
        /// change. (domain/people.md, sections 3–5 and 12.1).
        holdings: Box<[Holding]>,
    },
    /// Admit one keyed call after restoration; saved retries reply, pending duplicates join bounded
    /// waiters, and a new eligible flight routes once. (domain/people.md, sections 3–5 and 12.1).
    Ask {
        /// Destination admitted into at most `Limits::waiters` replies per flight.
        /// (domain/people.md, sections 3–5 and 12.1).
        reply_to: ReplyTo,
        /// Root-issued active authenticated sign-in number; wall or monotonic expiry refuses
        /// entrance (domain/people.md, sections 3–5 and 12.1).
        sign_in: u64,
        /// Opaque web key, scoped to the sign-in's stable person. (domain/people.md, sections 3–5
        /// and 12.1).
        key: [u8; 16],
        /// Typed request with payload bounds checked before lookup, route or mutation.
        /// (domain/people.md, sections 3–5 and 12.1).
        ask: Ask,
    },
    /// Complete a live `Route` exactly once, retire its flight and answer each waiter; permanent
    /// answer writes join the root's action commit. (domain/people.md, sections 3–5 and 12.1).
    Decided {
        /// Opaque token from `Route`; the root must return this live flight exactly once, or the
        /// child panics. (domain/people.md, sections 3–5 and 12.1).
        request: Token,
        /// Terminal root decision; persist permanent answers with task changes, but do not retain
        /// `Busy`/`NotReady` pressure. (domain/people.md, sections 3–5 and 12.1).
        outcome: Outcome,
    },
    /// Load one saved row while restoring; invalid input emits one `RestoreRefused` and permanently
    /// fails this instance. (domain/people.md, sections 3–5 and 12.1).
    Restore {
        /// One typed saved row, admitted once during restoring; duplicate/oversized rows fail
        /// restoration. (domain/people.md, sections 3–5 and 12.1).
        record: Stored,
    },
    /// Finish restoration, validate all person references and reproject sign-in deadlines; erase
    /// expired entries. Repeating after ready/failed is ignored. (domain/people.md, sections 3–5
    /// and 12.1).
    Restored,
}

/// Child-to-root outputs; persistence joins the root's decision, and the root withholds external
/// replies until required writes are durable. (domain/people.md, sections 5.1.1 and 12.1).
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// New eligible keyed flight for the root's authority/task decision; duplicate callers share
    /// this route. (domain/people.md, sections 5.1.1 and 12.1).
    Route {
        /** Opaque live flight token; root owes exactly one matching `Decided`, including admission pressure. (domain/people.md, sections 5.1.1 and 12.1). */
        request: Token,
        /** Authenticated stable requester number. (domain/people.md, sections 5.1.1 and 12.1). */
        person: u64,
        /** Project selected by the typed request. (domain/people.md, sections 5.1.1 and 12.1). */
        project: u32,
        /** Current membership checked by people for this admission; root still checks authority. (domain/people.md, sections 5.1.1 and 12.1). */
        role: Role,
        /** Original bounded request, owned by the root for routing and atomic decision. (domain/people.md, sections 5.1.1 and 12.1). */
        ask: Ask,
    },
    /// Terminal client output; the parent enforces durability before exposure. (domain/people.md,
    /// sections 5.1.1 and 12.1).
    Reply {
        /** Opaque client destination; each admitted call or waiter receives one terminal reply. (domain/people.md, sections 5.1.1 and 12.1). */
        to: ReplyTo,
        /** Answer to deliver only after any state-changing decision is durable. (domain/people.md, sections 5.1.1 and 12.1). */
        reply: Reply,
    },
    /// Typed persistence output to join the parent's atomic decision, including task creation and
    /// keyed answer together. (domain/people.md, sections 5.1.1 and 12.1).
    Save {
        /** Owned replacement row for the parent's current atomic decision, not a standalone IO submission. (domain/people.md, sections 5.1.1 and 12.1). */
        record: Stored,
    },
    /// Typed removal output for an ended or expired sign-in, with durability handled by the parent.
    /// (domain/people.md, sections 5.1.1 and 12.1).
    Erase {
        /** Logical row to remove with the parent's decision; secret cleanup remains below the domain. (domain/people.md, sections 5.1.1 and 12.1). */
        key: Key,
    },
    /// Terminal role-update refusal with no role replacement. (domain/people.md, sections 5.1.1 and
    /// 12.1).
    RolesRefused {
        /** Project whose role update was refused without replacing holdings. (domain/people.md, sections 5.1.1 and 12.1). */
        project: u32,
        /** Bound or capacity reason for the refused role update. (domain/people.md, sections 5.1.1 and 12.1). */
        refusal: Refusal,
    },
    /// Terminal restoration failure; a failed instance cannot become ready through more rows or
    /// `Restored`. (domain/people.md, sections 5.1.1 and 12.1).
    RestoreRefused {
        /** Record key that made restoration fail. (domain/people.md, sections 5.1.1 and 12.1). */
        key: Key,
        /** Restore admission/reference reason; this instance stays unready. (domain/people.md, sections 5.1.1 and 12.1). */
        refusal: Refusal,
    },
}

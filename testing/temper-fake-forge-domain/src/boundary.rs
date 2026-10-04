//! The boundary between the fake forge's domain and its protocol layer, or a
//! world standing in for it (programming-model.md, 4.4): one kind of call up,
//! and its replies and the forge's webhooks down. What the calls ask and
//! answer is the forge's API, in the `api` module.
//!
//! The contract:
//!
//! - Every [`Event::Call`] is answered by exactly one [`Request::Reply`],
//!   which consumes its `ReplyTo`: at once when the forge holds as many
//!   calls as it may (as unavailable), and otherwise when the call's
//!   latency is past. What a call does, it does when it arrives, so a reply
//!   that fails as timed out follows a call that was made.
//! - A [`Request::Hook`] has no terminal event: the forge sends it and hears
//!   nothing back, as a real forge's webhook deliveries go. Hooks are best
//!   effort: late, lost, or in another order than the changes they tell of.
//! - A call names its repository and its user, and the forge echoes
//!   neither: a reply goes back on its `ReplyTo`.

use alloc::boxed::Box;

use skein_lib::ReplyTo;

use crate::api::{Answer, Change, Error, Op};

/// protocol -> domain
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// A call by `user`: do `op` on `repository`, named in full.
    Call { reply_to: ReplyTo, user: u64, repository: Box<[u8]>, op: Op },
}

/// domain -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Terminal for `Call`: its answer, exactly one per call.
    Reply { to: ReplyTo, result: Result<Answer, Error> },
    /// A webhook to `repository`'s subscriber: something of `change` changed,
    /// about the item `number` if it names one; a push names its `branch`
    /// and the `commit` it moved to (none if it was deleted), and a status
    /// its `commit`. No event answers it.
    Hook {
        repository: Box<[u8]>,
        change: Change,
        number: Option<u64>,
        branch: Option<Box<[u8]>>,
        commit: Option<u64>,
        by: Option<u64>,
    },
}

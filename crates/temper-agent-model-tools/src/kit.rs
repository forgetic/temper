//! Kits: one session's tools. A session opens its kit when it starts, with the
//! authority its opener gave it, and closes it when it ends.
//!
//! A call is checked at the entrance, where refusing it costs nothing: the
//! family of tools it belongs to must be granted, its path must lie in a
//! repository of the checkout, and in a writable one for a change, and what it
//! would store must fit the limits.

use temper_lib::{Env, Id, Queue, ReplyTo, Token};

use crate::authority::{self, Authority, Checkout};
use crate::boundary::{Refusal, Request};
use crate::call::{Call, Outcome};
use crate::limits::Limits;
use crate::model::Model;

#[derive(Debug)]
pub(crate) struct Kit {
    /// The session's token, echoed when the kit closes.
    session: Token,
    checkout: Checkout,
}

pub(crate) fn open(
    model: &mut Model,
    env: &Env<Limits>,
    session: Token,
    authority: Authority,
    out: &mut Queue<Request>,
) {
    if model.kits.is_full() {
        out.push(Request::Refused { session, refusal: Refusal::Busy });
        return;
    }
    let Some(checkout) = authority::admit(authority, &env.limits) else {
        out.push(Request::Refused { session, refusal: Refusal::Invalid });
        return;
    };
    let id = model.kits.insert(Kit { session, checkout }).expect("checked for room above");
    out.push(Request::Opened { session, kit: id.token() });
}

pub(crate) fn call(
    model: &mut Model,
    env: &Env<Limits>,
    kit: Token,
    reply_to: ReplyTo,
    call: Call,
    out: &mut Queue<Request>,
) {
    let kit = model.kits.get(Id::from_token(kit)).expect("a kit lives until its session closes it");
    let outcome = match refusal(&kit.checkout, &call, &env.limits) {
        Some(outcome) => outcome,
        None => Outcome::Unsupported,
    };
    out.push(Request::Answer { to: reply_to, outcome });
}

pub(crate) fn close(model: &mut Model, kit: Token, out: &mut Queue<Request>) {
    let id = Id::from_token(kit);
    let kit = model.kits.get(id).expect("a kit lives until its session closes it");
    out.push(Request::Closed { session: kit.session });
    model.kits.retire(id);
}

/// What refuses `call` at the entrance, if anything does.
fn refusal(checkout: &Checkout, call: &Call, limits: &Limits) -> Option<Outcome> {
    if !authority::granted(checkout.grants, call) {
        return Some(Outcome::NotGranted);
    }
    let (path, content) = match call {
        Call::Read { path, .. } | Call::List { path } | Call::Search { path, .. } => {
            return authority::locate(checkout, path, limits.path_bytes).err();
        }
        Call::Write { path, content } => (path, Some(content)),
        Call::Edit { path, .. } => (path, None),
        Call::Shell { .. } => return None,
    };
    let located = match authority::locate(checkout, path, limits.path_bytes) {
        Ok(located) => located,
        Err(outcome) => return Some(outcome),
    };
    if !located.writable {
        return Some(Outcome::ReadOnly);
    }
    if let Some(content) = content {
        let size = u64::try_from(content.len()).unwrap_or(u64::MAX);
        if size > u64::from(limits.file_bytes) {
            return Some(Outcome::TooLarge { size });
        }
    }
    None
}

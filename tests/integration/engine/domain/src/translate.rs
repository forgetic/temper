//! The engine's protocol layer, as the world plays it: the engine's forge
//! operations as the fake forge's calls, through the forge child domain's
//! world's translation (`temper_engine_domain_forge_tests::translate`), with
//! the payloads the engine names filled in by the codecs ([`crate::codec`]),
//! and what the engine wrote found again in the fake's answers and decoded.
//!
//! - **Payloads.** A record, an outcome posted or a note's page goes out as
//!   the bytes its codec writes, in the comment or the wiki page the op
//!   names it in.
//! - **Decoded.** Every comment an answer shows, and every wiki page it
//!   reads, is decoded: what holds an engine payload is handed to the
//!   engine typed, beside the answer. A comment that starts as a record and
//!   does not decode is marked mangled: a person edited it.
//! - **Charters and outcomes** cross a worker's channel as bytes: the world
//!   encodes each charter the engine assigns, and decodes each outcome a
//!   scripted run answers with, as both sides' protocol layers would.

use std::collections::BTreeMap;

use temper_engine_domain::forge::api as engine;
use temper_engine_domain::{Decoded, Payload};
use temper_engine_domain_forge_tests::translate::{self as forge_world, Fill};
use temper_forge_domain::api as forge;
use temper_lib::Token;

use crate::codec;

pub use forge_world::{Asked, commit, count};

/// The payload token an engine op names, if it names one.
#[must_use]
pub fn payload_token(op: &engine::Op) -> Option<Token> {
    let body = match op {
        engine::Op::CreateIssue { body, .. }
        | engine::Op::Post { body, .. }
        | engine::Op::EditComment { body, .. }
        | engine::Op::Review { body, .. }
        | engine::Op::OpenPull { body, .. }
        | engine::Op::PutPage { content: body, .. } => body,
        engine::Op::Items { .. }
        | engine::Op::Item { .. }
        | engine::Op::Comment { .. }
        | engine::Op::Pull { .. }
        | engine::Op::PullFor { .. }
        | engine::Op::Reviews { .. }
        | engine::Op::Statuses { .. }
        | engine::Op::Remarks { .. }
        | engine::Op::Permission { .. }
        | engine::Op::Branch { .. }
        | engine::Op::Pages { .. }
        | engine::Op::Page { .. }
        | engine::Op::AddLabels { .. }
        | engine::Op::RemoveLabels { .. }
        | engine::Op::SetReviewers { .. }
        | engine::Op::SetDependencies { .. }
        | engine::Op::Reopen { .. }
        | engine::Op::Merge { .. }
        | engine::Op::Close { .. }
        | engine::Op::DeleteBranch { .. }
        | engine::Op::DeletePage { .. } => return None,
    };
    match body {
        engine::Body::Text(_) => None,
        engine::Body::Payload(token) | engine::Body::Record { payload: token, .. } => Some(*token),
    }
}

/// The fake's call for an engine op, with its payload written by its codec,
/// asking for pages of `page` entries.
#[must_use]
pub fn op(op: engine::Op, payload: Option<&Payload>, page: u32) -> (Asked, forge::Op) {
    let mut fill = Fill::new();
    if let Some(token) = payload_token(&op) {
        let payload = payload.expect("the engine fills in the payload its op names");
        fill.insert(token.raw(), bytes(payload));
    }
    forge_world::op(op, page, &fill)
}

/// The bytes of a payload.
#[must_use]
pub fn bytes(payload: &Payload) -> Vec<u8> {
    match payload {
        Payload::Record(record) => codec::record_block(record),
        Payload::Outcome(posted) => codec::posted_block(posted),
        Payload::Page(page) => codec::page(page),
    }
}

/// The full bodies of the comments the fake's answer shows, by id, and the
/// page it read, before the answer is cut to the engine's limits.
#[must_use]
pub fn bodies(result: &Result<forge::Answer, forge::Error>) -> (BTreeMap<u64, Vec<u8>>, Option<forge::Page>) {
    let mut bodies = BTreeMap::new();
    let mut page = None;
    match result {
        Ok(forge::Answer::Item { comments, .. }) => {
            for comment in comments {
                bodies.insert(comment.id, comment.body.to_vec());
            }
        }
        Ok(forge::Answer::Comment { comment, .. }) => {
            bodies.insert(comment.id, comment.body.to_vec());
        }
        Ok(forge::Answer::Page(found)) => page = Some(found.clone()),
        Ok(_) | Err(_) => {}
    }
    (bodies, page)
}

/// What the engine finds of its own in `answer`, the comments' full bodies
/// in `bodies`: its payloads, decoded, and a record that does not decode
/// marked mangled.
#[must_use]
pub fn decode(
    answer: &mut Result<engine::Answer, engine::Error>,
    bodies: &BTreeMap<u64, Vec<u8>>,
    page: Option<forge::Page>,
) -> Box<[Decoded]> {
    let mut decoded = Vec::new();
    let mut take = |comment: &mut engine::Comment| {
        let Some(body) = bodies.get(&comment.id) else { return };
        match codec::comment(comment.id, body) {
            Some(found) => decoded.push(found),
            None => {
                if let engine::Mark::Record { .. } = comment.mark {
                    comment.mark = engine::Mark::Mangled;
                }
            }
        }
    };
    match answer {
        Ok(engine::Answer::Item { comments, .. }) => {
            for comment in comments.iter_mut() {
                take(comment);
            }
        }
        Ok(engine::Answer::Comment(comment)) => take(comment),
        Ok(engine::Answer::Page(_)) => {
            if let Some(page) = page
                && let Some(note) = codec::page_of(&page.content)
            {
                decoded.push(Decoded::Page { name: page.name, page: Box::new(note) });
            }
        }
        Ok(_) | Err(_) => {}
    }
    decoded.into_boxed_slice()
}

//! Owned-byte checks for calls and replies (domain/forge.md, section 5.4).
//!
//! This module keeps no state and knows no task policy. `op` checks a
//! retained request before admission; `answer` checks returned allocations
//! before an answer can affect the working set or a caller.
use crate::Limits;
use crate::api::{Answer, File, Op, Read, Summary, Write};
use alloc::boxed::Box;

fn bytes(value: &[u8]) -> Option<u64> {
    u64::try_from(value.len()).ok()
}
fn optional(value: Option<&[u8]>) -> Option<u64> {
    match value {
        Some(value) => bytes(value),
        None => Some(0),
    }
}
fn rows(count: usize, width: usize, limit: u32) -> Option<u64> {
    let count = u64::try_from(count).ok()?;
    if count > u64::from(limit) {
        return None;
    }
    count.checked_mul(u64::try_from(width).ok()?)
}
pub(crate) fn op(value: &Op, l: &Limits) -> bool {
    match value {
        Op::Read(Read::Job { max_bytes, .. } | Read::File { max_bytes, .. }) => {
            if *max_bytes == 0 || *max_bytes > l.answer_bytes {
                return false;
            }
        }
        Op::Read(_) | Op::Write(_) => {}
    }
    let held = match value {
        Op::Read(value) => read(value),
        Op::Write(value) => write(value, l),
    };
    match held {
        Some(held) => held <= u64::from(l.op_bytes),
        None => false,
    }
}
pub(crate) fn read(value: &Read) -> Option<u64> {
    match value {
        Read::File { path, .. } => {
            if path.is_empty() {
                None
            } else {
                bytes(path)
            }
        }
        Read::PullFor { head, base } => bytes(head)?.checked_add(bytes(base)?),
        Read::Branch { branch } | Read::Protection { branch } => bytes(branch),
        Read::Items { page, .. }
        | Read::Reviews { page, .. }
        | Read::Statuses { page, .. }
        | Read::Remarks { page, .. }
        | Read::PullFiles { page, .. }
        | Read::Collaborators { page } => {
            if *page == 0 {
                None
            } else {
                Some(0)
            }
        }
        Read::Job { .. }
        | Read::Item { .. }
        | Read::Pull { .. }
        | Read::Compare { .. }
        | Read::Checks { .. }
        | Read::Branches
        | Read::Settings
        | Read::Permission { .. } => Some(0),
    }
}
pub(crate) fn write(value: &Write, l: &Limits) -> Option<u64> {
    match value {
        Write::CreateIssue { key, title, body } => bytes(key)?.checked_add(bytes(title)?)?.checked_add(bytes(body)?),
        Write::OpenPull { title, body, head, base } => {
            bytes(title)?.checked_add(bytes(body)?)?.checked_add(bytes(head)?)?.checked_add(bytes(base)?)
        }
        Write::Post { key, body, .. } | Write::Review { key, body, .. } => bytes(key)?.checked_add(bytes(body)?),
        Write::Edit { title, body, .. } => optional(title.as_deref())?.checked_add(optional(body.as_deref())?),
        Write::SetReviewers { reviewers, .. } => rows(reviewers.len(), size_of::<u64>(), l.rows),
        Write::Status { context, .. } => bytes(context),
        Write::CreateBranch { branch, .. } | Write::DeleteBranch { branch } => bytes(branch),
        Write::Close { .. } | Write::Reopen { .. } | Write::Update { .. } | Write::Merge { .. } => Some(0),
    }
}
fn summary(value: &Summary, limit: u32) -> Option<u64> {
    let mut held = rows(value.labels.len(), size_of::<Box<[u8]>>(), limit)?;
    for label in &value.labels {
        held = held.checked_add(bytes(label)?)?;
    }
    held.checked_add(optional(value.key.as_deref())?)?
        .checked_add(bytes(&value.title)?)?
        .checked_add(bytes(&value.body)?)
}
fn files(values: &[File], limit: u32) -> Option<u64> {
    let mut held = rows(values.len(), size_of::<File>(), limit)?;
    for value in values {
        held = held
            .checked_add(bytes(&value.path)?)?
            .checked_add(optional(value.before.as_deref())?)?
            .checked_add(optional(value.after.as_deref())?)?;
    }
    Some(held)
}
pub(crate) fn answer(value: &Answer, l: &Limits) -> bool {
    match answer_heap(value, l.rows) {
        Some(held) => held <= u64::from(l.answer_bytes),
        None => false,
    }
}
fn answer_heap(value: &Answer, limit: u32) -> Option<u64> {
    match value {
        Answer::Items { items, .. } => {
            let mut held = rows(items.len(), size_of::<Summary>(), limit)?;
            for item in items {
                held = held.checked_add(summary(item, limit)?)?;
            }
            Some(held)
        }
        Answer::Item { item, comments, .. } => {
            let mut held =
                rows(comments.len(), size_of::<crate::api::Comment>(), limit)?.checked_add(summary(item, limit)?)?;
            for comment in comments {
                held = held.checked_add(optional(comment.key.as_deref())?)?.checked_add(bytes(&comment.body)?)?;
            }
            Some(held)
        }
        Answer::Pull(value) => pull(value, limit),
        Answer::Reviews { reviews, .. } => {
            let mut held = rows(reviews.len(), size_of::<crate::api::Review>(), limit)?;
            for review in reviews {
                held = held.checked_add(optional(review.key.as_deref())?)?.checked_add(bytes(&review.body)?)?;
            }
            Some(held)
        }
        Answer::Statuses { statuses, .. } | Answer::Checks(statuses) => {
            let mut held = rows(statuses.len(), size_of::<crate::api::Status>(), limit)?;
            for status in statuses {
                held = held
                    .checked_add(bytes(&status.context)?)?
                    .checked_add(bytes(&status.description)?)?
                    .checked_add(bytes(&status.url)?)?;
            }
            Some(held)
        }
        Answer::Job { log, .. } => bytes(log),
        Answer::File { path, bytes: value, .. } => bytes(path)?.checked_add(bytes(value)?),
        Answer::Remarks { remarks, .. } => {
            let mut held = rows(remarks.len(), size_of::<crate::api::Remark>(), limit)?;
            for remark in remarks {
                held = held.checked_add(bytes(&remark.path)?)?.checked_add(bytes(&remark.body)?)?;
            }
            Some(held)
        }
        Answer::PullFiles { files: values, .. } => files(values, limit),
        Answer::Compare { files: values, commits, .. } => {
            files(values, limit)?.checked_add(rows(commits.len(), size_of::<crate::api::Commit>(), limit)?)
        }
        Answer::Protection(value) => match value {
            Some(value) => {
                let mut held =
                    rows(value.contexts.len(), size_of::<Box<[u8]>>(), limit)?.checked_add(bytes(&value.branch)?)?;
                for context in &value.contexts {
                    held = held.checked_add(bytes(context)?)?;
                }
                Some(held)
            }
            None => Some(0),
        },
        Answer::Settings(value) => bytes(&value.default_branch),
        Answer::Collaborators { collaborators, .. } => {
            rows(collaborators.len(), size_of::<crate::api::Collaborator>(), limit)
        }
        Answer::Branches(branches) => {
            let mut held = rows(branches.len(), size_of::<Box<[u8]>>(), limit)?;
            for branch in branches {
                held = held.checked_add(bytes(branch)?)?;
            }
            Some(held)
        }
        Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Branch(_)
        | Answer::Done => Some(0),
    }
}
pub(crate) fn answer_bytes(value: &Answer, l: &Limits) -> Option<u64> {
    answer_heap(value, l.rows)
}

pub(crate) fn answer_bytes_unbounded(value: &Answer) -> Option<u64> {
    answer_heap(value, u32::MAX)
}

fn pull(value: &crate::api::Pull, limit: u32) -> Option<u64> {
    rows(value.reviewers.len(), size_of::<u64>(), limit)?
        .checked_add(bytes(&value.head)?)?
        .checked_add(bytes(&value.base)?)
}
pub(crate) fn effect_bytes(effect: &crate::Effect) -> Option<u64> {
    let held = match &effect.write {
        Write::CreateIssue { key, title, body } => bytes(key)?.checked_add(bytes(title)?)?.checked_add(bytes(body)?)?,
        Write::OpenPull { title, body, head, base } => {
            bytes(title)?.checked_add(bytes(body)?)?.checked_add(bytes(head)?)?.checked_add(bytes(base)?)?
        }
        Write::Post { key, body, .. } | Write::Review { key, body, .. } => bytes(key)?.checked_add(bytes(body)?)?,
        Write::Edit { title, body, .. } => optional(title.as_deref())?.checked_add(optional(body.as_deref())?)?,
        Write::SetReviewers { reviewers, .. } => {
            u64::try_from(reviewers.len()).ok()?.checked_mul(u64::try_from(size_of::<u64>()).ok()?)?
        }
        Write::Status { context, .. } => bytes(context)?,
        Write::CreateBranch { branch, .. } | Write::DeleteBranch { branch } => bytes(branch)?,
        Write::Close { .. } | Write::Reopen { .. } | Write::Update { .. } | Write::Merge { .. } => 0,
    };
    match &effect.condition {
        crate::Condition::Merge { base } => held.checked_add(bytes(base)?),
        crate::Condition::None | crate::Condition::Update { .. } | crate::Condition::Review { .. } => Some(held),
    }
}

pub(crate) fn stored_bytes(row: &crate::Stored) -> Option<u64> {
    match row {
        crate::Stored::Repository(_) => Some(0),
        crate::Stored::Live(row) => {
            let mut held = match &row.watch.resource.what {
                crate::What::Branch(branch) => bytes(branch)?,
                crate::What::Repository | crate::What::Pull(_) | crate::What::Issue(_) => 0,
            };
            for count in [row.comments.len(), row.reviews.len(), row.echoes.len()] {
                held = held.checked_add(u64::try_from(count).ok()?.checked_mul(16)?)?;
            }
            if let Some(item) = &row.cached.item {
                held = held.checked_add(
                    u64::try_from(item.labels.len()).ok()?.checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
                )?;
                for label in &item.labels {
                    held = held.checked_add(bytes(label)?)?;
                }
                held = held
                    .checked_add(optional(item.key.as_deref())?)?
                    .checked_add(bytes(&item.title)?)?
                    .checked_add(bytes(&item.body)?)?;
            }
            if let Some(pull) = &row.cached.pull {
                held = held
                    .checked_add(u64::try_from(pull.reviewers.len()).ok()?.checked_mul(8)?)?
                    .checked_add(bytes(&pull.head)?)?
                    .checked_add(bytes(&pull.base)?)?;
            }
            Some(held)
        }
    }
}
pub(crate) fn cached(value: &crate::Cached, l: &Limits) -> bool {
    let item = match &value.item {
        Some(item) => summary(item, l.rows),
        None => Some(0),
    };
    let pull = match &value.pull {
        Some(value) => pull(value, l.rows),
        None => Some(0),
    };
    match item {
        Some(bytes) if bytes <= u64::from(l.answer_bytes) => {}
        Some(_) | None => return false,
    }
    match pull {
        Some(bytes) => bytes <= u64::from(l.answer_bytes),
        None => false,
    }
}

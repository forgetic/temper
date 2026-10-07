//! Forge read and effect values decoded at the Smith host-tool boundary.
//! The root still checks adoption, grants, resource pins and outbox order.

use alloc::boxed::Box;
use skein_lib::{List, Time};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_client::api;

use crate::{Problem, json::Value};

pub(crate) fn repository(value: &Value) -> Result<api::Repository, Problem> {
    Ok(api::Repository {
        forge: value.required(b"forge")?.narrow()?,
        repository: value.required(b"repository")?.small()?,
    })
}

pub(crate) fn resource(value: &Value) -> Result<forge::What, Problem> {
    match value.required(b"kind")?.text()?.as_ref() {
        b"repository" => Ok(forge::What::Repository),
        b"pull" => Ok(forge::What::Pull(value.required(b"number")?.number()?)),
        b"issue" => Ok(forge::What::Issue(value.required(b"number")?.number()?)),
        b"branch" => Ok(forge::What::Branch(texts(value.required(b"segments")?)?)),
        _ => Err(Problem::Range),
    }
}

pub(crate) fn topic(value: &Value) -> Result<forge::Topic, Problem> {
    let repository = repository(value.required(b"repository")?)?;
    match value.required(b"kind")?.text()?.as_ref() {
        b"landings" => Ok(forge::Topic::Landings { repository, branch: value.required(b"branch")?.text()? }),
        b"ci" => Ok(forge::Topic::Ci { repository, head: commit(value.required(b"head")?)? }),
        b"pull" => Ok(forge::Topic::Pull { repository, number: value.required(b"number")?.number()? }),
        b"participation" => {
            Ok(forge::Topic::Participation { repository, number: value.required(b"number")?.number()? })
        }
        _ => Err(Problem::Range),
    }
}

pub(crate) fn read(value: &Value) -> Result<api::Read, Problem> {
    let kind = value.required(b"kind")?.text()?;
    match kind.as_ref() {
        b"items" => Ok(api::Read::Items {
            since: Time::from_nanos(value.required(b"since")?.number()?),
            page: value.required(b"page")?.small()?,
            kind: match value.field(b"item_kind")? {
                Some(kind) => match kind.text()?.as_ref() {
                    b"issue" => Some(api::Kind::Issue),
                    b"pull" => Some(api::Kind::Pull),
                    _ => return Err(Problem::Range),
                },
                None => None,
            },
        }),
        b"item" => Ok(api::Read::Item {
            number: value.required(b"number")?.number()?,
            after: value.required(b"after")?.number()?,
        }),
        b"pull" => Ok(api::Read::Pull { number: value.required(b"number")?.number()? }),
        b"pull_for" => {
            Ok(api::Read::PullFor { head: value.required(b"head")?.text()?, base: value.required(b"base")?.text()? })
        }
        b"reviews" => Ok(api::Read::Reviews {
            number: value.required(b"number")?.number()?,
            page: value.required(b"page")?.small()?,
        }),
        b"statuses" => Ok(api::Read::Statuses {
            commit: commit(value.required(b"commit")?)?,
            page: value.required(b"page")?.small()?,
        }),
        b"remarks" => Ok(api::Read::Remarks {
            number: value.required(b"number")?.number()?,
            review: value.required(b"review")?.number()?,
            page: value.required(b"page")?.small()?,
        }),
        b"branch" => Ok(api::Read::Branch { branch: value.required(b"branch")?.text()? }),
        b"branches" => Ok(api::Read::Branches),
        b"pull_files" => Ok(api::Read::PullFiles {
            number: value.required(b"number")?.number()?,
            head: commit(value.required(b"head")?)?,
            page: value.required(b"page")?.small()?,
        }),
        b"compare" => Ok(api::Read::Compare {
            before: commit(value.required(b"before")?)?,
            after: commit(value.required(b"after")?)?,
        }),
        b"checks" => Ok(api::Read::Checks { commit: commit(value.required(b"commit")?)? }),
        b"job" => Ok(api::Read::Job {
            attempt: api::JobAttempt {
                head: commit(value.required(b"head")?)?,
                run: value.required(b"run")?.number()?,
                job: value.required(b"job")?.number()?,
                attempt: value.required(b"attempt")?.small()?,
            },
            max_bytes: value.required(b"max_bytes")?.small()?,
        }),
        b"protection" => Ok(api::Read::Protection { branch: value.required(b"branch")?.text()? }),
        b"settings" => Ok(api::Read::Settings),
        b"collaborators" => Ok(api::Read::Collaborators { page: value.required(b"page")?.small()? }),
        b"permission" => Ok(api::Read::Permission { user: value.required(b"user")?.number()? }),
        _ => Err(Problem::Range),
    }
}

/// Bind one declared connector read name to its decoder discriminator.
pub(crate) fn read_named(name: &[u8], value: &Value) -> Result<api::Read, Problem> {
    let kind: &[u8] = match name {
        b"read_items" => b"items",
        b"read_item" => b"item",
        b"read_pull" => b"pull",
        b"read_pull_for" => b"pull_for",
        b"read_reviews" => b"reviews",
        b"read_statuses" => b"statuses",
        b"read_remarks" => b"remarks",
        b"read_branch" => b"branch",
        b"read_branches" => b"branches",
        b"read_pull_files" => b"pull_files",
        b"read_compare" => b"compare",
        b"read_checks" => b"checks",
        b"read_job" => b"job",
        b"read_protection" => b"protection",
        b"read_settings" => b"settings",
        b"read_collaborators" => b"collaborators",
        b"read_permission" => b"permission",
        _ => return Err(Problem::UnknownTool),
    };
    let fields = match value {
        Value::Object(fields) => fields,
        Value::Array(_) | Value::String(_) | Value::Number(_) | Value::Boolean(_) | Value::Null => {
            return Err(Problem::Type);
        }
    };
    let Ok(capacity) = u32::try_from(fields.len().checked_add(1).ok_or(Problem::TooLarge)?) else {
        return Err(Problem::TooLarge);
    };
    let mut named = List::with_capacity(capacity);
    if named.push((Box::from(&b"kind"[..]), Value::String(Box::from(kind)))).is_err() {
        return Err(Problem::TooLarge);
    }
    for (key, item) in fields {
        if key.as_ref() == b"kind" {
            return Err(Problem::Range);
        }
        if named.push((key.clone(), item.clone())).is_err() {
            return Err(Problem::TooLarge);
        }
    }
    read(&Value::Object(named.into_boxed()))
}

pub(crate) fn write(value: &Value) -> Result<api::Write, Problem> {
    let kind = value.required(b"kind")?.text()?;
    match kind.as_ref() {
        b"create_issue" => Ok(api::Write::CreateIssue {
            key: value.required(b"key")?.text()?,
            title: value.required(b"title")?.text()?,
            body: value.required(b"body")?.text()?,
        }),
        b"open_pull" => Ok(api::Write::OpenPull {
            title: value.required(b"title")?.text()?,
            body: value.required(b"body")?.text()?,
            head: value.required(b"head")?.text()?,
            base: value.required(b"base")?.text()?,
        }),
        b"post" => Ok(api::Write::Post {
            number: value.required(b"number")?.number()?,
            key: value.required(b"key")?.text()?,
            body: value.required(b"body")?.text()?,
        }),
        b"review" => {
            let verdict = match value.required(b"verdict")?.text()?.as_ref() {
                b"approve" => api::Verdict::Approve,
                b"request_changes" => api::Verdict::RequestChanges,
                b"comment" => api::Verdict::Comment,
                _ => return Err(Problem::Range),
            };
            Ok(api::Write::Review {
                number: value.required(b"number")?.number()?,
                key: value.required(b"key")?.text()?,
                verdict,
                body: value.required(b"body")?.text()?,
            })
        }
        b"edit" => Ok(api::Write::Edit {
            number: value.required(b"number")?.number()?,
            title: optional_text(value, b"title")?,
            body: optional_text(value, b"body")?,
        }),
        b"set_reviewers" => Ok(api::Write::SetReviewers {
            number: value.required(b"number")?.number()?,
            reviewers: numbers(value.required(b"reviewers")?)?,
        }),
        b"close" => Ok(api::Write::Close { number: value.required(b"number")?.number()? }),
        b"reopen" => Ok(api::Write::Reopen { number: value.required(b"number")?.number()? }),
        b"merge" => Ok(api::Write::Merge {
            number: value.required(b"number")?.number()?,
            head: commit(value.required(b"head")?)?,
        }),
        b"update" => Ok(api::Write::Update { number: value.required(b"number")?.number()? }),
        b"status" => {
            let check = match value.required(b"check")?.text()?.as_ref() {
                b"pending" => api::Check::Pending,
                b"passed" => api::Check::Passed,
                b"failed" => api::Check::Failed,
                _ => return Err(Problem::Range),
            };
            Ok(api::Write::Status {
                commit: commit(value.required(b"commit")?)?,
                context: value.required(b"context")?.text()?,
                check,
            })
        }
        b"create_branch" => Ok(api::Write::CreateBranch {
            branch: value.required(b"branch")?.text()?,
            commit: commit(value.required(b"commit")?)?,
        }),
        b"delete_branch" => Ok(api::Write::DeleteBranch { branch: value.required(b"branch")?.text()? }),
        _ => Err(Problem::Range),
    }
}

fn commit(value: &Value) -> Result<api::Commit, Problem> {
    let text = value.text()?;
    if text.len() != 64 {
        return Err(Problem::Range);
    }
    let mut commit = [0_u8; 32];
    for (at, cell) in commit.iter_mut().enumerate() {
        let high = text.get(at.checked_mul(2).ok_or(Problem::Range)?).ok_or(Problem::Range)?;
        let low_at = at.checked_mul(2).ok_or(Problem::Range)?.checked_add(1).ok_or(Problem::Range)?;
        let low = text.get(low_at).ok_or(Problem::Range)?;
        *cell = high_hex(*high)?
            .checked_mul(16)
            .ok_or(Problem::Range)?
            .checked_add(high_hex(*low)?)
            .ok_or(Problem::Range)?;
    }
    Ok(commit)
}

fn high_hex(value: u8) -> Result<u8, Problem> {
    match value {
        b'0'..=b'9' => Ok(value.saturating_sub(b'0')),
        b'a'..=b'f' => Ok(value.saturating_sub(b'a').saturating_add(10)),
        b'A'..=b'F' => Ok(value.saturating_sub(b'A').saturating_add(10)),
        _ => Err(Problem::Range),
    }
}

fn optional_text(value: &Value, name: &[u8]) -> Result<Option<Box<[u8]>>, Problem> {
    match value.field(name)? {
        Some(value) => Ok(Some(value.text()?)),
        None => Ok(None),
    }
}

pub(crate) fn texts(value: &Value) -> Result<Box<[Box<[u8]>]>, Problem> {
    let items = value.array()?;
    let Ok(capacity) = u32::try_from(items.len()) else { return Err(Problem::TooLarge) };
    let mut out = List::with_capacity(capacity);
    for item in items {
        let Ok(()) = out.push(item.text()?) else { return Err(Problem::TooLarge) };
    }
    Ok(out.into_boxed())
}

fn numbers(value: &Value) -> Result<Box<[u64]>, Problem> {
    let items = value.array()?;
    let Ok(capacity) = u32::try_from(items.len()) else { return Err(Problem::TooLarge) };
    let mut out = List::with_capacity(capacity);
    for item in items {
        let Ok(()) = out.push(item.number()?) else { return Err(Problem::TooLarge) };
    }
    Ok(out.into_boxed())
}

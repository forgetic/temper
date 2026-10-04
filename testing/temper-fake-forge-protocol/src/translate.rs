//! One wire request maps to one visible domain call; startup facts are reads
//! of neutral metadata. Multi-step engine policy and retries are above us.
use crate::config::Config;
use alloc::boxed::Box;
use skein_lib::{List, Time, Writer};
use temper_fake_forge_domain::{self as domain, api};
use temper_forge_forgejo::{
    Limits, binary,
    request::{self, Operation},
    types::{self, Document},
};

#[derive(Debug)]
pub enum Dispatch {
    Immediate(Document),
    Call { repository: Box<[u8]>, op: api::Op },
}
pub(crate) fn full_name(repository: &request::Repository) -> Box<[u8]> {
    let length = repository
        .owner
        .len()
        .checked_add(repository.name.len())
        .expect("bounded repository")
        .checked_add(1)
        .expect("separator");
    let mut out = Writer::new(length);
    out.put(&repository.owner).expect("measured");
    out.put(b"/").expect("measured");
    out.put(&repository.name).expect("measured");
    out.finish()
}
/// Fake object ids preserve its numeric store identity in their first 8 bytes;
/// the remaining bytes are zero, with SHA1's 12-byte pad always zero.
#[must_use]
pub fn commit(id: u64) -> [u8; 32] {
    let mut out = [0; 32];
    for (out, byte) in out.iter_mut().zip(id.to_be_bytes()) {
        *out = byte;
    }
    out
}
pub fn commit_id(hex: &[u8], format: types::ObjectFormat) -> Result<u64, api::Error> {
    let value = match binary::commit(hex, format) {
        Ok(value) => value,
        Err(error) => return Err(document_error(error)),
    };
    for byte in value.get(8..).expect("suffix") {
        if *byte != 0 {
            return Err(api::Error::Missing(api::What::Commit));
        }
    }
    Ok(u64::from_be_bytes(value.get(..8).expect("prefix").try_into().expect("eight")))
}
fn document_error(_error: temper_forge_forgejo::Error) -> api::Error {
    api::Error::TooLarge
}
fn state(value: types::State) -> api::State {
    match value {
        types::State::Open => api::State::Open,
        types::State::Closed => api::State::Closed,
    }
}
fn list_page(length: usize, page: request::Page, index: usize) -> bool {
    let start = u64::from(page.number.saturating_sub(1)).saturating_mul(u64::from(page.limit));
    let at = u64::try_from(index).expect("slice");
    at >= start && at < start.saturating_add(u64::from(page.limit)) && index < length
}
#[expect(
    clippy::too_many_lines,
    clippy::manual_map,
    reason = "exhaustive operation table and Option projection without combinators"
)]
pub fn dispatch(
    request: &request::Request,
    user: u64,
    config: &Config,
    forge: &domain::Domain,
    settings: &domain::Config,
    limits: &Limits,
) -> Result<Dispatch, api::Error> {
    match &request.operation {
        Operation::Settings => {
            return Ok(Dispatch::Immediate(Document::Settings {
                max_response_items: limits.page,
                default_paging_num: config.default_page,
            }));
        }
        Operation::CurrentUser => return Ok(Dispatch::Immediate(Document::User(config.user(user)?))),
        Operation::SearchUser { id } => {
            let users = match config.user(*id) {
                Ok(user) => Box::from([user]),
                Err(_) => Box::from([]),
            };
            return Ok(Dispatch::Immediate(Document::Users(users)));
        }
        Operation::Repository
        | Operation::Labels { .. }
        | Operation::Items(_)
        | Operation::Item { .. }
        | Operation::Comments { .. }
        | Operation::Comment { .. }
        | Operation::Pull { .. }
        | Operation::PullFor { .. }
        | Operation::Reviews { .. }
        | Operation::Statuses { .. }
        | Operation::Remarks { .. }
        | Operation::Permission { .. }
        | Operation::Branch { .. }
        | Operation::Pages { .. }
        | Operation::Page { .. }
        | Operation::Dependencies { .. }
        | Operation::CreateIssue { .. }
        | Operation::Post { .. }
        | Operation::EditComment { .. }
        | Operation::AddLabels { .. }
        | Operation::RemoveLabel { .. }
        | Operation::OpenPull { .. }
        | Operation::Merge { .. }
        | Operation::Review { .. }
        | Operation::Reviewers { .. }
        | Operation::Dependency { .. }
        | Operation::EditState { .. }
        | Operation::DeleteBranch { .. }
        | Operation::PutPage { .. }
        | Operation::DeletePage { .. } => {}
    }
    let repository = full_name(request.repository.as_ref().ok_or(api::Error::Missing(api::What::Repository))?);
    let descriptor = config.repository(&repository)?;
    if forge.permission(&repository, user)? < api::Permission::Read {
        return Err(api::Error::Forbidden);
    }
    let op = match &request.operation {
        Operation::Repository => {
            let metadata = forge.metadata(&repository)?;
            return Ok(Dispatch::Immediate(Document::Repository(types::RepositoryInfo {
                id: descriptor.id,
                full_name: metadata.name,
                object_format: descriptor.object_format,
                has_wiki: descriptor.has_wiki,
                default_branch: metadata.default_branch,
            })));
        }
        Operation::Labels { page } => {
            let answer = forge.inspect(settings, &repository, &api::Read::Labels)?;
            let names = answer_labels(answer)?;
            let mut labels = List::with_capacity(page.limit);
            for (index, name) in names.iter().enumerate() {
                if list_page(names.len(), *page, index) {
                    labels.push(label(descriptor, name)?).expect("page");
                }
            }
            return Ok(Dispatch::Immediate(Document::Labels(labels.into_boxed())));
        }
        Operation::Items(items) => {
            let labels = match &items.label {
                Some(label) => Box::from([label.clone()]),
                None => Box::from([]),
            };
            api::Op::Read(api::Read::Items {
                state: match items.state {
                    Some(value) => Some(state(value)),
                    None => None,
                },
                kind: match items.pulls {
                    Some(value) => Some(item_kind(value)),
                    None => None,
                },
                labels,
                author: match &items.author {
                    Some(login) => Some(config.user_id(login)?),
                    None => None,
                },
                since: Time::from_nanos(items.since.unwrap_or(0)),
                page: items.page.number,
                limit: items.page.limit,
            })
        }
        Operation::Item { number } | Operation::Comments { number, .. } => {
            api::Op::Read(api::Read::Item { number: *number, after: 0 })
        }
        Operation::Comment { id } => api::Op::Read(api::Read::Comment { id: *id }),
        Operation::Pull { number } | Operation::Reviews { number, .. } | Operation::Remarks { number, .. } => {
            api::Op::Read(api::Read::Pull { number: *number })
        }
        Operation::PullFor { base, head } => {
            api::Op::Read(api::Read::PullFor { base: base.clone(), head: head.clone() })
        }
        Operation::Statuses { commit, page: _ } => {
            api::Op::Read(api::Read::Statuses { commit: commit_id(commit, descriptor.object_format)? })
        }
        Operation::Permission { login } => api::Op::Read(api::Read::Permission { user: config.user_id(login)? }),
        Operation::Branch { name } => api::Op::Read(api::Read::Branch { branch: name.clone() }),
        Operation::Pages { .. } => api::Op::Read(api::Read::Pages { after: None }),
        Operation::Page { name } => api::Op::Read(api::Read::Page { name: name.clone() }),
        Operation::Dependencies { number, .. } => api::Op::Read(api::Read::Dependencies { number: *number }),
        Operation::CreateIssue { title, body, labels } => {
            let mut names = List::with_capacity(limits.fields);
            for id in labels {
                if let Ok(name) = label_name(descriptor, *id)
                    && names.push(name).is_err()
                {
                    return Err(api::Error::Full);
                }
            }
            api::Op::Write(api::Write::CreateIssue {
                title: title.clone(),
                body: body.clone(),
                labels: names.into_boxed(),
            })
        }
        Operation::Post { number, body } => api::Op::Write(api::Write::Comment { number: *number, body: body.clone() }),
        Operation::EditComment { id, body } => api::Op::Write(api::Write::EditComment { id: *id, body: body.clone() }),
        Operation::AddLabels { number, labels } => {
            let mut known = List::with_capacity(limits.fields);
            for name in labels {
                if label(descriptor, name).is_ok() && known.push(name.clone()).is_err() {
                    return Err(api::Error::Full);
                }
            }
            api::Op::Write(api::Write::AddLabels { number: *number, labels: known.into_boxed() })
        }
        Operation::RemoveLabel { number, label: wire } => {
            api::Op::Write(api::Write::RemoveLabels { number: *number, labels: Box::from([wire.clone()]) })
        }
        Operation::OpenPull { title, body, head, base } => api::Op::Write(api::Write::OpenPull {
            title: title.clone(),
            body: body.clone(),
            head: head.clone(),
            base: base.clone(),
        }),
        Operation::Merge { number, style, head } => {
            if style.as_ref() != b"merge" {
                return Err(api::Error::Refused);
            }
            api::Op::Write(api::Write::Merge { number: *number, head: commit_id(head, descriptor.object_format)? })
        }
        Operation::Review { number, event, body } => {
            api::Op::Write(api::Write::Review { number: *number, verdict: verdict(*event)?, body: body.clone() })
        }
        Operation::Reviewers { number, remove, reviewers } => {
            let answer = forge.inspect(settings, &repository, &api::Read::Pull { number: *number })?;
            let pull = answer_pull(answer)?;
            let mut kept = List::with_capacity(limits.fields);
            for id in &pull.reviewers {
                let login = config.user(*id)?.login;
                if !contains(reviewers, &login) && kept.push(*id).is_err() {
                    return Err(api::Error::Full);
                }
            }
            if !remove {
                for login in reviewers {
                    if kept.push(config.user_id(login)?).is_err() {
                        return Err(api::Error::Full);
                    }
                }
            }
            api::Op::Write(api::Write::SetReviewers { number: *number, reviewers: kept.into_boxed() })
        }
        Operation::Dependency { number, remove, dependency, owner, repository: other } => {
            if owner != &request.repository.as_ref().expect("repo").owner
                || other != &request.repository.as_ref().expect("repo").name
            {
                return Err(api::Error::Refused);
            }
            let answer = forge.inspect(settings, &repository, &api::Read::Dependencies { number: *number })?;
            let old = answer_dependencies(answer)?;
            let mut kept = List::with_capacity(limits.fields);
            for id in &old {
                if *id != *dependency && kept.push(*id).is_err() {
                    return Err(api::Error::Full);
                }
            }
            if !remove && kept.push(*dependency).is_err() {
                return Err(api::Error::Full);
            }
            api::Op::Write(api::Write::SetDependencies { number: *number, dependencies: kept.into_boxed() })
        }
        Operation::EditState { number, state: types::State::Open } => {
            api::Op::Write(api::Write::Reopen { number: *number })
        }
        Operation::EditState { number, state: types::State::Closed } => {
            api::Op::Write(api::Write::Close { number: *number })
        }
        Operation::DeleteBranch { name } => api::Op::Write(api::Write::DeleteBranch { branch: name.clone() }),
        Operation::PutPage { name, content_base64, .. } => api::Op::Write(api::Write::PutPage {
            name: name.clone(),
            content: match binary::base64_decode(content_base64, limits.body_bytes) {
                Ok(value) => value,
                Err(error) => return Err(document_error(error)),
            },
        }),
        Operation::DeletePage { name } => api::Op::Write(api::Write::DeletePage { name: name.clone() }),
        Operation::Settings | Operation::CurrentUser | Operation::SearchUser { .. } => unreachable!("startup above"),
    };
    Ok(Dispatch::Call { repository, op })
}
fn item_kind(pulls: bool) -> api::Kind {
    if pulls { api::Kind::Pull } else { api::Kind::Issue }
}
fn contains(names: &[Box<[u8]>], name: &[u8]) -> bool {
    for old in names {
        if old.as_ref() == name {
            return true;
        }
    }
    false
}
fn verdict(event: types::ReviewState) -> Result<Option<api::Verdict>, api::Error> {
    match event {
        types::ReviewState::Approved => Ok(Some(api::Verdict::Approve)),
        types::ReviewState::RequestChanges => Ok(Some(api::Verdict::RequestChanges)),
        types::ReviewState::Comment => Ok(Some(api::Verdict::Comment)),
        types::ReviewState::Pending => Ok(None),
        types::ReviewState::Dismissed | types::ReviewState::Requested => Err(api::Error::Refused),
    }
}
fn label(repository: &crate::config::Repository, name: &[u8]) -> Result<types::Label, api::Error> {
    for label in &repository.labels {
        if label.name.as_ref() == name {
            return Ok(label.clone());
        }
    }
    Err(api::Error::Missing(api::What::Label))
}
fn label_name(repository: &crate::config::Repository, id: u64) -> Result<Box<[u8]>, api::Error> {
    for label in &repository.labels {
        if label.id == id {
            return Ok(label.name.clone());
        }
    }
    Err(api::Error::Missing(api::What::Label))
}

fn answer_labels(answer: api::Answer) -> Result<Box<[Box<[u8]>]>, api::Error> {
    match answer {
        api::Answer::Labels(value) => Ok(value),
        api::Answer::Items { .. }
        | api::Answer::Item { .. }
        | api::Answer::Pull(_)
        | api::Answer::Statuses(_)
        | api::Answer::Permission(_)
        | api::Answer::Comment { .. }
        | api::Answer::Dependencies(_)
        | api::Answer::Commit(_)
        | api::Answer::Tree(_)
        | api::Answer::File(_)
        | api::Answer::Pages { .. }
        | api::Answer::Page(_)
        | api::Answer::Created(_)
        | api::Answer::Commented(_)
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Revision(_)
        | api::Answer::Done
        | api::Answer::Cloned { .. }
        | api::Answer::Pushed(_)
        | api::Answer::Branch(_) => Err(api::Error::Refused),
    }
}

fn answer_pull(answer: api::Answer) -> Result<api::Pull, api::Error> {
    match answer {
        api::Answer::Pull(value) => Ok(value),
        api::Answer::Items { .. }
        | api::Answer::Item { .. }
        | api::Answer::Statuses(_)
        | api::Answer::Permission(_)
        | api::Answer::Comment { .. }
        | api::Answer::Dependencies(_)
        | api::Answer::Labels(_)
        | api::Answer::Commit(_)
        | api::Answer::Tree(_)
        | api::Answer::File(_)
        | api::Answer::Pages { .. }
        | api::Answer::Page(_)
        | api::Answer::Created(_)
        | api::Answer::Commented(_)
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Revision(_)
        | api::Answer::Done
        | api::Answer::Cloned { .. }
        | api::Answer::Pushed(_)
        | api::Answer::Branch(_) => Err(api::Error::Refused),
    }
}

fn answer_dependencies(answer: api::Answer) -> Result<Box<[u64]>, api::Error> {
    match answer {
        api::Answer::Dependencies(value) => Ok(value),
        api::Answer::Items { .. }
        | api::Answer::Item { .. }
        | api::Answer::Pull(_)
        | api::Answer::Statuses(_)
        | api::Answer::Permission(_)
        | api::Answer::Comment { .. }
        | api::Answer::Labels(_)
        | api::Answer::Commit(_)
        | api::Answer::Tree(_)
        | api::Answer::File(_)
        | api::Answer::Pages { .. }
        | api::Answer::Page(_)
        | api::Answer::Created(_)
        | api::Answer::Commented(_)
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Revision(_)
        | api::Answer::Done
        | api::Answer::Cloned { .. }
        | api::Answer::Pushed(_)
        | api::Answer::Branch(_) => Err(api::Error::Refused),
    }
}

fn summary(
    value: api::Summary,
    repository: &crate::config::Repository,
    config: &Config,
    limits: &Limits,
) -> Result<types::Item, api::Error> {
    let mut labels = List::with_capacity(limits.fields);
    for name in &value.labels {
        if labels.push(label(repository, name)?).is_err() {
            return Err(api::Error::Full);
        }
    }
    Ok(types::Item {
        number: value.number,
        kind: match value.kind {
            api::Kind::Issue => types::ItemKind::Issue,
            api::Kind::Pull => types::ItemKind::Pull,
        },
        state: match value.state {
            api::State::Open => types::State::Open,
            api::State::Closed => types::State::Closed,
        },
        user: config.user(value.author)?,
        title: value.title,
        body: value.body,
        labels: labels.into_boxed(),
        created: value.created.as_nanos(),
        updated: value.updated.as_nanos(),
    })
}
pub(crate) fn comment(
    value: api::Comment,
    repository: &[u8],
    number: u64,
    config: &Config,
) -> Result<types::Comment, api::Error> {
    let number = skein_lib::Decimal::of(number);
    let length = b"/api/v1/repos/"
        .len()
        .checked_add(repository.len())
        .expect("bounded")
        .checked_add(b"/issues/".len())
        .expect("fixed")
        .checked_add(number.as_bytes().len())
        .expect("number");
    let mut url = Writer::new(length);
    url.put(b"/api/v1/repos/").expect("measured");
    url.put(repository).expect("measured");
    url.put(b"/issues/").expect("measured");
    url.put(number.as_bytes()).expect("measured");
    let digest = binary::digest(&value.body);
    Ok(types::Comment {
        id: value.id,
        user: config.user(value.author)?,
        body: value.body,
        issue_url: url.finish(),
        created: value.created.as_nanos(),
        updated: value.edited.unwrap_or(value.created).as_nanos(),
        revision: u64::from_be_bytes(digest.get(..8).expect("prefix").try_into().expect("eight")),
    })
}
#[expect(clippy::manual_map, reason = "the subset uses exhaustive Option projection")]
fn pull(value: api::Pull, config: &Config, limits: &Limits) -> Result<types::Pull, api::Error> {
    let mut reviewers = List::with_capacity(limits.fields);
    for id in &value.reviewers {
        if reviewers.push(config.user(*id)?).is_err() {
            return Err(api::Error::Full);
        }
    }
    Ok(types::Pull {
        number: value.number,
        state: match value.state {
            api::State::Open => types::State::Open,
            api::State::Closed => types::State::Closed,
        },
        head: value.head,
        base: value.base,
        commit: commit(value.commit),
        base_commit: commit(value.base_commit.unwrap_or(0)),
        merged: value.merged.is_some(),
        merge_commit: match value.merged {
            Some(value) => Some(commit(value)),
            None => None,
        },
        mergeable: value.mergeable,
        reviewers: reviewers.into_boxed(),
    })
}
fn review(value: api::Review, config: &Config) -> Result<types::Review, api::Error> {
    Ok(types::Review {
        id: value.id,
        user: config.user(value.author)?,
        state: match value.verdict {
            api::Verdict::Approve => types::ReviewState::Approved,
            api::Verdict::RequestChanges => types::ReviewState::RequestChanges,
            api::Verdict::Comment => types::ReviewState::Comment,
        },
        commit: commit(value.commit),
        body: value.body,
        submitted: value.at.as_nanos(),
        official: value.official,
        dismissed: false,
    })
}
fn permission(value: api::Permission) -> types::Permission {
    match value {
        api::Permission::None => types::Permission::None,
        api::Permission::Read => types::Permission::Read,
        api::Permission::Write => types::Permission::Write,
        api::Permission::Admin => types::Permission::Admin,
    }
}
fn check(value: api::Check) -> types::Check {
    match value {
        api::Check::Pending => types::Check::Pending,
        api::Check::Passed => types::Check::Success,
        api::Check::Failed => types::Check::Failure,
    }
}
fn number(request: &request::Request) -> Result<u64, api::Error> {
    match &request.operation {
        Operation::Item { number }
        | Operation::Comments { number, .. }
        | Operation::Pull { number }
        | Operation::Reviews { number, .. }
        | Operation::Remarks { number, .. }
        | Operation::Dependencies { number, .. }
        | Operation::Post { number, .. }
        | Operation::AddLabels { number, .. }
        | Operation::RemoveLabel { number, .. }
        | Operation::Merge { number, .. }
        | Operation::Review { number, .. }
        | Operation::Reviewers { number, .. }
        | Operation::Dependency { number, .. }
        | Operation::EditState { number, .. } => Ok(*number),
        Operation::Settings
        | Operation::CurrentUser
        | Operation::SearchUser { .. }
        | Operation::Repository
        | Operation::Labels { .. }
        | Operation::Items(_)
        | Operation::Comment { .. }
        | Operation::PullFor { .. }
        | Operation::Statuses { .. }
        | Operation::Permission { .. }
        | Operation::Branch { .. }
        | Operation::Pages { .. }
        | Operation::Page { .. }
        | Operation::CreateIssue { .. }
        | Operation::EditComment { .. }
        | Operation::OpenPull { .. }
        | Operation::DeleteBranch { .. }
        | Operation::PutPage { .. }
        | Operation::DeletePage { .. } => Err(api::Error::Refused),
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive fake domain response projections; writes read their resulting stored rows"
)]
pub fn project(
    request: &request::Request,
    user: u64,
    answer: api::Answer,
    config: &Config,
    forge: &domain::Domain,
    settings: &domain::Config,
    limits: &Limits,
) -> Result<Document, api::Error> {
    let repository = full_name(request.repository.as_ref().ok_or(api::Error::Missing(api::What::Repository))?);
    let descriptor = config.repository(&repository)?;
    match answer {
        api::Answer::Items { items, .. } => {
            let mut out = List::with_capacity(limits.page);
            for value in items {
                if out.push(summary(value, descriptor, config, limits)?).is_err() {
                    return Err(api::Error::Full);
                }
            }
            Ok(Document::Items(out.into_boxed()))
        }
        api::Answer::Item { item, comments, more } => match &request.operation {
            Operation::Comments { since, .. } => {
                if more {
                    return Err(api::Error::TooLarge);
                }
                let mut out = List::with_capacity(settings.limits.page_size);
                for value in comments {
                    if value.edited.unwrap_or(value.created).as_nanos() >= since.unwrap_or(0)
                        && out.push(comment(value, &repository, item.number, config)?).is_err()
                    {
                        return Err(api::Error::Full);
                    }
                }
                Ok(Document::Comments(out.into_boxed()))
            }
            Operation::Settings
            | Operation::CurrentUser
            | Operation::SearchUser { .. }
            | Operation::Repository
            | Operation::Labels { .. }
            | Operation::Items(_)
            | Operation::Item { .. }
            | Operation::Comment { .. }
            | Operation::Pull { .. }
            | Operation::PullFor { .. }
            | Operation::Reviews { .. }
            | Operation::Statuses { .. }
            | Operation::Remarks { .. }
            | Operation::Permission { .. }
            | Operation::Branch { .. }
            | Operation::Pages { .. }
            | Operation::Page { .. }
            | Operation::Dependencies { .. }
            | Operation::CreateIssue { .. }
            | Operation::Post { .. }
            | Operation::EditComment { .. }
            | Operation::AddLabels { .. }
            | Operation::RemoveLabel { .. }
            | Operation::OpenPull { .. }
            | Operation::Merge { .. }
            | Operation::Review { .. }
            | Operation::Reviewers { .. }
            | Operation::Dependency { .. }
            | Operation::EditState { .. }
            | Operation::DeleteBranch { .. }
            | Operation::PutPage { .. }
            | Operation::DeletePage { .. } => Ok(Document::Item(summary(item, descriptor, config, limits)?)),
        },
        api::Answer::Comment { number, comment: value } => {
            Ok(Document::Comment(comment(value, &repository, number, config)?))
        }
        api::Answer::Pull(value) => match &request.operation {
            Operation::Reviews { page, .. } => {
                let mut out = List::with_capacity(page.limit);
                for (index, value) in value.reviews.into_iter().enumerate() {
                    if list_page(usize::MAX, *page, index) && out.push(review(value, config)?).is_err() {
                        return Err(api::Error::Full);
                    }
                }
                Ok(Document::Reviews(out.into_boxed()))
            }
            Operation::Remarks { .. } => Ok(Document::Remarks(Box::from([]))),
            Operation::PullFor { .. }
            | Operation::Settings
            | Operation::CurrentUser
            | Operation::SearchUser { .. }
            | Operation::Repository
            | Operation::Labels { .. }
            | Operation::Items(_)
            | Operation::Item { .. }
            | Operation::Comments { .. }
            | Operation::Comment { .. }
            | Operation::Pull { .. }
            | Operation::Statuses { .. }
            | Operation::Permission { .. }
            | Operation::Branch { .. }
            | Operation::Pages { .. }
            | Operation::Page { .. }
            | Operation::Dependencies { .. }
            | Operation::CreateIssue { .. }
            | Operation::Post { .. }
            | Operation::EditComment { .. }
            | Operation::AddLabels { .. }
            | Operation::RemoveLabel { .. }
            | Operation::OpenPull { .. }
            | Operation::Merge { .. }
            | Operation::Review { .. }
            | Operation::Reviewers { .. }
            | Operation::Dependency { .. }
            | Operation::EditState { .. }
            | Operation::DeleteBranch { .. }
            | Operation::PutPage { .. }
            | Operation::DeletePage { .. } => Ok(Document::Pull(pull(value, config, limits)?)),
        },
        api::Answer::Statuses(values) => {
            let page = match &request.operation {
                Operation::Statuses { page, .. } => *page,
                Operation::Settings
                | Operation::CurrentUser
                | Operation::SearchUser { .. }
                | Operation::Repository
                | Operation::Labels { .. }
                | Operation::Items(_)
                | Operation::Item { .. }
                | Operation::Comments { .. }
                | Operation::Comment { .. }
                | Operation::Pull { .. }
                | Operation::PullFor { .. }
                | Operation::Reviews { .. }
                | Operation::Remarks { .. }
                | Operation::Permission { .. }
                | Operation::Branch { .. }
                | Operation::Pages { .. }
                | Operation::Page { .. }
                | Operation::Dependencies { .. }
                | Operation::CreateIssue { .. }
                | Operation::Post { .. }
                | Operation::EditComment { .. }
                | Operation::AddLabels { .. }
                | Operation::RemoveLabel { .. }
                | Operation::OpenPull { .. }
                | Operation::Merge { .. }
                | Operation::Review { .. }
                | Operation::Reviewers { .. }
                | Operation::Dependency { .. }
                | Operation::EditState { .. }
                | Operation::DeleteBranch { .. }
                | Operation::PutPage { .. }
                | Operation::DeletePage { .. } => return Err(api::Error::Refused),
            };
            let total_count = u64::try_from(values.len()).expect("bounded");
            let mut state = types::Check::Success;
            let mut out = List::with_capacity(page.limit);
            for (index, value) in values.into_iter().enumerate() {
                let row_state = check(value.state);
                if row_state == types::Check::Failure
                    || (row_state == types::Check::Pending && state == types::Check::Success)
                {
                    state = row_state;
                }
                if list_page(usize::MAX, page, index)
                    && out
                        .push(types::Status {
                            context: value.context,
                            state: row_state,
                            creator: config.user(value.author)?,
                            description: Box::from([]),
                            target_url: Box::from([]),
                            created: value.at.as_nanos(),
                        })
                        .is_err()
                {
                    return Err(api::Error::Full);
                }
            }
            if total_count == 0 {
                state = types::Check::Pending;
            }
            Ok(Document::Statuses { state, total_count, statuses: out.into_boxed() })
        }
        api::Answer::Permission(value) => Ok(Document::Permission(permission(value))),
        api::Answer::Commit(value) => Ok(Document::Branch(commit(value))),
        api::Answer::Dependencies(values) => {
            let repository = request.repository.as_ref().expect("repository");
            let mut out = List::with_capacity(limits.page);
            for value in values {
                if out
                    .push(types::Dependency {
                        number: value,
                        owner: repository.owner.clone(),
                        repository: repository.name.clone(),
                    })
                    .is_err()
                {
                    return Err(api::Error::Full);
                }
            }
            Ok(Document::Dependencies(out.into_boxed()))
        }
        api::Answer::Labels(values) => {
            let mut out = List::with_capacity(limits.page);
            for name in values {
                if out.push(label(descriptor, &name)?).is_err() {
                    return Err(api::Error::Full);
                }
            }
            Ok(Document::Labels(out.into_boxed()))
        }
        api::Answer::Pages { pages, .. } => {
            let mut out = List::with_capacity(settings.limits.page_size);
            for page in pages {
                if out.push(types::WikiPage { title: page.name, content: None, sha: commit(page.revision) }).is_err() {
                    return Err(api::Error::Full);
                }
            }
            Ok(Document::Pages(out.into_boxed()))
        }
        api::Answer::Page(value) => Ok(Document::Page(types::WikiPage {
            title: value.name,
            content: Some(value.content),
            sha: commit(value.revision),
        })),
        api::Answer::Created(created) => stored_item(created, &repository, descriptor, config, forge, settings, limits),
        api::Answer::Commented(id) => stored_comment(id, &repository, config, forge, settings),
        api::Answer::Reviewed(id) => stored_review(id, request, user, &repository, config, forge, settings),
        api::Answer::Revision(_) => {
            let name = match &request.operation {
                Operation::PutPage { name, .. } => name,
                Operation::Settings
                | Operation::CurrentUser
                | Operation::SearchUser { .. }
                | Operation::Repository
                | Operation::Labels { .. }
                | Operation::Items(_)
                | Operation::Item { .. }
                | Operation::Comments { .. }
                | Operation::Comment { .. }
                | Operation::Pull { .. }
                | Operation::PullFor { .. }
                | Operation::Reviews { .. }
                | Operation::Statuses { .. }
                | Operation::Remarks { .. }
                | Operation::Permission { .. }
                | Operation::Branch { .. }
                | Operation::Pages { .. }
                | Operation::Page { .. }
                | Operation::Dependencies { .. }
                | Operation::CreateIssue { .. }
                | Operation::Post { .. }
                | Operation::EditComment { .. }
                | Operation::AddLabels { .. }
                | Operation::RemoveLabel { .. }
                | Operation::OpenPull { .. }
                | Operation::Merge { .. }
                | Operation::Review { .. }
                | Operation::Reviewers { .. }
                | Operation::Dependency { .. }
                | Operation::EditState { .. }
                | Operation::DeleteBranch { .. }
                | Operation::DeletePage { .. } => return Err(api::Error::Refused),
            };
            let answer = forge.inspect(settings, &repository, &api::Read::Page { name: name.clone() })?;
            page_answer(answer)
        }
        api::Answer::Done => done(request, &repository, descriptor, config, forge, settings, limits),
        api::Answer::Merged(_) => Ok(Document::Done),
        api::Answer::Tree(_)
        | api::Answer::File(_)
        | api::Answer::Cloned { .. }
        | api::Answer::Pushed(_)
        | api::Answer::Branch(_) => Err(api::Error::Refused),
    }
}
fn stored_item(
    number: u64,
    repository: &[u8],
    descriptor: &crate::config::Repository,
    config: &Config,
    forge: &domain::Domain,
    settings: &domain::Config,
    limits: &Limits,
) -> Result<Document, api::Error> {
    match forge.inspect(settings, repository, &api::Read::Item { number, after: 0 })? {
        api::Answer::Item { item, .. } => Ok(Document::Item(summary(item, descriptor, config, limits)?)),
        api::Answer::Items { .. }
        | api::Answer::Pull(_)
        | api::Answer::Statuses(_)
        | api::Answer::Permission(_)
        | api::Answer::Comment { .. }
        | api::Answer::Dependencies(_)
        | api::Answer::Labels(_)
        | api::Answer::Commit(_)
        | api::Answer::Tree(_)
        | api::Answer::File(_)
        | api::Answer::Pages { .. }
        | api::Answer::Page(_)
        | api::Answer::Created(_)
        | api::Answer::Commented(_)
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Revision(_)
        | api::Answer::Done
        | api::Answer::Cloned { .. }
        | api::Answer::Pushed(_)
        | api::Answer::Branch(_) => Err(api::Error::Refused),
    }
}
fn stored_comment(
    id: u64,
    repository: &[u8],
    config: &Config,
    forge: &domain::Domain,
    settings: &domain::Config,
) -> Result<Document, api::Error> {
    match forge.inspect(settings, repository, &api::Read::Comment { id })? {
        api::Answer::Comment { number, comment: value } => {
            Ok(Document::Comment(comment(value, repository, number, config)?))
        }
        api::Answer::Items { .. }
        | api::Answer::Item { .. }
        | api::Answer::Pull(_)
        | api::Answer::Statuses(_)
        | api::Answer::Permission(_)
        | api::Answer::Dependencies(_)
        | api::Answer::Labels(_)
        | api::Answer::Commit(_)
        | api::Answer::Tree(_)
        | api::Answer::File(_)
        | api::Answer::Pages { .. }
        | api::Answer::Page(_)
        | api::Answer::Created(_)
        | api::Answer::Commented(_)
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Revision(_)
        | api::Answer::Done
        | api::Answer::Cloned { .. }
        | api::Answer::Pushed(_)
        | api::Answer::Branch(_) => Err(api::Error::Refused),
    }
}
fn stored_review(
    id: u64,
    request: &request::Request,
    user: u64,
    repository: &[u8],
    config: &Config,
    forge: &domain::Domain,
    settings: &domain::Config,
) -> Result<Document, api::Error> {
    let number = number(request)?;
    let pull = answer_pull(forge.inspect(settings, repository, &api::Read::Pull { number })?)?;
    for value in &pull.reviews {
        if value.id == id {
            return Ok(Document::Review(review(value.clone(), config)?));
        }
    }
    match &request.operation {
        Operation::Review { event: types::ReviewState::Pending, body, .. } => Ok(Document::Review(types::Review {
            id,
            user: config.user(user)?,
            state: types::ReviewState::Pending,
            commit: commit(pull.commit),
            body: body.clone(),
            submitted: 0,
            official: forge.permission(repository, user)? >= api::Permission::Write,
            dismissed: false,
        })),
        Operation::Settings
        | Operation::CurrentUser
        | Operation::SearchUser { .. }
        | Operation::Repository
        | Operation::Labels { .. }
        | Operation::Items(_)
        | Operation::Item { .. }
        | Operation::Comments { .. }
        | Operation::Comment { .. }
        | Operation::Pull { .. }
        | Operation::PullFor { .. }
        | Operation::Reviews { .. }
        | Operation::Statuses { .. }
        | Operation::Remarks { .. }
        | Operation::Permission { .. }
        | Operation::Branch { .. }
        | Operation::Pages { .. }
        | Operation::Page { .. }
        | Operation::Dependencies { .. }
        | Operation::CreateIssue { .. }
        | Operation::Post { .. }
        | Operation::EditComment { .. }
        | Operation::AddLabels { .. }
        | Operation::RemoveLabel { .. }
        | Operation::OpenPull { .. }
        | Operation::Merge { .. }
        | Operation::Review { .. }
        | Operation::Reviewers { .. }
        | Operation::Dependency { .. }
        | Operation::EditState { .. }
        | Operation::DeleteBranch { .. }
        | Operation::PutPage { .. }
        | Operation::DeletePage { .. } => Err(api::Error::Refused),
    }
}
fn page_answer(answer: api::Answer) -> Result<Document, api::Error> {
    match answer {
        api::Answer::Page(value) => Ok(Document::Page(types::WikiPage {
            title: value.name,
            content: Some(value.content),
            sha: commit(value.revision),
        })),
        api::Answer::Items { .. }
        | api::Answer::Item { .. }
        | api::Answer::Pull(_)
        | api::Answer::Statuses(_)
        | api::Answer::Permission(_)
        | api::Answer::Comment { .. }
        | api::Answer::Dependencies(_)
        | api::Answer::Labels(_)
        | api::Answer::Commit(_)
        | api::Answer::Tree(_)
        | api::Answer::File(_)
        | api::Answer::Pages { .. }
        | api::Answer::Created(_)
        | api::Answer::Commented(_)
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Revision(_)
        | api::Answer::Done
        | api::Answer::Cloned { .. }
        | api::Answer::Pushed(_)
        | api::Answer::Branch(_) => Err(api::Error::Refused),
    }
}
fn done(
    request: &request::Request,
    repository: &[u8],
    descriptor: &crate::config::Repository,
    config: &Config,
    forge: &domain::Domain,
    settings: &domain::Config,
    limits: &Limits,
) -> Result<Document, api::Error> {
    match &request.operation {
        Operation::EditComment { id, .. } => stored_comment(*id, repository, config, forge, settings),
        Operation::EditState { number, .. } => {
            stored_item(*number, repository, descriptor, config, forge, settings, limits)
        }
        Operation::AddLabels { number, .. } => {
            match forge.inspect(settings, repository, &api::Read::Item { number: *number, after: 0 })? {
                api::Answer::Item { item, .. } => {
                    let mut out = List::with_capacity(limits.fields);
                    for name in &item.labels {
                        if out.push(label(descriptor, name)?).is_err() {
                            return Err(api::Error::Full);
                        }
                    }
                    Ok(Document::Labels(out.into_boxed()))
                }
                api::Answer::Items { .. }
                | api::Answer::Pull(_)
                | api::Answer::Statuses(_)
                | api::Answer::Permission(_)
                | api::Answer::Comment { .. }
                | api::Answer::Dependencies(_)
                | api::Answer::Labels(_)
                | api::Answer::Commit(_)
                | api::Answer::Tree(_)
                | api::Answer::File(_)
                | api::Answer::Pages { .. }
                | api::Answer::Page(_)
                | api::Answer::Created(_)
                | api::Answer::Commented(_)
                | api::Answer::Reviewed(_)
                | api::Answer::Merged(_)
                | api::Answer::Revision(_)
                | api::Answer::Done
                | api::Answer::Cloned { .. }
                | api::Answer::Pushed(_)
                | api::Answer::Branch(_) => Err(api::Error::Refused),
            }
        }
        Operation::Settings
        | Operation::CurrentUser
        | Operation::SearchUser { .. }
        | Operation::Repository
        | Operation::Labels { .. }
        | Operation::Items(_)
        | Operation::Item { .. }
        | Operation::Comments { .. }
        | Operation::Comment { .. }
        | Operation::Pull { .. }
        | Operation::PullFor { .. }
        | Operation::Reviews { .. }
        | Operation::Statuses { .. }
        | Operation::Remarks { .. }
        | Operation::Permission { .. }
        | Operation::Branch { .. }
        | Operation::Pages { .. }
        | Operation::Page { .. }
        | Operation::Dependencies { .. }
        | Operation::CreateIssue { .. }
        | Operation::Post { .. }
        | Operation::RemoveLabel { .. }
        | Operation::OpenPull { .. }
        | Operation::Merge { .. }
        | Operation::Review { .. }
        | Operation::Reviewers { .. }
        | Operation::Dependency { .. }
        | Operation::DeleteBranch { .. }
        | Operation::PutPage { .. }
        | Operation::DeletePage { .. } => Ok(Document::Done),
    }
}

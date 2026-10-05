//! Reads added for the connector (domain/forge.md, sections 17 and 20).
//! Comparisons deliberately do not page; excessive data is an explicit
//! refusal, never a short answer claiming to be complete.

use crate::api::{Answer, ChangedFile, Check, CheckSummary, Collaborator, Error, Settings, State, What};
use crate::domain::Domain;
use crate::git;
use crate::limits::Limits;
use crate::pulls;
use crate::store::{Repository, fits};
use alloc::boxed::Box;
use skein_lib::bytes::copy_of;
use skein_lib::{Id, List, Map};

pub(crate) fn changed(
    domain: &Domain,
    limits: &Limits,
    base: Option<u64>,
    head: u64,
) -> Result<Box<[ChangedFile]>, Error> {
    let empty = Map::with_capacity(0);
    let before = match base {
        Some(base) => &domain.object(base).ok_or(Error::Missing(What::Commit))?.tree,
        None => &empty,
    };
    let after = &domain.object(head).ok_or(Error::Missing(What::Commit))?.tree;
    let mut files = List::with_capacity(limits.files);
    for (path, old) in before {
        let new = after.get(&**path);
        if new != Some(old) {
            if files.room() == 0 {
                return Err(Error::TooLarge);
            }
            #[expect(clippy::manual_map, reason = "programming-model.md forbids closure-taking Option::map")]
            let after = match new {
                Some(content) => Some(copy_of(content)),
                None => None,
            };
            files
                .push(ChangedFile { path: copy_of(path), before: Some(copy_of(old)), after })
                .expect("changed-file room checked before copying");
        }
    }
    for (path, new) in after {
        if !before.contains_key(&**path) {
            if files.room() == 0 {
                return Err(Error::TooLarge);
            }
            files
                .push(ChangedFile { path: copy_of(path), before: None, after: Some(copy_of(new)) })
                .expect("changed-file room checked before copying");
        }
    }
    Ok(files.into_boxed())
}

pub(crate) fn pull_files(
    domain: &Domain,
    limits: &Limits,
    repository: &Repository,
    number: u64,
    page: u32,
    limit: u32,
) -> Result<Answer, Error> {
    let item = repository.item(number)?;
    let pull = item.pull.as_ref().ok_or(Error::Missing(What::Pull))?;
    let onto = match item.state {
        State::Closed => match pull.merged {
            Some(merged) => domain.object(merged).expect("a stored merge").parent,
            None => repository.branches.get(&*pull.base).copied(),
        },
        State::Open => repository.branches.get(&*pull.base).copied(),
    }
    .ok_or(Error::Missing(What::Branch))?;
    let files = changed(domain, limits, git::merge_base(domain, onto, pull.commit), pull.commit)?;
    let size = if limit == 0 { limits.page_size } else { limit.min(limits.page_size) };
    let skip = u64::from(page.saturating_sub(1)).saturating_mul(u64::from(size));
    let mut answer = List::with_capacity(size);
    let mut more = false;
    for (index, file) in files.into_iter().enumerate() {
        if u64::try_from(index).expect("bounded file index") < skip {
            continue;
        }
        if answer.len() == size {
            more = true;
            break;
        }
        answer.push(file).expect("a page's room checked");
    }
    Ok(Answer::PullFiles { head: pull.commit, files: answer.into_boxed(), more })
}

pub(crate) fn compare(
    domain: &Domain,
    limits: &Limits,
    repository: &Repository,
    base: u64,
    head: u64,
) -> Result<Answer, Error> {
    if !repository.has.contains(&base) || !repository.has.contains(&head) {
        return Err(Error::Missing(What::Commit));
    }
    let files = changed(domain, limits, Some(base), head)?;
    let old = git::ancestors(domain, base);
    let new = git::ancestors(domain, head);
    let mut commits = List::with_capacity(limits.commits);
    for &commit in &new {
        if !old.contains(&commit) {
            commits.push(commit).expect("one visit per commit");
        }
    }
    Ok(Answer::Comparison { base, head, files, commits: commits.into_boxed() })
}

pub(crate) fn checks(repository: &Repository, limits: &Limits, commit: u64) -> Result<Answer, Error> {
    if !repository.has.contains(&commit) {
        return Err(Error::Missing(What::Commit));
    }
    let mut summaries = List::with_capacity(limits.contexts);
    for status in pulls::statuses(repository, limits, commit) {
        let description: &[u8] = match status.state {
            Check::Pending => b"CI pending",
            Check::Passed => b"CI passed",
            Check::Failed => b"CI failed",
        };
        fits(description, limits.body_bytes)?;
        let mut link = List::with_capacity(limits.name_bytes);
        for &byte in b"/job/" {
            if link.push(byte).is_err() {
                return Err(Error::TooLarge);
            }
        }
        let mut digits = [0_u8; 20];
        let mut at = digits.len();
        let mut remainder = commit;
        for _ in 0..digits.len() {
            at = at.checked_sub(1).expect("a u64 has at most twenty digits");
            *digits.get_mut(at).expect("within the digit array") =
                u8::try_from(remainder.checked_rem(10).expect("ten is nonzero"))
                    .expect("a digit")
                    .checked_add(b'0')
                    .expect("an ASCII digit");
            remainder = remainder.checked_div(10).expect("ten is nonzero");
            if remainder == 0 {
                break;
            }
        }
        assert!(remainder == 0, "a u64 has at most twenty digits");
        for &byte in digits.get(at..).expect("within the digit array") {
            if link.push(byte).is_err() {
                return Err(Error::TooLarge);
            }
        }
        summaries
            .push(CheckSummary { status, description: copy_of(description), link: link.into_boxed() })
            .expect("one summary per admitted context");
    }
    Ok(Answer::Checks(summaries.into_boxed()))
}

pub(crate) fn settings(repository: &Repository) -> Answer {
    Answer::Settings(Settings {
        default: copy_of(&repository.default),
        merge: repository.styles.merge,
        rebase: repository.styles.rebase,
        squash: repository.styles.squash,
    })
}

pub(crate) fn collaborators(repository: &Repository) -> Answer {
    let mut users = List::with_capacity(repository.permissions.len());
    for (&user, &permission) in &repository.permissions {
        users.push(Collaborator { user, permission }).expect("one entry per collaborator");
    }
    Answer::Collaborators(users.into_boxed())
}

pub(crate) fn update(
    domain: &mut Domain,
    env: &skein_lib::Env<crate::Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get(id).expect("a repository of the forge");
    repository.require(user, crate::api::Permission::Write)?;
    if !repository.styles.merge {
        return Err(Error::Refused);
    }
    let item = repository.item(number)?;
    if item.state == State::Closed {
        return Err(Error::Closed);
    }
    let pull = item.pull.as_ref().ok_or(Error::Missing(What::Pull))?;
    let onto = *repository.branches.get(&*pull.base).ok_or(Error::Missing(What::Branch))?;
    let head = pull.commit;
    if git::is_ancestor(domain, onto, head) {
        return Ok(Answer::Done);
    }
    let tree = pulls::merged(domain, &env.limits.limits, onto, head)?;
    let branch = copy_of(&pull.head);
    if repository.is_protected(&branch) {
        return Err(Error::Protected);
    }
    let merged = git::store(domain, crate::Object { parent: Some(head), merge_parent: Some(onto), tree })?;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.has.insert(merged).expect("a repository has room for every commit");
    repository.branches.insert(copy_of(&branch), merged).expect("the head branch exists");
    git::moved(domain, env, id, &branch, Some(head), merged, user);
    Ok(Answer::Done)
}

//! Independent typed-protocol projection. Keys live in hexadecimal body
//! markers; fake counts stand in for full commit IDs. Paging is applied to
//! the fake's bounded snapshots; comparisons remain whole responses.
use skein_lib::{List, Time};
use temper_engine_domain_forge_client::{Limits, api as client};
use temper_fake_forge_domain::api as forge;
const KEY: &[u8] = b"<!-- temper:key ";
const END: &[u8] = b" -->\n";
#[must_use]
pub fn commit(number: u64) -> client::Commit {
    let mut id = [0; 32];
    id[..8].copy_from_slice(&number.to_be_bytes());
    id
}
fn number(id: client::Commit) -> u64 {
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&id[..8]);
    u64::from_be_bytes(bytes)
}
#[must_use]
pub fn keyed(key: &[u8], text: &[u8]) -> Box<[u8]> {
    let capacity = KEY
        .len()
        .checked_add(key.len().checked_mul(2).expect("world key bound"))
        .expect("world key bound")
        .checked_add(END.len())
        .expect("world key bound")
        .checked_add(text.len())
        .expect("world text bound");
    let mut bytes = List::with_capacity(u32::try_from(capacity).expect("world payload bound"));
    for byte in KEY {
        bytes.push(*byte).expect("marker capacity");
    }
    for byte in key {
        bytes.push(b"0123456789abcdef"[usize::from(*byte >> 4)]).expect("marker capacity");
        bytes.push(b"0123456789abcdef"[usize::from(*byte & 15)]).expect("marker capacity");
    }
    for byte in END {
        bytes.push(*byte).expect("marker capacity");
    }
    for byte in text {
        bytes.push(*byte).expect("marker capacity");
    }
    bytes.into_boxed()
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
#[must_use]
pub fn key_of(body: &[u8]) -> Option<Box<[u8]>> {
    let rest = body.strip_prefix(KEY)?;
    for end in 0..rest.len() {
        if rest.get(end..)?.starts_with(END) {
            if end % 2 != 0 {
                return None;
            }
            let mut key = List::with_capacity(u32::try_from(end / 2).expect("world body bound"));
            for pair in rest.get(..end)?.chunks_exact(2) {
                key.push(hex(pair[0])?.checked_mul(16)?.checked_add(hex(pair[1])?)?).expect("key capacity");
            }
            return Some(key.into_boxed());
        }
    }
    None
}
#[must_use]
pub fn op(op: &client::Op, l: &Limits) -> forge::Op {
    match op {
        client::Op::Read(read) => forge::Op::Read(read_op(read, l)),
        client::Op::Write(write) => forge::Op::Write(write_op(write)),
    }
}
fn read_op(read: &client::Read, l: &Limits) -> forge::Read {
    match read {
        client::Read::Items { since, page, kind } => forge::Read::Items {
            since: *since,
            page: *page,
            kind: match kind {
                Some(client::Kind::Issue) => Some(forge::Kind::Issue),
                Some(client::Kind::Pull) => Some(forge::Kind::Pull),
                None => None,
            },
            state: None,
            labels: Box::new([]),
            author: None,
            limit: l.rows,
        },
        client::Read::Item { number, after } => forge::Read::Item { number: *number, after: *after },
        client::Read::Pull { number } | client::Read::Reviews { number, .. } | client::Read::Remarks { number, .. } => {
            forge::Read::Pull { number: *number }
        }
        client::Read::PullFor { head, base } => forge::Read::PullFor { head: head.clone(), base: base.clone() },
        client::Read::Statuses { commit, .. } => forge::Read::Statuses { commit: number(*commit) },
        client::Read::Branch { branch } => forge::Read::Branch { branch: branch.clone() },
        client::Read::PullFiles { number, page, .. } => {
            forge::Read::PullFiles { number: *number, page: *page, limit: l.rows }
        }
        client::Read::Compare { before, after } => {
            forge::Read::Compare { base: number(*before), head: number(*after), page: 1, limit: l.rows }
        }
        client::Read::Checks { commit } => forge::Read::Checks { commit: number(*commit) },
        client::Read::Job { attempt, .. } => forge::Read::Job { commit: number(attempt.head), context: Box::new([]) },
        client::Read::Protection { branch } => forge::Read::Protection { branch: branch.clone() },
        client::Read::Settings => forge::Read::Settings,
        client::Read::Collaborators { .. } => forge::Read::Collaborators,
        client::Read::Permission { user } => forge::Read::Permission { user: *user },
    }
}
fn write_op(write: &client::Write) -> forge::Write {
    match write {
        client::Write::CreateIssue { key, title, body } => {
            forge::Write::CreateIssue { title: title.clone(), body: keyed(key, body), labels: Box::new([]) }
        }
        client::Write::OpenPull { title, body, head, base } => {
            forge::Write::OpenPull { title: title.clone(), body: body.clone(), head: head.clone(), base: base.clone() }
        }
        client::Write::Post { number, key, body } => forge::Write::Comment { number: *number, body: keyed(key, body) },
        client::Write::Review { number, key, verdict, body } => {
            forge::Write::Review { number: *number, body: keyed(key, body), verdict: Some(verdict_out(*verdict)) }
        }
        client::Write::Edit { number, title, body } => {
            forge::Write::EditItem { number: *number, title: title.clone(), body: body.clone() }
        }
        client::Write::SetReviewers { number, reviewers } => {
            forge::Write::SetReviewers { number: *number, reviewers: reviewers.clone() }
        }
        client::Write::Close { number } => forge::Write::Close { number: *number },
        client::Write::Reopen { number } => forge::Write::Reopen { number: *number },
        client::Write::Merge { number: pull, head } => forge::Write::Merge { number: *pull, head: number(*head) },
        client::Write::Update { number } => forge::Write::Update { number: *number },
        client::Write::Status { commit, context, check } => {
            forge::Write::Status { commit: number(*commit), context: context.clone(), state: check_out(*check) }
        }
        client::Write::CreateBranch { branch, commit } => {
            forge::Write::CreateBranch { branch: branch.clone(), commit: number(*commit) }
        }
        client::Write::DeleteBranch { branch } => forge::Write::DeleteBranch { branch: branch.clone() },
    }
}
#[must_use]
pub fn error(error: forge::Error, now: Time) -> client::Error {
    match error {
        forge::Error::Unavailable | forge::Error::Unreachable => client::Error::Unavailable,
        forge::Error::Timeout => client::Error::Timeout,
        forge::Error::RateLimited { reset } => client::Error::RateLimited { after: reset.saturating_since(now) },
        forge::Error::Forbidden => client::Error::Forbidden,
        forge::Error::Missing(what) => match what {
            forge::What::Job => client::Error::MissingJob,
            forge::What::Repository
            | forge::What::Item
            | forge::What::Pull
            | forge::What::Comment
            | forge::What::Label
            | forge::What::Branch
            | forge::What::Commit
            | forge::What::File
            | forge::What::Page
            | forge::What::Review => client::Error::Missing,
        },
        forge::Error::TooLarge => client::Error::TooLarge,
        forge::Error::Full => client::Error::Full,
        forge::Error::Exists => client::Error::Exists,
        forge::Error::Circular | forge::Error::Refused => client::Error::Refused,
        forge::Error::NothingToMerge => client::Error::NothingToMerge,
        forge::Error::Empty => client::Error::Empty,
        forge::Error::Closed => client::Error::Closed,
        forge::Error::Stale => client::Error::Stale,
        forge::Error::Conflict => client::Error::Conflict,
        forge::Error::Protected => client::Error::Protected,
    }
}
#[must_use]
pub fn answer(asked: &client::Op, answer: forge::Answer, l: &Limits) -> client::Answer {
    match answer {
        forge::Answer::Items { items, more, now } => {
            let mut rows = List::with_capacity(l.rows);
            for item in items {
                rows.push(summary(item)).expect("fake listing bounded");
            }
            client::Answer::Items { items: rows.into_boxed(), more, now }
        }
        forge::Answer::Item { item, comments, more } => {
            let mut rows = List::with_capacity(l.rows);
            for comment in comments {
                rows.push(client::Comment {
                    provenance: match comment.edited {
                        Some(_) => client::Provenance::Revised,
                        None => client::Provenance::Original,
                    },
                    id: comment.id,
                    author: comment.author,
                    revision: digest(&comment.body)
                        ^ comment.edited.unwrap_or(comment.created).as_nanos()
                        ^ if comment.edited.is_some() { u64::MAX } else { 0 },
                    key: key_of(&comment.body),
                    body: comment.body,
                    created: comment.created,
                })
                .expect("fake comments bounded");
            }
            client::Answer::Item { item: summary(item), comments: rows.into_boxed(), more }
        }
        forge::Answer::Pull(pull) => pull_answer(asked, pull, l),
        forge::Answer::Statuses(statuses) => statuses_answer(asked, &statuses, l),
        forge::Answer::Commit(id) => client::Answer::Commit(commit(id)),
        forge::Answer::PullFiles { head, files, more } => {
            client::Answer::PullFiles { head: commit(head), files: files_in(files), more }
        }
        forge::Answer::Comparison { base, head, files, commits } => {
            let mut ids = List::with_capacity(u32::try_from(commits.len()).expect("bounded fake comparison"));
            for id in commits {
                ids.push(commit(id)).expect("comparison capacity");
            }
            client::Answer::Compare {
                before: commit(base),
                after: commit(head),
                files: files_in(files),
                commits: ids.into_boxed(),
            }
        }
        forge::Answer::Checks(checks) => {
            let mut rows = List::with_capacity(u32::try_from(checks.len()).expect("bounded fake statuses"));
            for check in checks {
                let mut status = status(&check.status);
                status.description = check.description;
                status.url = check.link;
                rows.push(status).expect("checks capacity");
            }
            client::Answer::Checks(rows.into_boxed())
        }
        forge::Answer::Protection(protection) => client::Answer::Protection(match protection {
            Some(p) => Some(client::Protection {
                branch: p.branch,
                contexts: p.contexts,
                approvals: p.approvals,
                dismiss_stale: p.dismiss_stale,
            }),
            None => None,
        }),
        forge::Answer::Settings(s) => client::Answer::Settings(client::Settings {
            default_branch: s.default,
            merge: s.merge,
            rebase: s.rebase,
            squash: s.squash,
        }),
        forge::Answer::Collaborators(rows) => collaborators_answer(asked, &rows, l),
        forge::Answer::Permission(p) => client::Answer::Permission(permission(p)),
        forge::Answer::Created(n) => client::Answer::Created(n),
        forge::Answer::Commented(n) => client::Answer::Commented(n),
        forge::Answer::Reviewed(n) => client::Answer::Reviewed(n),
        forge::Answer::Merged(n) => client::Answer::Merged(commit(n)),
        forge::Answer::Branch(created) => client::Answer::Branch(match created {
            forge::Created::Created => client::BranchCreation::Created,
            forge::Created::Exists => client::BranchCreation::Exists,
        }),
        forge::Answer::Done => client::Answer::Done,
        forge::Answer::Comment { .. }
        | forge::Answer::Dependencies(_)
        | forge::Answer::Labels(_)
        | forge::Answer::Tree(_)
        | forge::Answer::File(_)
        | forge::Answer::Pages { .. }
        | forge::Answer::Page(_)
        | forge::Answer::Revision(_)
        | forge::Answer::Cloned { .. }
        | forge::Answer::Pushed(_) => panic!("new client does not ask these fake routes"),
    }
}
fn summary(s: forge::Summary) -> client::Summary {
    client::Summary {
        number: s.number,
        kind: match s.kind {
            forge::Kind::Issue => client::Kind::Issue,
            forge::Kind::Pull => client::Kind::Pull,
        },
        state: state(s.state),
        key: key_of(&s.body),
        title: s.title,
        body: s.body,
        labels: s.labels,
        author: s.author,
        updated: s.updated,
    }
}
fn state(s: forge::State) -> client::State {
    match s {
        forge::State::Open => client::State::Open,
        forge::State::Closed => client::State::Closed,
    }
}
#[expect(clippy::manual_map, reason = "the bounded client world uses concrete matches, without callbacks")]
fn pull_answer(asked: &client::Op, pull: forge::Pull, l: &Limits) -> client::Answer {
    match asked {
        client::Op::Read(read) => match read {
            client::Read::Pull { .. } | client::Read::PullFor { .. } => client::Answer::Pull(client::Pull {
                number: pull.number,
                state: state(pull.state),
                head: pull.head,
                base: pull.base,
                commit: commit(pull.commit),
                base_commit: match pull.base_commit {
                    Some(n) => Some(commit(n)),
                    None => None,
                },
                merged: match pull.merged {
                    Some(n) => Some(commit(n)),
                    None => None,
                },
                reviewers: pull.reviewers,
                mergeable: pull.mergeable,
                ci: combined(&pull.statuses),
            }),
            client::Read::Reviews { page, .. } => {
                let range = page_range(pull.reviews.len(), *page, l.rows);
                let mut rows = List::with_capacity(l.rows);
                for review in pull.reviews.get(range.0..range.1).expect("page range") {
                    rows.push(client::Review {
                        // This fake exposes no operation that edits a submitted review.
                        provenance: client::Provenance::Original,
                        id: review.id,
                        revision: digest(&review.body) ^ review.at.as_nanos(),
                        author: review.author,
                        verdict: verdict(review.verdict),
                        commit: commit(review.commit),
                        key: key_of(&review.body),
                        body: review.body.clone(),
                        at: review.at,
                        official: review.official,
                    })
                    .expect("page capacity");
                }
                client::Answer::Reviews { reviews: rows.into_boxed(), more: range.1 < pull.reviews.len() }
            }
            client::Read::Remarks { .. } => client::Answer::Remarks { remarks: Box::new([]), more: false },
            client::Read::Items { .. }
            | client::Read::Item { .. }
            | client::Read::Statuses { .. }
            | client::Read::Branch { .. }
            | client::Read::PullFiles { .. }
            | client::Read::Compare { .. }
            | client::Read::Checks { .. }
            | client::Read::Job { .. }
            | client::Read::Protection { .. }
            | client::Read::Settings
            | client::Read::Collaborators { .. }
            | client::Read::Permission { .. } => panic!("pull route projection"),
        },
        client::Op::Write(_) => panic!("pull answers a read"),
    }
}
fn page_range(length: usize, page: u32, rows: u32) -> (usize, usize) {
    let rows = usize::try_from(rows).expect("world rows fit");
    let from = usize::try_from(page.saturating_sub(1)).expect("world page fits").saturating_mul(rows).min(length);
    (from, from.saturating_add(rows).min(length))
}
fn statuses_answer(asked: &client::Op, statuses: &[forge::Status], l: &Limits) -> client::Answer {
    let page = match asked {
        client::Op::Read(client::Read::Statuses { page, .. }) => *page,
        client::Op::Read(_) | client::Op::Write(_) => panic!("statuses projection"),
    };
    let range = page_range(statuses.len(), page, l.rows);
    let mut rows = List::with_capacity(l.rows);
    for s in statuses.get(range.0..range.1).expect("status page") {
        rows.push(status(s)).expect("page capacity");
    }
    client::Answer::Statuses { ci: combined(statuses), statuses: rows.into_boxed(), more: range.1 < statuses.len() }
}
fn collaborators_answer(asked: &client::Op, collaborators: &[forge::Collaborator], l: &Limits) -> client::Answer {
    let page = match asked {
        client::Op::Read(client::Read::Collaborators { page }) => *page,
        client::Op::Read(_) | client::Op::Write(_) => panic!("collaborator projection"),
    };
    let range = page_range(collaborators.len(), page, l.rows);
    let mut rows = List::with_capacity(l.rows);
    for c in collaborators.get(range.0..range.1).expect("collaborator page") {
        rows.push(client::Collaborator { user: c.user, permission: permission(c.permission) }).expect("page capacity");
    }
    client::Answer::Collaborators { collaborators: rows.into_boxed(), more: range.1 < collaborators.len() }
}
fn files_in(files: Box<[forge::ChangedFile]>) -> Box<[client::File]> {
    let mut rows = List::with_capacity(u32::try_from(files.len()).expect("bounded fake files"));
    for f in files {
        rows.push(client::File { path: f.path, before: f.before, after: f.after }).expect("files capacity");
    }
    rows.into_boxed()
}
fn permission(p: forge::Permission) -> client::Permission {
    match p {
        forge::Permission::None => client::Permission::None,
        forge::Permission::Read => client::Permission::Read,
        forge::Permission::Write => client::Permission::Write,
        forge::Permission::Admin => client::Permission::Admin,
    }
}
fn verdict(v: forge::Verdict) -> client::Verdict {
    match v {
        forge::Verdict::Approve => client::Verdict::Approve,
        forge::Verdict::RequestChanges => client::Verdict::RequestChanges,
        forge::Verdict::Comment => client::Verdict::Comment,
    }
}
fn verdict_out(v: client::Verdict) -> forge::Verdict {
    match v {
        client::Verdict::Approve => forge::Verdict::Approve,
        client::Verdict::RequestChanges => forge::Verdict::RequestChanges,
        client::Verdict::Comment => forge::Verdict::Comment,
    }
}
fn check(s: forge::Check) -> client::Check {
    match s {
        forge::Check::Pending => client::Check::Pending,
        forge::Check::Passed => client::Check::Passed,
        forge::Check::Failed => client::Check::Failed,
    }
}
fn check_out(s: client::Check) -> forge::Check {
    match s {
        client::Check::Pending => forge::Check::Pending,
        client::Check::Passed => forge::Check::Passed,
        client::Check::Failed => forge::Check::Failed,
    }
}
fn status(s: &forge::Status) -> client::Status {
    client::Status {
        author: s.author,
        at: s.at,
        context: s.context.clone(),
        check: check(s.state),
        description: Box::new([]),
        url: Box::new([]),
        job: None,
    }
}
fn combined(statuses: &[forge::Status]) -> client::Ci {
    if statuses.is_empty() {
        return client::Ci::None;
    }
    let mut pending = false;
    for s in statuses {
        match s.state {
            forge::Check::Failed => return client::Ci::Failed,
            forge::Check::Pending => pending = true,
            forge::Check::Passed => {}
        }
    }
    if pending { client::Ci::Pending } else { client::Ci::Passed }
}
#[must_use]
pub fn digest(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

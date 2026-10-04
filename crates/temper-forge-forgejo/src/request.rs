//! One Forgejo HTTP step at a time; call plans belong to the engine.
use crate::{
    Error, Limits, json, time,
    types::{ReviewState, State},
};
use alloc::boxed::Box;
use skein_json::{Token, writer::Encoder};
use skein_lib::{Decimal, List, Writer, bytes};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Get,
    Post,
    Patch,
    Delete,
}
impl Method {
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Method::Get => b"GET",
            Method::Post => b"POST",
            Method::Patch => b"PATCH",
            Method::Delete => b"DELETE",
        }
    }
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Repository {
    pub owner: Box<[u8]>,
    pub name: Box<[u8]>,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Page {
    pub number: u32,
    pub limit: u32,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Items {
    pub state: Option<State>,
    pub pulls: Option<bool>,
    pub label: Option<Box<[u8]>>,
    pub author: Option<Box<[u8]>>,
    pub since: Option<u64>,
    pub page: Page,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Operation {
    Settings,
    CurrentUser,
    SearchUser { id: u64 },
    Repository,
    Labels { page: Page },
    Items(Items),
    Item { number: u64 },
    Comments { number: u64, since: Option<u64> },
    Comment { id: u64 },
    Pull { number: u64 },
    PullFor { base: Box<[u8]>, head: Box<[u8]> },
    Reviews { number: u64, page: Page },
    Statuses { commit: Box<[u8]>, page: Page },
    Remarks { number: u64, review: u64 },
    Permission { login: Box<[u8]> },
    Branch { name: Box<[u8]> },
    Pages { page: Page },
    Page { name: Box<[u8]> },
    Dependencies { number: u64, page: Page },
    CreateIssue { title: Box<[u8]>, body: Box<[u8]>, labels: Box<[u64]> },
    Post { number: u64, body: Box<[u8]> },
    EditComment { id: u64, body: Box<[u8]> },
    AddLabels { number: u64, labels: Box<[Box<[u8]>]> },
    RemoveLabel { number: u64, label: Box<[u8]> },
    OpenPull { title: Box<[u8]>, body: Box<[u8]>, head: Box<[u8]>, base: Box<[u8]> },
    Merge { number: u64, style: Box<[u8]>, head: Box<[u8]> },
    Review { number: u64, event: ReviewState, body: Box<[u8]> },
    Reviewers { number: u64, remove: bool, reviewers: Box<[Box<[u8]>]> },
    Dependency { number: u64, remove: bool, dependency: u64, owner: Box<[u8]>, repository: Box<[u8]> },
    EditState { number: u64, state: State },
    DeleteBranch { name: Box<[u8]> },
    PutPage { name: Box<[u8]>, create: bool, content_base64: Box<[u8]>, message: Box<[u8]> },
    DeletePage { name: Box<[u8]> },
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Request {
    pub repository: Option<Repository>,
    pub operation: Operation,
}
#[derive(PartialEq, Eq, Debug)]
pub struct Encoded {
    pub method: Method,
    pub target: Box<[u8]>,
    pub body: Option<Box<[u8]>>,
}

#[derive(Debug)]
struct Target {
    length: usize,
    cap: usize,
    writer: Option<Writer>,
    failed: bool,
}
impl Target {
    fn put(&mut self, text: &[u8]) {
        self.length = self.length.saturating_add(text.len());
        if self.length > self.cap {
            self.failed = true;
            return;
        }
        if let Some(writer) = &mut self.writer {
            writer.put(text).expect("target was measured");
        }
    }
    fn name(&mut self, text: &[u8]) {
        for &byte in text {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                self.put(&[byte]);
            } else {
                self.put(&[b'%', hex(byte >> 4), hex(byte & 15)]);
            }
        }
    }
    fn number(&mut self, n: u64) {
        self.put(Decimal::of(n).as_bytes());
    }
    fn index(&mut self, n: u64) {
        self.put(b"/");
        self.number(n);
    }
    fn query(&mut self, key: &[u8], value: &[u8], first: bool) {
        self.put(if first { b"?" } else { b"&" });
        self.put(key);
        self.put(b"=");
        self.name(value);
    }
    fn page(&mut self, page: Page, first: bool) {
        self.query(b"page", Decimal::of(u64::from(page.number)).as_bytes(), first);
        self.query(b"limit", Decimal::of(u64::from(page.limit)).as_bytes(), false);
    }
}
const fn hex(n: u8) -> u8 {
    if n < 10 { b'0'.wrapping_add(n) } else { b'A'.wrapping_add(n.wrapping_sub(10)) }
}
pub fn encode(request: &Request, limits: &Limits) -> Result<Encoded, Error> {
    validate(request, limits)?;
    let cap = usize::try_from(limits.document_bytes).expect("u32 fits usize");
    let mut measure = Target { length: 0, cap, writer: None, failed: false };
    target(&mut measure, request)?;
    if measure.failed {
        return Err(Error::TooLarge);
    }
    let mut write = Target { length: 0, cap, writer: Some(Writer::new(measure.length)), failed: false };
    target(&mut write, request)?;
    let target = write.writer.expect("writing target").finish();
    let (method, has_body) = method(&request.operation);
    let body = if has_body {
        let bounds = skein_json::writer::Limits { depth: limits.depth, length: limits.document_bytes };
        let mut measure = Encoder::measure(&bounds);
        write_body(&mut measure, &request.operation);
        let length = measured(measure)?;
        let mut write = Encoder::write(length, &bounds);
        write_body(&mut write, &request.operation);
        Some(write.finish())
    } else {
        None
    };
    Ok(Encoded { method, target, body })
}
pub(crate) fn measured(out: Encoder) -> Result<u32, Error> {
    match out.measured() {
        Ok(n) => Ok(n),
        Err(skein_json::writer::Refusal::Text) => Err(Error::Text),
        Err(skein_json::writer::Refusal::Number) => Err(Error::Malformed),
        Err(skein_json::writer::Refusal::TooDeep | skein_json::writer::Refusal::TooLong) => Err(Error::TooLarge),
    }
}
fn method(op: &Operation) -> (Method, bool) {
    match op {
        Operation::CreateIssue { .. }
        | Operation::Post { .. }
        | Operation::AddLabels { .. }
        | Operation::OpenPull { .. }
        | Operation::Merge { .. }
        | Operation::Review { .. } => (Method::Post, true),
        Operation::Reviewers { remove, .. } | Operation::Dependency { remove, .. } => {
            (if *remove { Method::Delete } else { Method::Post }, true)
        }
        Operation::EditComment { .. } | Operation::EditState { .. } => (Method::Patch, true),
        Operation::PutPage { create, .. } => (if *create { Method::Post } else { Method::Patch }, true),
        Operation::RemoveLabel { .. } | Operation::DeleteBranch { .. } | Operation::DeletePage { .. } => {
            (Method::Delete, false)
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
        | Operation::Dependencies { .. } => (Method::Get, false),
    }
}
#[expect(clippy::too_many_lines, reason = "exhaustive Forgejo single-step route table")]
fn target(out: &mut Target, request: &Request) -> Result<(), Error> {
    out.put(b"/api/v1");
    match &request.operation {
        Operation::Settings => {
            out.put(b"/settings/api");
            return Ok(());
        }
        Operation::CurrentUser => {
            out.put(b"/user");
            return Ok(());
        }
        Operation::SearchUser { id } => {
            out.put(b"/users/search");
            out.query(b"uid", Decimal::of(*id).as_bytes(), true);
            out.page(Page { number: 1, limit: 1 }, false);
            return Ok(());
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
    let repository = request.repository.as_ref().ok_or(Error::Missing)?;
    out.put(b"/repos/");
    out.name(&repository.owner);
    out.put(b"/");
    out.name(&repository.name);
    match &request.operation {
        Operation::Repository => {}
        Operation::Labels { page } => {
            out.put(b"/labels");
            out.page(*page, true);
        }
        Operation::Items(items) => {
            out.put(b"/issues");
            out.query(
                b"state",
                match items.state {
                    Some(State::Open) => b"open",
                    Some(State::Closed) => b"closed",
                    None => b"all",
                },
                true,
            );
            if let Some(pulls) = items.pulls {
                out.query(b"type", if pulls { b"pulls" } else { b"issues" }, false);
            }
            if let Some(label) = &items.label {
                out.query(b"labels", label, false);
            }
            if let Some(author) = &items.author {
                out.query(b"created_by", author, false);
            }
            if let Some(since) = items.since {
                out.query(b"since", &time::format(since), false);
            }
            out.query(b"sort", b"leastupdate", false);
            out.page(items.page, false);
        }
        Operation::Item { number } | Operation::EditState { number, .. } => {
            out.put(b"/issues");
            out.index(*number);
        }
        Operation::Comments { number, since } => {
            out.put(b"/issues");
            out.index(*number);
            out.put(b"/comments");
            if let Some(since) = since {
                out.query(b"since", &time::format(*since), true);
            }
        }
        Operation::Post { number, .. } => {
            out.put(b"/issues");
            out.index(*number);
            out.put(b"/comments");
        }
        Operation::Comment { id } | Operation::EditComment { id, .. } => {
            out.put(b"/issues/comments");
            out.index(*id);
        }
        Operation::Pull { number } => {
            out.put(b"/pulls");
            out.index(*number);
        }
        Operation::PullFor { base, head } => {
            out.put(b"/pulls/");
            out.name(base);
            out.put(b"/");
            out.name(head);
        }
        Operation::Reviews { number, page } => {
            out.put(b"/pulls");
            out.index(*number);
            out.put(b"/reviews");
            out.page(*page, true);
        }
        Operation::Statuses { commit, page } => {
            out.put(b"/commits/");
            out.name(commit);
            out.put(b"/status");
            out.page(*page, true);
        }
        Operation::Remarks { number, review } => {
            out.put(b"/pulls");
            out.index(*number);
            out.put(b"/reviews");
            out.index(*review);
            out.put(b"/comments");
        }
        Operation::Permission { login } => {
            out.put(b"/collaborators/");
            out.name(login);
            out.put(b"/permission");
        }
        Operation::Branch { name } | Operation::DeleteBranch { name } => {
            out.put(b"/branches/");
            out.name(name);
        }
        Operation::Pages { page } => {
            out.put(b"/wiki/pages");
            out.page(*page, true);
        }
        Operation::Page { name } | Operation::DeletePage { name } => {
            out.put(b"/wiki/page/");
            out.name(name);
        }
        Operation::Dependencies { number, page } => {
            out.put(b"/issues");
            out.index(*number);
            out.put(b"/dependencies");
            out.page(*page, true);
        }
        Operation::CreateIssue { .. } => out.put(b"/issues"),
        Operation::AddLabels { number, .. } => {
            out.put(b"/issues");
            out.index(*number);
            out.put(b"/labels");
        }
        Operation::RemoveLabel { number, label } => {
            out.put(b"/issues");
            out.index(*number);
            out.put(b"/labels/");
            out.name(label);
        }
        Operation::OpenPull { .. } => out.put(b"/pulls"),
        Operation::Merge { number, .. } => {
            out.put(b"/pulls");
            out.index(*number);
            out.put(b"/merge");
        }
        Operation::Review { number, .. } => {
            out.put(b"/pulls");
            out.index(*number);
            out.put(b"/reviews");
        }
        Operation::Reviewers { number, .. } => {
            out.put(b"/pulls");
            out.index(*number);
            out.put(b"/requested_reviewers");
        }
        Operation::Dependency { number, .. } => {
            out.put(b"/issues");
            out.index(*number);
            out.put(b"/dependencies");
        }
        Operation::PutPage { name, create, .. } => {
            if *create {
                out.put(b"/wiki/new");
            } else {
                out.put(b"/wiki/page/");
                out.name(name);
            }
        }
        Operation::Settings | Operation::CurrentUser | Operation::SearchUser { .. } => {
            unreachable!("global routes returned")
        }
    }
    Ok(())
}
fn text(out: &mut Encoder, key: &[u8], value: &[u8]) {
    out.key(key);
    out.string(value);
}
fn strings(out: &mut Encoder, key: &[u8], values: &[Box<[u8]>]) {
    out.key(key);
    out.array_start();
    for value in values {
        out.string(value);
    }
    out.array_end();
}
fn write_body(out: &mut Encoder, op: &Operation) {
    out.object_start();
    match op {
        Operation::CreateIssue { title, body, labels } => {
            text(out, b"title", title);
            text(out, b"body", body);
            out.key(b"labels");
            out.array_start();
            for &id in labels {
                out.unsigned(id);
            }
            out.array_end();
        }
        Operation::Post { body, .. } | Operation::EditComment { body, .. } => text(out, b"body", body),
        Operation::AddLabels { labels, .. } => strings(out, b"labels", labels),
        Operation::OpenPull { title, body, head, base } => {
            text(out, b"title", title);
            text(out, b"body", body);
            text(out, b"head", head);
            text(out, b"base", base);
        }
        Operation::Merge { style, head, .. } => {
            text(out, b"Do", style);
            text(out, b"head_commit_id", head);
        }
        Operation::Review { event, body, .. } => {
            text(out, b"event", review_event(*event));
            text(out, b"body", body);
        }
        Operation::Reviewers { reviewers, .. } => strings(out, b"reviewers", reviewers),
        Operation::Dependency { dependency, owner, repository, .. } => {
            text(out, b"owner", owner);
            text(out, b"repo", repository);
            out.key(b"index");
            out.unsigned(*dependency);
        }
        Operation::EditState { state, .. } => text(
            out,
            b"state",
            match state {
                State::Open => b"open",
                State::Closed => b"closed",
            },
        ),
        Operation::PutPage { name, content_base64, message, .. } => {
            text(out, b"title", name);
            text(out, b"content_base64", content_base64);
            text(out, b"message", message);
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
        | Operation::RemoveLabel { .. }
        | Operation::DeleteBranch { .. }
        | Operation::DeletePage { .. } => {}
    }
    out.object_end();
}
#[must_use]
pub const fn review_event(state: ReviewState) -> &'static [u8] {
    match state {
        ReviewState::Approved => b"APPROVE",
        ReviewState::RequestChanges => b"REQUEST_CHANGES",
        ReviewState::Comment => b"COMMENT",
        ReviewState::Pending => b"PENDING",
        ReviewState::Dismissed => b"DISMISSED",
        ReviewState::Requested => b"REQUEST_REVIEW",
    }
}
fn count(length: usize, cap: u32) -> Result<(), Error> {
    if length > usize::try_from(cap).expect("u32 fits usize") { Err(Error::TooLarge) } else { Ok(()) }
}
#[expect(clippy::disallowed_methods, reason = "the protocol validates Forgejo path text as UTF8")]
fn name(text: &[u8], limits: &Limits) -> Result<(), Error> {
    count(text.len(), limits.name_bytes)?;
    if text.is_empty() || core::str::from_utf8(text).is_err() { Err(Error::Text) } else { Ok(()) }
}
fn page(page: Page, limits: &Limits) -> Result<(), Error> {
    if page.number == 0 || page.limit == 0 || page.limit > limits.page { Err(Error::TooLarge) } else { Ok(()) }
}
fn validate(request: &Request, limits: &Limits) -> Result<(), Error> {
    if !limits.valid() {
        return Err(Error::TooLarge);
    }
    if let Some(repo) = &request.repository {
        name(&repo.owner, limits)?;
        name(&repo.name, limits)?;
    }
    match &request.operation {
        Operation::Labels { page: p }
        | Operation::Reviews { page: p, .. }
        | Operation::Pages { page: p }
        | Operation::Dependencies { page: p, .. } => page(*p, limits)?,
        Operation::Items(items) => {
            page(items.page, limits)?;
            if let Some(label) = &items.label {
                name(label, limits)?;
            }
            if let Some(author) = &items.author {
                name(author, limits)?;
            }
        }
        Operation::Statuses { commit, page: p } => {
            page(*p, limits)?;
            valid_commit(commit)?;
        }
        Operation::PullFor { base, head } => {
            name(base, limits)?;
            name(head, limits)?;
        }
        Operation::Permission { login } => name(login, limits)?,
        Operation::Branch { name: n }
        | Operation::Page { name: n }
        | Operation::DeleteBranch { name: n }
        | Operation::DeletePage { name: n } => name(n, limits)?,
        Operation::CreateIssue { title, body, labels } => {
            count(title.len(), limits.title_bytes)?;
            count(body.len(), limits.body_bytes)?;
            count(labels.len(), limits.fields)?;
        }
        Operation::Post { body, .. } | Operation::EditComment { body, .. } | Operation::Review { body, .. } => {
            count(body.len(), limits.body_bytes)?;
        }
        Operation::AddLabels { labels, .. } | Operation::Reviewers { reviewers: labels, .. } => {
            count(labels.len(), limits.fields)?;
            for label in labels {
                name(label, limits)?;
            }
        }
        Operation::RemoveLabel { label, .. } => name(label, limits)?,
        Operation::OpenPull { title, body, head, base } => {
            count(title.len(), limits.title_bytes)?;
            count(body.len(), limits.body_bytes)?;
            name(head, limits)?;
            name(base, limits)?;
        }
        Operation::Merge { style, head, .. } => {
            name(style, limits)?;
            valid_commit(head)?;
        }
        Operation::Dependency { owner, repository, .. } => {
            name(owner, limits)?;
            name(repository, limits)?;
        }
        Operation::PutPage { name: n, content_base64, message, .. } => {
            name(n, limits)?;
            count(content_base64.len(), limits.document_bytes)?;
            count(message.len(), limits.body_bytes)?;
        }
        Operation::Settings
        | Operation::CurrentUser
        | Operation::SearchUser { .. }
        | Operation::Repository
        | Operation::Item { .. }
        | Operation::Comments { .. }
        | Operation::Comment { .. }
        | Operation::Pull { .. }
        | Operation::Remarks { .. }
        | Operation::EditState { .. } => {}
    }
    Ok(())
}
fn valid_commit(commit: &[u8]) -> Result<(), Error> {
    if commit.len() != 40 && commit.len() != 64 {
        return Err(Error::Malformed);
    }
    for byte in commit {
        if !byte.is_ascii_hexdigit() {
            return Err(Error::Malformed);
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct Decoded {
    pub request: Request,
    /// Forgejo defaults to newest-created first when no recognized sort is given.
    pub least_update: bool,
}
#[derive(Debug)]
struct Query {
    name: Box<[u8]>,
    value: Box<[u8]>,
}
pub fn decode(
    method: Method,
    target: &[u8],
    body: &[u8],
    default_page: u32,
    limits: &Limits,
) -> Result<Decoded, Error> {
    count(target.len(), limits.document_bytes)?;
    let query_at = bytes::find(target, b"?").unwrap_or(target.len());
    let path = target.get(..query_at).ok_or(Error::Malformed)?;
    let query = match target.get(query_at..) {
        Some([b'?', rest @ ..]) => rest,
        Some([]) => b"",
        Some(_) | None => return Err(Error::Malformed),
    };
    let mut parts = List::with_capacity(16);
    for part in split(path, b'/', 16)?.get(1..).ok_or(Error::Malformed)? {
        if parts.push(unescape(part, limits.name_bytes, false)?).is_err() {
            return Err(Error::TooLarge);
        }
    }
    let mut params = List::<Query>::with_capacity(limits.fields);
    if !query.is_empty() {
        for pair in &split(query, b'&', limits.fields)? {
            let at = bytes::find(pair, b"=").ok_or(Error::Malformed)?;
            let name = unescape(pair.get(..at).ok_or(Error::Malformed)?, limits.name_bytes, true)?;
            let value = unescape(
                pair.get(at.checked_add(1).ok_or(Error::TooLarge)?..).ok_or(Error::Malformed)?,
                limits.name_bytes,
                true,
            )?;
            for earlier in &params {
                if earlier.name == name {
                    return Err(Error::Duplicate);
                }
            }
            if params.push(Query { name, value }).is_err() {
                return Err(Error::TooLarge);
            }
        }
    }
    let parts = parts.into_boxed();
    let params = params.into_boxed();
    let value = if body.is_empty() { None } else { Some(json::Json::from_bytes(body, limits)?) };
    let tokens = match &value {
        Some(value) => value.tokens(),
        None => &[],
    };
    let page = read_page(&params, default_page, limits)?;
    let least_update = parameter(&params, Parameter::Sort) == Some(b"leastupdate");
    let (repository, operation) = match parts.as_ref() {
        [api, v1, settings, kind]
            if api.as_ref() == b"api"
                && v1.as_ref() == b"v1"
                && settings.as_ref() == b"settings"
                && kind.as_ref() == b"api"
                && method == Method::Get =>
        {
            (None, Operation::Settings)
        }
        [api, v1, user]
            if api.as_ref() == b"api" && v1.as_ref() == b"v1" && user.as_ref() == b"user" && method == Method::Get =>
        {
            (None, Operation::CurrentUser)
        }
        [api, v1, users, search]
            if api.as_ref() == b"api"
                && v1.as_ref() == b"v1"
                && users.as_ref() == b"users"
                && search.as_ref() == b"search"
                && method == Method::Get =>
        {
            (
                None,
                Operation::SearchUser { id: json::decimal(parameter(&params, Parameter::Uid).ok_or(Error::Missing)?)? },
            )
        }
        [api, v1, repos, owner, repository, tail @ ..]
            if api.as_ref() == b"api" && v1.as_ref() == b"v1" && repos.as_ref() == b"repos" =>
        {
            (
                Some(Repository { owner: owner.clone(), name: repository.clone() }),
                route(method, tail, &params, tokens, page, limits)?,
            )
        }
        _ => return Err(Error::Unsupported),
    };
    let request = Request { repository, operation };
    validate(&request, limits)?;
    Ok(Decoded { request, least_update })
}
#[expect(clippy::disallowed_methods, reason = "the protocol validates Forgejo path text as UTF8")]
fn unescape(input: &[u8], cap: u32, plus: bool) -> Result<Box<[u8]>, Error> {
    let mut out = List::with_capacity(cap);
    let mut skip = 0;
    for (at, &byte) in input.iter().enumerate() {
        if at < skip {
            continue;
        }
        let decoded = if byte == b'%' {
            let pair = input
                .get(at.checked_add(1).ok_or(Error::TooLarge)?..at.checked_add(3).ok_or(Error::TooLarge)?)
                .ok_or(Error::Malformed)?;
            let mut out = [0];
            crate::binary::unhex(pair, &mut out)?;
            skip = at.checked_add(3).ok_or(Error::TooLarge)?;
            let [byte] = out;
            byte
        } else if plus && byte == b'+' {
            b' '
        } else {
            byte
        };
        if out.push(decoded).is_err() {
            return Err(Error::TooLarge);
        }
    }
    let out = out.into_boxed();
    if core::str::from_utf8(&out).is_err() {
        return Err(Error::Text);
    }
    Ok(out)
}
fn parameter(params: &[Query], key: Parameter) -> Option<&[u8]> {
    // No stored borrow: queries are owned only for this bounded request parse.
    for param in params {
        if param.name.as_ref() == key.name() {
            return Some(&param.value);
        }
    }
    None
}
fn read_page(params: &[Query], default: u32, limits: &Limits) -> Result<Page, Error> {
    let number = match parameter(params, Parameter::Page) {
        Some(n) => as_u32(json::decimal(n)?)?,
        None => 1,
    };
    let limit = if parameter(params, Parameter::Page).is_none() {
        default
    } else {
        match parameter(params, Parameter::Limit) {
            Some(n) => as_u32(json::decimal(n)?)?,
            None => default,
        }
    };
    Ok(Page { number: number.max(1), limit: limit.max(1).min(limits.page) })
}
fn string_field(tokens: &[Token], key: &[u8]) -> Result<Box<[u8]>, Error> {
    Ok(bytes::copy_of(json::text(json::value(tokens, json::required(tokens, key)?)?)?))
}
fn strings_field(tokens: &[Token], key: &[u8], limits: &Limits) -> Result<Box<[Box<[u8]>]>, Error> {
    let values = json::value(tokens, json::required(tokens, key)?)?;
    let mut out = List::with_capacity(limits.fields);
    for &at in &json::array(values, limits.fields)? {
        out.push(bytes::copy_of(json::text(json::value(values, at)?)?)).expect("array offsets bounded");
    }
    Ok(out.into_boxed())
}
fn number_field(tokens: &[Token], key: &[u8]) -> Result<u64, Error> {
    json::unsigned(json::value(tokens, json::required(tokens, key)?)?)
}
#[expect(clippy::too_many_lines, reason = "exhaustive Forgejo single-step route table")]
fn route(
    method: Method,
    tail: &[Box<[u8]>],
    params: &[Query],
    tokens: &[Token],
    page: Page,
    limits: &Limits,
) -> Result<Operation, Error> {
    match tail {
        [] if method == Method::Get => Ok(Operation::Repository),
        [labels] if labels.as_ref() == b"labels" && method == Method::Get => Ok(Operation::Labels { page }),
        [issues] if issues.as_ref() == b"issues" => match method {
            Method::Get => Ok(Operation::Items(Items {
                state: match parameter(params, Parameter::State) {
                    Some(b"open") => Some(State::Open),
                    Some(b"closed") => Some(State::Closed),
                    Some(b"all") | None => None,
                    Some(_) => return Err(Error::Malformed),
                },
                pulls: match parameter(params, Parameter::Type) {
                    Some(b"pulls") => Some(true),
                    Some(b"issues") => Some(false),
                    Some(_) => return Err(Error::Malformed),
                    None => None,
                },
                label: parameter_owned(params, Parameter::Labels),
                author: parameter_owned(params, Parameter::CreatedBy),
                since: since(params)?,
                page,
            })),
            Method::Post => {
                let values = json::value(tokens, json::required(tokens, b"labels")?)?;
                let mut labels = List::with_capacity(limits.fields);
                for &at in &json::array(values, limits.fields)? {
                    labels.push(json::unsigned(json::value(values, at)?)?).expect("bounded array");
                }
                Ok(Operation::CreateIssue {
                    title: string_field(tokens, b"title")?,
                    body: string_field(tokens, b"body")?,
                    labels: labels.into_boxed(),
                })
            }
            Method::Patch | Method::Delete => Err(Error::Unsupported),
        },
        [issues, n] if issues.as_ref() == b"issues" => {
            let number = json::decimal(n)?;
            match method {
                Method::Get => Ok(Operation::Item { number }),
                Method::Patch => Ok(Operation::EditState { number, state: state(&string_field(tokens, b"state")?)? }),
                Method::Post | Method::Delete => Err(Error::Unsupported),
            }
        }
        [issues, comments, id] if issues.as_ref() == b"issues" && comments.as_ref() == b"comments" => {
            let id = json::decimal(id)?;
            match method {
                Method::Get => Ok(Operation::Comment { id }),
                Method::Patch => Ok(Operation::EditComment { id, body: string_field(tokens, b"body")? }),
                Method::Post | Method::Delete => Err(Error::Unsupported),
            }
        }
        [issues, n, comments] if issues.as_ref() == b"issues" && comments.as_ref() == b"comments" => {
            let number = json::decimal(n)?;
            match method {
                Method::Get => Ok(Operation::Comments { number, since: since(params)? }),
                Method::Post => Ok(Operation::Post { number, body: string_field(tokens, b"body")? }),
                Method::Patch | Method::Delete => Err(Error::Unsupported),
            }
        }
        [issues, n, labels]
            if issues.as_ref() == b"issues" && labels.as_ref() == b"labels" && method == Method::Post =>
        {
            Ok(Operation::AddLabels { number: json::decimal(n)?, labels: strings_field(tokens, b"labels", limits)? })
        }
        [issues, n, labels, label]
            if issues.as_ref() == b"issues" && labels.as_ref() == b"labels" && method == Method::Delete =>
        {
            Ok(Operation::RemoveLabel { number: json::decimal(n)?, label: label.clone() })
        }
        [issues, n, dependencies] if issues.as_ref() == b"issues" && dependencies.as_ref() == b"dependencies" => {
            let number = json::decimal(n)?;
            match method {
                Method::Get => Ok(Operation::Dependencies { number, page }),
                Method::Post | Method::Delete => Ok(Operation::Dependency {
                    number,
                    remove: method == Method::Delete,
                    dependency: number_field(tokens, b"index")?,
                    owner: string_field(tokens, b"owner")?,
                    repository: string_field(tokens, b"repo")?,
                }),
                Method::Patch => Err(Error::Unsupported),
            }
        }
        [pulls] if pulls.as_ref() == b"pulls" && method == Method::Post => Ok(Operation::OpenPull {
            title: string_field(tokens, b"title")?,
            body: string_field(tokens, b"body")?,
            head: string_field(tokens, b"head")?,
            base: string_field(tokens, b"base")?,
        }),
        [pulls, n] if pulls.as_ref() == b"pulls" && method == Method::Get => {
            Ok(Operation::Pull { number: json::decimal(n)? })
        }
        [pulls, n, merge] if pulls.as_ref() == b"pulls" && merge.as_ref() == b"merge" && method == Method::Post => {
            Ok(Operation::Merge {
                number: json::decimal(n)?,
                style: string_field(tokens, b"Do")?,
                head: string_field(tokens, b"head_commit_id")?,
            })
        }
        [pulls, n, reviews] if pulls.as_ref() == b"pulls" && reviews.as_ref() == b"reviews" => {
            let number = json::decimal(n)?;
            match method {
                Method::Get => Ok(Operation::Reviews { number, page }),
                Method::Post => Ok(Operation::Review {
                    number,
                    event: review_state(&string_field(tokens, b"event")?)?,
                    body: string_field(tokens, b"body")?,
                }),
                Method::Patch | Method::Delete => Err(Error::Unsupported),
            }
        }
        [pulls, n, reviewers]
            if pulls.as_ref() == b"pulls"
                && reviewers.as_ref() == b"requested_reviewers"
                && (method == Method::Post || method == Method::Delete) =>
        {
            Ok(Operation::Reviewers {
                number: json::decimal(n)?,
                remove: method == Method::Delete,
                reviewers: strings_field(tokens, b"reviewers", limits)?,
            })
        }
        [pulls, n, reviews, id, comments]
            if pulls.as_ref() == b"pulls"
                && reviews.as_ref() == b"reviews"
                && comments.as_ref() == b"comments"
                && method == Method::Get =>
        {
            Ok(Operation::Remarks { number: json::decimal(n)?, review: json::decimal(id)? })
        }
        [pulls, base, head] if pulls.as_ref() == b"pulls" && method == Method::Get => {
            Ok(Operation::PullFor { base: base.clone(), head: head.clone() })
        }
        [commits, commit, status]
            if commits.as_ref() == b"commits" && status.as_ref() == b"status" && method == Method::Get =>
        {
            Ok(Operation::Statuses { commit: commit.clone(), page })
        }
        [collaborators, login, permission]
            if collaborators.as_ref() == b"collaborators"
                && permission.as_ref() == b"permission"
                && method == Method::Get =>
        {
            Ok(Operation::Permission { login: login.clone() })
        }
        [branches, name] if branches.as_ref() == b"branches" => match method {
            Method::Get => Ok(Operation::Branch { name: name.clone() }),
            Method::Delete => Ok(Operation::DeleteBranch { name: name.clone() }),
            Method::Post | Method::Patch => Err(Error::Unsupported),
        },
        [wiki, pages] if wiki.as_ref() == b"wiki" && pages.as_ref() == b"pages" && method == Method::Get => {
            Ok(Operation::Pages { page })
        }
        [wiki, new] if wiki.as_ref() == b"wiki" && new.as_ref() == b"new" && method == Method::Post => {
            Ok(Operation::PutPage {
                name: string_field(tokens, b"title")?,
                create: true,
                content_base64: string_field(tokens, b"content_base64")?,
                message: string_field(tokens, b"message")?,
            })
        }
        [wiki, page, name] if wiki.as_ref() == b"wiki" && page.as_ref() == b"page" => match method {
            Method::Get => Ok(Operation::Page { name: name.clone() }),
            Method::Delete => Ok(Operation::DeletePage { name: name.clone() }),
            Method::Patch => Ok(Operation::PutPage {
                name: name.clone(),
                create: false,
                content_base64: string_field(tokens, b"content_base64")?,
                message: string_field(tokens, b"message")?,
            }),
            Method::Post => Err(Error::Unsupported),
        },
        _ => Err(Error::Unsupported),
    }
}
#[expect(clippy::manual_map, reason = "the programming subset uses exhaustive matches")]
fn parameter_owned(params: &[Query], key: Parameter) -> Option<Box<[u8]>> {
    match parameter(params, key) {
        Some(value) => Some(bytes::copy_of(value)),
        None => None,
    }
}
fn since(params: &[Query]) -> Result<Option<u64>, Error> {
    match parameter(params, Parameter::Since) {
        Some(value) => Ok(Some(time::parse(value)?)),
        None => Ok(None),
    }
}
fn state(text: &[u8]) -> Result<State, Error> {
    match text {
        b"open" => Ok(State::Open),
        b"closed" => Ok(State::Closed),
        _ => Err(Error::Malformed),
    }
}
fn review_state(text: &[u8]) -> Result<ReviewState, Error> {
    match text {
        b"APPROVE" => Ok(ReviewState::Approved),
        b"REQUEST_CHANGES" => Ok(ReviewState::RequestChanges),
        b"COMMENT" => Ok(ReviewState::Comment),
        b"PENDING" => Ok(ReviewState::Pending),
        _ => Err(Error::Unsupported),
    }
}

#[derive(Clone, Copy, Debug)]
enum Parameter {
    Page,
    Limit,
    Sort,
    State,
    Type,
    Labels,
    CreatedBy,
    Since,
    Uid,
}
impl Parameter {
    const fn name(self) -> &'static [u8] {
        match self {
            Self::Page => b"page",
            Self::Limit => b"limit",
            Self::Sort => b"sort",
            Self::State => b"state",
            Self::Type => b"type",
            Self::Labels => b"labels",
            Self::CreatedBy => b"created_by",
            Self::Since => b"since",
            Self::Uid => b"uid",
        }
    }
}
fn as_u32(n: u64) -> Result<u32, Error> {
    match u32::try_from(n) {
        Ok(n) => Ok(n),
        Err(_) => Err(Error::TooLarge),
    }
}
fn split(input: &[u8], delimiter: u8, cap: u32) -> Result<Box<[Box<[u8]>]>, Error> {
    let mut out = List::with_capacity(cap);
    let mut start = 0;
    for (at, &byte) in input.iter().enumerate() {
        if byte == delimiter {
            if out.push(bytes::copy_of(input.get(start..at).expect("bounded range"))).is_err() {
                return Err(Error::TooLarge);
            }
            start = at.checked_add(1).ok_or(Error::TooLarge)?;
        }
    }
    if out.push(bytes::copy_of(input.get(start..).expect("bounded range"))).is_err() {
        return Err(Error::TooLarge);
    }
    Ok(out.into_boxed())
}

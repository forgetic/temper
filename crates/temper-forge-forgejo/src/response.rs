//! Selective, bounded Forgejo response decoding.
use crate::{
    Error, Limits, binary, json, request, time,
    types::{self, Document, ObjectFormat},
};
use alloc::boxed::Box;
use skein_json::{Token, writer::Encoder};
use skein_lib::{List, bytes};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Settings,
    User,
    Users,
    Repository,
    Labels,
    Item,
    Items,
    Comment,
    Comments,
    Pull,
    Review,
    Reviews,
    Statuses,
    Remarks,
    Permission,
    Branch,
    Pages,
    Page,
    Dependencies,
    Error,
    Done,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub after: u64,
    /// Offset slice of an unpaged response; ordinary paged responses use 1.
    pub page: u32,
    pub limit: u32,
}
impl Selection {
    #[must_use]
    pub const fn first(limit: u32) -> Selection {
        Selection { after: 0, page: 1, limit }
    }
}
#[derive(Debug)]
enum Row {
    User(types::User),
    Label(types::Label),
    Item(types::Item),
    Comment(types::Comment),
    Pull(types::Pull),
    Review(types::Review),
    Status(types::Status),
    Remark(types::Remark),
    Page(types::WikiPage),
    Dependency(types::Dependency),
    Other(Document),
}

/// Holds one row's selected tokens and at most the requested page of typed
/// rows. Unknown fields are discarded as their tokens arrive. This layer does
/// not solve the tokenizer's whole-string limit; live history execution awaits
/// upstream string pieces and value skipping.
#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent JSON root, skip and array-seen parser flags")]
pub struct Decoder {
    kind: Kind,
    format: ObjectFormat,
    selection: Selection,
    limits: Limits,
    rows: List<Row>,
    row: List<Token>,
    row_bytes: u32,
    depth: u32,
    row_depth: Option<u32>,
    ignored: u32,
    skip_value: bool,
    root: bool,
    whole: bool,
    seen: u64,
    total_count: Option<u64>,
    state: Option<types::Check>,
    meta_key: Option<Box<[u8]>>,
    row_array: Option<u32>,
    array_seen: bool,
}
impl Decoder {
    pub fn new(kind: Kind, format: ObjectFormat, selection: Selection, limits: &Limits) -> Result<Decoder, Error> {
        if !limits.valid() || selection.page == 0 || selection.limit == 0 || selection.limit > limits.page {
            return Err(Error::TooLarge);
        }
        Ok(Decoder {
            kind,
            format,
            selection,
            limits: *limits,
            rows: List::with_capacity(selection.limit),
            row: List::with_capacity(limits.tokens),
            row_bytes: 0,
            depth: 0,
            row_depth: None,
            ignored: 0,
            skip_value: false,
            root: false,
            whole: false,
            seen: 0,
            total_count: None,
            state: None,
            meta_key: None,
            row_array: None,
            array_seen: false,
        })
    }
    #[expect(clippy::too_many_lines, reason = "one token advances bounded root, array, row and depth states")]
    pub fn token(&mut self, token: Token) -> Result<(), Error> {
        if self.whole {
            return Err(Error::Malformed);
        }
        let opening = match token {
            Token::ObjectStart | Token::ArrayStart => true,
            Token::ObjectEnd
            | Token::ArrayEnd
            | Token::Key(_)
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null => false,
        };
        let closing = match token {
            Token::ObjectEnd | Token::ArrayEnd => true,
            Token::ObjectStart
            | Token::ArrayStart
            | Token::Key(_)
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null => false,
        };
        if opening {
            self.depth = self.depth.checked_add(1).ok_or(Error::TooLarge)?;
            if self.depth > self.limits.depth {
                return Err(Error::TooLarge);
            }
        }
        if !self.root {
            self.root = true;
            let expected = if direct_list(self.kind) { Token::ArrayStart } else { Token::ObjectStart };
            if token != expected {
                return Err(Error::Malformed);
            }
            if !direct_list(self.kind) && !wrapper(self.kind) {
                self.row_depth = Some(1);
            } else if direct_list(self.kind) {
                self.row_array = Some(1);
                self.array_seen = true;
            }
        } else if self.row_depth.is_none() && token == Token::ArrayStart && self.depth == 2 && wrapper(self.kind) {
            let key = match self.kind {
                Kind::Users => b"data".as_slice(),
                Kind::Statuses => b"statuses".as_slice(),
                Kind::Settings
                | Kind::User
                | Kind::Repository
                | Kind::Labels
                | Kind::Item
                | Kind::Items
                | Kind::Comment
                | Kind::Comments
                | Kind::Pull
                | Kind::Review
                | Kind::Reviews
                | Kind::Remarks
                | Kind::Permission
                | Kind::Branch
                | Kind::Pages
                | Kind::Page
                | Kind::Dependencies
                | Kind::Error
                | Kind::Done => unreachable!("wrapper kind"),
            };
            if self.meta_key.as_deref() == Some(key) {
                if self.array_seen {
                    return Err(Error::Duplicate);
                }
                self.row_array = Some(2);
                self.array_seen = true;
            }
        } else if self.row_depth.is_none() && token == Token::ObjectStart && self.row_array == self.depth.checked_sub(1)
        {
            self.row_depth = Some(self.depth);
        }
        if let Some(row_depth) = self.row_depth {
            self.capture(token)?;
            if closing && self.depth == row_depth {
                let row = decode_row(self.kind, self.row.as_slice(), self.format, &self.limits)?;
                self.row.clear();
                self.row_bytes = 0;
                self.row_depth = None;
                self.ignored = 0;
                self.skip_value = false;
                let keep = match &row {
                    Row::Comment(comment) => comment.id > self.selection.after,
                    Row::Review(review) => {
                        self.kind == Kind::Review
                            || match review.state {
                                types::ReviewState::Pending | types::ReviewState::Requested => false,
                                types::ReviewState::Approved
                                | types::ReviewState::RequestChanges
                                | types::ReviewState::Comment
                                | types::ReviewState::Dismissed => true,
                            }
                    }
                    Row::User(_)
                    | Row::Label(_)
                    | Row::Item(_)
                    | Row::Pull(_)
                    | Row::Status(_)
                    | Row::Remark(_)
                    | Row::Page(_)
                    | Row::Dependency(_)
                    | Row::Other(_) => true,
                };
                if keep {
                    let offset = u64::from(self.selection.page.checked_sub(1).expect("page nonzero"))
                        .checked_mul(u64::from(self.selection.limit))
                        .ok_or(Error::TooLarge)?;
                    if self.seen >= offset && self.rows.len() < self.selection.limit {
                        self.rows.push(row).expect("page has room");
                    }
                    self.seen = self.seen.checked_add(1).ok_or(Error::TooLarge)?;
                }
            }
        } else {
            self.meta(token)?;
        }
        if closing {
            if self.row_array == Some(self.depth) {
                self.row_array = None;
            }
            self.depth = self.depth.checked_sub(1).ok_or(Error::Malformed)?;
            if self.depth == 0 {
                self.whole = true;
            }
        }
        Ok(())
    }
    fn capture(&mut self, token: Token) -> Result<(), Error> {
        if self.ignored > 0 {
            match token {
                Token::ObjectStart | Token::ArrayStart => {
                    self.ignored = self.ignored.checked_add(1).ok_or(Error::TooLarge)?;
                }
                Token::ObjectEnd | Token::ArrayEnd => {
                    self.ignored = self.ignored.checked_sub(1).expect("ignored nesting nonzero");
                }
                Token::Key(_) | Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null => {}
            }
            return Ok(());
        }
        if self.skip_value {
            self.skip_value = false;
            match token {
                Token::ObjectStart | Token::ArrayStart => self.ignored = 1,
                Token::ObjectEnd | Token::ArrayEnd | Token::Key(_) => return Err(Error::Malformed),
                Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null => {}
            }
            return Ok(());
        }
        match &token {
            Token::Key(key) => {
                if !known(key) {
                    self.skip_value = true;
                    return Ok(());
                }
            }
            Token::ObjectStart
            | Token::ObjectEnd
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null => {}
        }
        let length = match &token {
            Token::Key(text) | Token::String(text) | Token::Number(text) => {
                u32::try_from(text.len()).unwrap_or(u32::MAX)
            }
            Token::ObjectStart
            | Token::ObjectEnd
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::True
            | Token::False
            | Token::Null => 0,
        };
        self.row_bytes = self.row_bytes.checked_add(length).ok_or(Error::TooLarge)?;
        if self.row_bytes > self.limits.document_bytes || self.row.push(token).is_err() {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
    fn meta(&mut self, token: Token) -> Result<(), Error> {
        match token {
            Token::Key(key) if self.depth == 1 => self.meta_key = Some(key),
            Token::String(text) if self.depth == 1 => {
                if self.meta_key.as_deref() == Some(b"state") {
                    if self.state.is_some() {
                        return Err(Error::Duplicate);
                    }
                    self.state = Some(check(&text)?);
                }
                self.meta_key = None;
            }
            Token::Number(text) if self.depth == 1 => {
                if self.meta_key.as_deref() == Some(b"total_count") {
                    if self.total_count.is_some() {
                        return Err(Error::Duplicate);
                    }
                    self.total_count = Some(json::decimal(&text)?);
                }
                self.meta_key = None;
            }
            Token::ObjectStart
            | Token::ObjectEnd
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::Key(_)
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null => {}
        }
        Ok(())
    }
    pub fn finish(self) -> Result<Document, Error> {
        if !self.whole || self.depth != 0 || self.row_depth.is_some() {
            return Err(Error::Malformed);
        }
        if (wrapper(self.kind) || direct_list(self.kind)) && !self.array_seen {
            return Err(Error::Missing);
        }
        assemble(self.kind, self.rows.into_boxed(), self.state, self.total_count)
    }
    #[must_use]
    pub const fn rows_seen(&self) -> u64 {
        self.seen
    }
}

/// Retained rows, one selected-token row, and transient row decoding/assembly.
/// String storage is capped in aggregate per row. Nested arrays each hold at
/// most `fields`; the sum of all record sizes conservatively covers any shape.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !limits.valid() {
        return None;
    }
    let nested = size_of::<types::User>()
        .checked_add(size_of::<types::Label>())?
        .checked_add(size_of::<types::Review>())?
        .checked_add(size_of::<types::Status>())?;
    let structure = u64::try_from(nested).ok()?.checked_mul(u64::from(limits.fields))?;
    let payload = u64::from(limits.document_bytes).checked_mul(2)?.checked_add(structure)?;
    List::<Row>::worst_case(limits.page)?
        .checked_add(List::<Token>::worst_case(limits.tokens)?)?
        .checked_add(u64::from(limits.document_bytes))?
        .checked_add(payload.checked_mul(u64::from(limits.page).checked_add(2)?)?)
}
const fn direct_list(kind: Kind) -> bool {
    match kind {
        Kind::Labels
        | Kind::Items
        | Kind::Comments
        | Kind::Reviews
        | Kind::Remarks
        | Kind::Pages
        | Kind::Dependencies => true,
        Kind::Settings
        | Kind::User
        | Kind::Users
        | Kind::Repository
        | Kind::Item
        | Kind::Comment
        | Kind::Pull
        | Kind::Review
        | Kind::Statuses
        | Kind::Permission
        | Kind::Branch
        | Kind::Page
        | Kind::Error
        | Kind::Done => false,
    }
}
fn wrapper(kind: Kind) -> bool {
    kind == Kind::Users || kind == Kind::Statuses
}
fn known(key: &[u8]) -> bool {
    matches_key(key)
}
#[expect(clippy::match_like_matches_macro, reason = "the programming subset uses exhaustive matches")]
fn matches_key(key: &[u8]) -> bool {
    match key {
        b"id"
        | b"login"
        | b"name"
        | b"full_name"
        | b"object_format_name"
        | b"has_wiki"
        | b"default_branch"
        | b"number"
        | b"state"
        | b"status"
        | b"user"
        | b"title"
        | b"body"
        | b"labels"
        | b"created_at"
        | b"updated_at"
        | b"pull_request"
        | b"issue_url"
        | b"head"
        | b"base"
        | b"ref"
        | b"sha"
        | b"merged"
        | b"merge_commit_sha"
        | b"mergeable"
        | b"requested_reviewers"
        | b"commit_id"
        | b"submitted_at"
        | b"official"
        | b"dismissed"
        | b"context"
        | b"creator"
        | b"description"
        | b"target_url"
        | b"path"
        | b"diff_hunk"
        | b"position"
        | b"original_commit_id"
        | b"content_base64"
        | b"last_commit"
        | b"index"
        | b"owner"
        | b"repo"
        | b"permission"
        | b"commit"
        | b"message"
        | b"max_response_items"
        | b"default_paging_num" => true,
        _ => false,
    }
}
pub fn from_bytes(
    input: &[u8],
    kind: Kind,
    format: ObjectFormat,
    selection: Selection,
    limits: &Limits,
) -> Result<Document, Error> {
    // A bounded convenience for fixtures and finite documents, not histories.
    let value = json::Json::from_bytes(input, limits)?;
    let mut decoder = Decoder::new(kind, format, selection, limits)?;
    for token in value.tokens() {
        decoder.token(token.clone())?;
    }
    decoder.finish()
}
fn required(tokens: &[Token], key: &[u8]) -> Result<usize, Error> {
    json::required(tokens, key)
}
fn text(tokens: &[Token], key: &[u8]) -> Result<Box<[u8]>, Error> {
    Ok(bytes::copy_of(json::text(json::value(tokens, required(tokens, key)?)?)?))
}
fn optional_text(tokens: &[Token], key: &[u8]) -> Result<Option<Box<[u8]>>, Error> {
    match json::field(tokens, key)? {
        Some(at) => match json::value(tokens, at)? {
            [Token::Null] => Ok(None),
            value => Ok(Some(bytes::copy_of(json::text(value)?))),
        },
        None => Ok(None),
    }
}
fn number(tokens: &[Token], key: &[u8]) -> Result<u64, Error> {
    json::unsigned(json::value(tokens, required(tokens, key)?)?)
}
fn boolean(tokens: &[Token], key: &[u8]) -> Result<bool, Error> {
    json::boolean(json::value(tokens, required(tokens, key)?)?)
}
fn named_object(tokens: &[Token], key: &[u8]) -> Result<usize, Error> {
    required(tokens, key)
}
fn user(tokens: &[Token], limits: &Limits) -> Result<types::User, Error> {
    Ok(types::User { id: number(tokens, b"id")?, login: bounded(text(tokens, b"login")?, limits.name_bytes)? })
}
fn nested_user(tokens: &[Token], key: &[u8], limits: &Limits) -> Result<types::User, Error> {
    user(json::value(tokens, named_object(tokens, key)?)?, limits)
}
fn timestamp(tokens: &[Token], key: &[u8]) -> Result<u64, Error> {
    time::parse(&text(tokens, key)?)
}
fn bounded(text: Box<[u8]>, cap: u32) -> Result<Box<[u8]>, Error> {
    if text.len() > usize::try_from(cap).expect("u32 fits usize") { Err(Error::TooLarge) } else { Ok(text) }
}
fn cut(text: Box<[u8]>, cap: u32) -> Box<[u8]> {
    if text.len() <= usize::try_from(cap).expect("u32 fits usize") {
        return text;
    }
    let mut end = text.len().min(usize::try_from(cap).expect("u32 fits usize"));
    for _ in 0_u32..4 {
        match text.get(end) {
            Some(byte) if byte & 0xC0 == 0x80 => end = end.checked_sub(1).expect("continuation has leading byte"),
            Some(_) | None => break,
        }
    }
    bytes::copy_of(text.get(..end).expect("prefix bounded"))
}
fn id(tokens: &[Token], key: &[u8], format: ObjectFormat) -> Result<[u8; 32], Error> {
    binary::commit(&text(tokens, key)?, format)
}
fn state(text: &[u8]) -> Result<types::State, Error> {
    match text {
        b"open" => Ok(types::State::Open),
        b"closed" => Ok(types::State::Closed),
        _ => Err(Error::Malformed),
    }
}
fn check(text: &[u8]) -> Result<types::Check, Error> {
    match text {
        b"pending" => Ok(types::Check::Pending),
        b"success" => Ok(types::Check::Success),
        b"error" => Ok(types::Check::Error),
        b"failure" => Ok(types::Check::Failure),
        b"warning" => Ok(types::Check::Warning),
        _ => Err(Error::Malformed),
    }
}
fn review_state(text: &[u8]) -> Result<types::ReviewState, Error> {
    match text {
        b"APPROVED" => Ok(types::ReviewState::Approved),
        b"REQUEST_CHANGES" => Ok(types::ReviewState::RequestChanges),
        b"COMMENT" => Ok(types::ReviewState::Comment),
        b"PENDING" => Ok(types::ReviewState::Pending),
        b"DISMISSED" => Ok(types::ReviewState::Dismissed),
        b"REQUEST_REVIEW" => Ok(types::ReviewState::Requested),
        _ => Err(Error::Malformed),
    }
}
#[expect(clippy::too_many_lines, reason = "exhaustive typed Forgejo document table")]
fn decode_row(kind: Kind, tokens: &[Token], format: ObjectFormat, limits: &Limits) -> Result<Row, Error> {
    match kind {
        Kind::User | Kind::Users => Ok(Row::User(user(tokens, limits)?)),
        Kind::Labels => Ok(Row::Label(types::Label {
            id: number(tokens, b"id")?,
            name: bounded(text(tokens, b"name")?, limits.name_bytes)?,
        })),
        Kind::Items | Kind::Item => {
            let mut labels = List::with_capacity(limits.fields);
            let values = json::value(tokens, required(tokens, b"labels")?)?;
            for &at in &json::array(values, limits.fields)? {
                let label = json::value(values, at)?;
                labels
                    .push(types::Label {
                        id: number(label, b"id")?,
                        name: bounded(text(label, b"name")?, limits.name_bytes)?,
                    })
                    .expect("bounded labels");
            }
            let body =
                cut(text(tokens, b"body")?, if kind == Kind::Items { limits.marker_bytes } else { limits.body_bytes });
            let title =
                if kind == Kind::Items { Box::from([]) } else { cut(text(tokens, b"title")?, limits.title_bytes) };
            let pull = match json::field(tokens, b"pull_request")? {
                Some(at) => json::value(tokens, at)? != [Token::Null],
                None => false,
            };
            Ok(Row::Item(types::Item {
                number: number(tokens, b"number")?,
                kind: if pull { types::ItemKind::Pull } else { types::ItemKind::Issue },
                state: state(&text(tokens, b"state")?)?,
                user: nested_user(tokens, b"user", limits)?,
                title,
                body,
                labels: labels.into_boxed(),
                created: timestamp(tokens, b"created_at")?,
                updated: timestamp(tokens, b"updated_at")?,
            }))
        }
        Kind::Comment | Kind::Comments => {
            let body = text(tokens, b"body")?;
            let digest = binary::digest(&body);
            let revision = u64::from_be_bytes(digest.get(..8).expect("digest prefix").try_into().expect("eight bytes"));
            Ok(Row::Comment(types::Comment {
                id: number(tokens, b"id")?,
                user: nested_user(tokens, b"user", limits)?,
                body: cut(body, limits.body_bytes),
                issue_url: bounded(text(tokens, b"issue_url")?, limits.name_bytes)?,
                created: timestamp(tokens, b"created_at")?,
                updated: timestamp(tokens, b"updated_at")?,
                revision,
            }))
        }
        Kind::Pull => {
            let head = json::value(tokens, required(tokens, b"head")?)?;
            let base = json::value(tokens, required(tokens, b"base")?)?;
            let mut reviewers = List::with_capacity(limits.fields);
            if let Some(at) = json::field(tokens, b"requested_reviewers")? {
                let values = json::value(tokens, at)?;
                if values != [Token::Null] {
                    for &at in &json::array(values, limits.fields)? {
                        reviewers.push(user(json::value(values, at)?, limits)?).expect("bounded reviewers");
                    }
                }
            }
            let merged = boolean(tokens, b"merged")?;
            let merge_commit = match optional_text(tokens, b"merge_commit_sha")? {
                Some(text) if !text.is_empty() => Some(binary::commit(&text, format)?),
                Some(_) | None => None,
            };
            Ok(Row::Pull(types::Pull {
                number: number(tokens, b"number")?,
                state: state(&text(tokens, b"state")?)?,
                head: bounded(text(head, b"ref")?, limits.name_bytes)?,
                base: bounded(text(base, b"ref")?, limits.name_bytes)?,
                commit: id(head, b"sha", format)?,
                base_commit: id(base, b"sha", format)?,
                merged,
                merge_commit,
                mergeable: boolean(tokens, b"mergeable")?,
                reviewers: reviewers.into_boxed(),
            }))
        }
        Kind::Review | Kind::Reviews => {
            let state = review_state(&text(tokens, b"state")?)?;
            let submitted = match state {
                types::ReviewState::Pending | types::ReviewState::Requested => 0,
                types::ReviewState::Approved
                | types::ReviewState::RequestChanges
                | types::ReviewState::Comment
                | types::ReviewState::Dismissed => timestamp(tokens, b"submitted_at")?,
            };
            Ok(Row::Review(types::Review {
                id: number(tokens, b"id")?,
                user: nested_user(tokens, b"user", limits)?,
                state,
                commit: id(tokens, b"commit_id", format)?,
                body: cut(text(tokens, b"body")?, limits.body_bytes),
                submitted,
                official: boolean(tokens, b"official")?,
                dismissed: match json::field(tokens, b"dismissed")? {
                    Some(at) => json::boolean(json::value(tokens, at)?)?,
                    None => false,
                },
            }))
        }
        Kind::Statuses => Ok(Row::Status(types::Status {
            context: bounded(text(tokens, b"context")?, limits.name_bytes)?,
            state: check(&text(tokens, b"status")?)?,
            creator: nested_user(tokens, b"creator", limits)?,
            description: cut(text(tokens, b"description")?, limits.body_bytes),
            target_url: cut(text(tokens, b"target_url")?, limits.body_bytes),
            created: timestamp(tokens, b"created_at")?,
        })),
        Kind::Remarks => Ok(Row::Remark(types::Remark {
            id: number(tokens, b"id")?,
            user: nested_user(tokens, b"user", limits)?,
            body: cut(text(tokens, b"body")?, limits.body_bytes),
            path: bounded(text(tokens, b"path")?, limits.name_bytes)?,
            diff_hunk: cut(text(tokens, b"diff_hunk")?, limits.body_bytes),
            position: number(tokens, b"position")?,
            commit: id(tokens, b"original_commit_id", format)?,
        })),
        Kind::Page | Kind::Pages => {
            let commit = json::value(tokens, required(tokens, b"last_commit")?)?;
            let content = match optional_text(tokens, b"content_base64")? {
                Some(content) => Some(binary::base64_decode(&content, limits.body_bytes)?),
                None => None,
            };
            if kind == Kind::Page && content.is_none() {
                return Err(Error::Missing);
            }
            Ok(Row::Page(types::WikiPage {
                title: bounded(text(tokens, b"title")?, limits.name_bytes)?,
                content,
                sha: id(commit, b"sha", format)?,
            }))
        }
        Kind::Dependencies => Ok(Row::Dependency(types::Dependency {
            number: number(tokens, b"index")?,
            owner: bounded(text(tokens, b"owner")?, limits.name_bytes)?,
            repository: bounded(text(tokens, b"repo")?, limits.name_bytes)?,
        })),
        Kind::Settings => Ok(Row::Other(Document::Settings {
            max_response_items: small(number(tokens, b"max_response_items")?)?,
            default_paging_num: small(number(tokens, b"default_paging_num")?)?,
        })),
        Kind::Repository => Ok(Row::Other(Document::Repository(types::RepositoryInfo {
            id: number(tokens, b"id")?,
            full_name: bounded(text(tokens, b"full_name")?, limits.name_bytes)?,
            object_format: match text(tokens, b"object_format_name")?.as_ref() {
                b"sha1" => ObjectFormat::Sha1,
                b"sha256" => ObjectFormat::Sha256,
                _ => return Err(Error::Unsupported),
            },
            has_wiki: boolean(tokens, b"has_wiki")?,
            default_branch: bounded(text(tokens, b"default_branch")?, limits.name_bytes)?,
        }))),
        Kind::Permission => Ok(Row::Other(Document::Permission(match text(tokens, b"permission")?.as_ref() {
            b"none" => types::Permission::None,
            b"read" => types::Permission::Read,
            b"write" => types::Permission::Write,
            b"admin" => types::Permission::Admin,
            b"owner" => types::Permission::Owner,
            _ => return Err(Error::Malformed),
        }))),
        Kind::Branch => {
            Ok(Row::Other(Document::Branch(id(json::value(tokens, required(tokens, b"commit")?)?, b"id", format)?)))
        }
        Kind::Error => Ok(Row::Other(Document::Error { message: cut(text(tokens, b"message")?, limits.body_bytes) })),
        Kind::Done => Ok(Row::Other(Document::Done)),
    }
}
fn small(n: u64) -> Result<u32, Error> {
    match u32::try_from(n) {
        Ok(n) => Ok(n),
        Err(_) => Err(Error::TooLarge),
    }
}

/// The finite response body written by the fake forge and conformance probes.
pub fn encode(document: &Document, format: ObjectFormat, limits: &Limits) -> Result<Box<[u8]>, Error> {
    let bounds = skein_json::writer::Limits { depth: limits.depth, length: limits.document_bytes };
    let mut measure = Encoder::measure(&bounds);
    write_document(&mut measure, document, format, limits)?;
    let length = request::measured(measure)?;
    let mut write = Encoder::write(length, &bounds);
    write_document(&mut write, document, format, limits)?;
    Ok(write.finish())
}
fn string(out: &mut Encoder, key: &[u8], value: &[u8]) {
    out.key(key);
    out.string(value);
}
fn num(out: &mut Encoder, key: &[u8], value: u64) {
    out.key(key);
    out.unsigned(value);
}
fn bool_field(out: &mut Encoder, key: &[u8], value: bool) {
    out.key(key);
    out.boolean(value);
}
fn source_time(out: &mut Encoder, key: &[u8], value: u64) {
    string(out, key, &time::format(value));
}
fn write_user(out: &mut Encoder, value: &types::User) {
    out.object_start();
    num(out, b"id", value.id);
    string(out, b"login", &value.login);
    out.object_end();
}
fn write_label(out: &mut Encoder, value: &types::Label) {
    out.object_start();
    num(out, b"id", value.id);
    string(out, b"name", &value.name);
    out.object_end();
}
fn write_item(out: &mut Encoder, value: &types::Item) {
    out.object_start();
    num(out, b"number", value.number);
    string(out, b"state", state_text(value.state));
    out.key(b"user");
    write_user(out, &value.user);
    string(out, b"title", &value.title);
    string(out, b"body", &value.body);
    out.key(b"labels");
    out.array_start();
    for label in &value.labels {
        write_label(out, label);
    }
    out.array_end();
    source_time(out, b"created_at", value.created);
    source_time(out, b"updated_at", value.updated);
    out.key(b"pull_request");
    match value.kind {
        types::ItemKind::Issue => out.null(),
        types::ItemKind::Pull => {
            out.object_start();
            out.object_end();
        }
    }
    out.object_end();
}
fn write_comment(out: &mut Encoder, value: &types::Comment) {
    out.object_start();
    num(out, b"id", value.id);
    out.key(b"user");
    write_user(out, &value.user);
    string(out, b"body", &value.body);
    string(out, b"issue_url", &value.issue_url);
    source_time(out, b"created_at", value.created);
    source_time(out, b"updated_at", value.updated);
    out.object_end();
}
fn write_commit(out: &mut Encoder, key: &[u8], value: &[u8; 32], format: ObjectFormat) -> Result<(), Error> {
    string(out, key, &binary::commit_hex(value, format)?);
    Ok(())
}
fn write_pull(out: &mut Encoder, value: &types::Pull, format: ObjectFormat) -> Result<(), Error> {
    out.object_start();
    num(out, b"number", value.number);
    string(out, b"state", state_text(value.state));
    out.key(b"head");
    out.object_start();
    string(out, b"ref", &value.head);
    write_commit(out, b"sha", &value.commit, format)?;
    out.object_end();
    out.key(b"base");
    out.object_start();
    string(out, b"ref", &value.base);
    write_commit(out, b"sha", &value.base_commit, format)?;
    out.object_end();
    bool_field(out, b"merged", value.merged);
    bool_field(out, b"mergeable", value.mergeable);
    out.key(b"merge_commit_sha");
    match value.merge_commit {
        Some(commit) => out.string(&binary::commit_hex(&commit, format)?),
        None => out.string(b""),
    }
    out.key(b"requested_reviewers");
    out.array_start();
    for user in &value.reviewers {
        write_user(out, user);
    }
    out.array_end();
    out.object_end();
    Ok(())
}
fn write_review(out: &mut Encoder, value: &types::Review, format: ObjectFormat) -> Result<(), Error> {
    out.object_start();
    num(out, b"id", value.id);
    out.key(b"user");
    write_user(out, &value.user);
    string(out, b"state", review_text(value.state));
    write_commit(out, b"commit_id", &value.commit, format)?;
    string(out, b"body", &value.body);
    source_time(out, b"submitted_at", value.submitted);
    bool_field(out, b"official", value.official);
    bool_field(out, b"dismissed", value.dismissed);
    out.object_end();
    Ok(())
}
fn write_status(out: &mut Encoder, value: &types::Status) {
    out.object_start();
    string(out, b"context", &value.context);
    string(out, b"status", check_text(value.state));
    out.key(b"creator");
    write_user(out, &value.creator);
    string(out, b"description", &value.description);
    string(out, b"target_url", &value.target_url);
    source_time(out, b"created_at", value.created);
    out.object_end();
}
fn write_remark(out: &mut Encoder, value: &types::Remark, format: ObjectFormat) -> Result<(), Error> {
    out.object_start();
    num(out, b"id", value.id);
    out.key(b"user");
    write_user(out, &value.user);
    string(out, b"body", &value.body);
    string(out, b"path", &value.path);
    string(out, b"diff_hunk", &value.diff_hunk);
    num(out, b"position", value.position);
    write_commit(out, b"original_commit_id", &value.commit, format)?;
    out.object_end();
    Ok(())
}
fn write_page(out: &mut Encoder, value: &types::WikiPage, format: ObjectFormat, limits: &Limits) -> Result<(), Error> {
    out.object_start();
    string(out, b"title", &value.title);
    out.key(b"last_commit");
    out.object_start();
    write_commit(out, b"sha", &value.sha, format)?;
    out.object_end();
    if let Some(content) = &value.content {
        string(out, b"content_base64", &binary::base64_encode(content, limits.document_bytes)?);
    }
    out.object_end();
    Ok(())
}
#[expect(clippy::too_many_lines, reason = "exhaustive typed Forgejo document table")]
fn write_document(out: &mut Encoder, document: &Document, format: ObjectFormat, limits: &Limits) -> Result<(), Error> {
    match document {
        Document::Settings { max_response_items, default_paging_num } => {
            out.object_start();
            num(out, b"max_response_items", u64::from(*max_response_items));
            num(out, b"default_paging_num", u64::from(*default_paging_num));
            out.object_end();
        }
        Document::User(user) => write_user(out, user),
        Document::Users(users) => {
            out.object_start();
            bool_field(out, b"ok", true);
            out.key(b"data");
            out.array_start();
            for user in users {
                write_user(out, user);
            }
            out.array_end();
            out.object_end();
        }
        Document::Repository(repository) => {
            out.object_start();
            num(out, b"id", repository.id);
            string(out, b"full_name", &repository.full_name);
            string(
                out,
                b"object_format_name",
                match repository.object_format {
                    ObjectFormat::Sha1 => b"sha1",
                    ObjectFormat::Sha256 => b"sha256",
                },
            );
            bool_field(out, b"has_wiki", repository.has_wiki);
            string(out, b"default_branch", &repository.default_branch);
            out.object_end();
        }
        Document::Labels(labels) => {
            out.array_start();
            for label in labels {
                write_label(out, label);
            }
            out.array_end();
        }
        Document::Item(item) => write_item(out, item),
        Document::Items(items) => {
            out.array_start();
            for item in items {
                write_item(out, item);
            }
            out.array_end();
        }
        Document::Comment(comment) => write_comment(out, comment),
        Document::Comments(comments) => {
            out.array_start();
            for comment in comments {
                write_comment(out, comment);
            }
            out.array_end();
        }
        Document::Pull(pull) => write_pull(out, pull, format)?,
        Document::Review(review) => write_review(out, review, format)?,
        Document::Reviews(reviews) => {
            out.array_start();
            for review in reviews {
                write_review(out, review, format)?;
            }
            out.array_end();
        }
        Document::Statuses { state, total_count, statuses } => {
            out.object_start();
            string(out, b"state", check_text(*state));
            num(out, b"total_count", *total_count);
            out.key(b"statuses");
            out.array_start();
            for status in statuses {
                write_status(out, status);
            }
            out.array_end();
            out.object_end();
        }
        Document::Remarks(remarks) => {
            out.array_start();
            for remark in remarks {
                write_remark(out, remark, format)?;
            }
            out.array_end();
        }
        Document::Permission(permission) => {
            out.object_start();
            string(
                out,
                b"permission",
                match permission {
                    types::Permission::None => b"none",
                    types::Permission::Read => b"read",
                    types::Permission::Write => b"write",
                    types::Permission::Admin => b"admin",
                    types::Permission::Owner => b"owner",
                },
            );
            out.object_end();
        }
        Document::Branch(commit) => {
            out.object_start();
            out.key(b"commit");
            out.object_start();
            write_commit(out, b"id", commit, format)?;
            out.object_end();
            out.object_end();
        }
        Document::Pages(pages) => {
            out.array_start();
            for page in pages {
                write_page(out, page, format, limits)?;
            }
            out.array_end();
        }
        Document::Page(page) => write_page(out, page, format, limits)?,
        Document::Dependencies(dependencies) => {
            out.array_start();
            for dependency in dependencies {
                out.object_start();
                num(out, b"index", dependency.number);
                string(out, b"owner", &dependency.owner);
                string(out, b"repo", &dependency.repository);
                out.object_end();
            }
            out.array_end();
        }
        Document::Error { message } => {
            out.object_start();
            string(out, b"message", message);
            out.object_end();
        }
        Document::Done => {
            out.object_start();
            out.object_end();
        }
    }
    Ok(())
}
const fn state_text(state: types::State) -> &'static [u8] {
    match state {
        types::State::Open => b"open",
        types::State::Closed => b"closed",
    }
}
const fn check_text(state: types::Check) -> &'static [u8] {
    match state {
        types::Check::Pending => b"pending",
        types::Check::Success => b"success",
        types::Check::Error => b"error",
        types::Check::Failure => b"failure",
        types::Check::Warning => b"warning",
    }
}
const fn review_text(state: types::ReviewState) -> &'static [u8] {
    match state {
        types::ReviewState::Approved => b"APPROVED",
        types::ReviewState::RequestChanges => b"REQUEST_CHANGES",
        types::ReviewState::Comment => b"COMMENT",
        types::ReviewState::Pending => b"PENDING",
        types::ReviewState::Dismissed => b"DISMISSED",
        types::ReviewState::Requested => b"REQUEST_REVIEW",
    }
}

#[expect(clippy::too_many_lines, reason = "exhaustive typed Forgejo document table")]
fn assemble(
    kind: Kind,
    rows: Box<[Row]>,
    state: Option<types::Check>,
    total_count: Option<u64>,
) -> Result<Document, Error> {
    let cap = u32::try_from(rows.len()).expect("bounded rows");
    match kind {
        Kind::Users => {
            let mut out = List::<types::User>::with_capacity(cap);
            for row in rows {
                match row {
                    Row::User(value) => out.push(value).expect("bounded rows"),
                    Row::Label(_)
                    | Row::Item(_)
                    | Row::Comment(_)
                    | Row::Pull(_)
                    | Row::Review(_)
                    | Row::Status(_)
                    | Row::Remark(_)
                    | Row::Page(_)
                    | Row::Dependency(_)
                    | Row::Other(_) => return Err(Error::Malformed),
                }
            }
            Ok(Document::Users(out.into_boxed()))
        }
        Kind::Labels => {
            let mut out = List::<types::Label>::with_capacity(cap);
            for row in rows {
                match row {
                    Row::Label(value) => out.push(value).expect("bounded rows"),
                    Row::User(_)
                    | Row::Item(_)
                    | Row::Comment(_)
                    | Row::Pull(_)
                    | Row::Review(_)
                    | Row::Status(_)
                    | Row::Remark(_)
                    | Row::Page(_)
                    | Row::Dependency(_)
                    | Row::Other(_) => return Err(Error::Malformed),
                }
            }
            Ok(Document::Labels(out.into_boxed()))
        }
        Kind::Items => {
            let mut out = List::<types::Item>::with_capacity(cap);
            for row in rows {
                match row {
                    Row::Item(value) => out.push(value).expect("bounded rows"),
                    Row::User(_)
                    | Row::Label(_)
                    | Row::Comment(_)
                    | Row::Pull(_)
                    | Row::Review(_)
                    | Row::Status(_)
                    | Row::Remark(_)
                    | Row::Page(_)
                    | Row::Dependency(_)
                    | Row::Other(_) => return Err(Error::Malformed),
                }
            }
            Ok(Document::Items(out.into_boxed()))
        }
        Kind::Comments => {
            let mut out = List::<types::Comment>::with_capacity(cap);
            for row in rows {
                match row {
                    Row::Comment(value) => out.push(value).expect("bounded rows"),
                    Row::User(_)
                    | Row::Label(_)
                    | Row::Item(_)
                    | Row::Pull(_)
                    | Row::Review(_)
                    | Row::Status(_)
                    | Row::Remark(_)
                    | Row::Page(_)
                    | Row::Dependency(_)
                    | Row::Other(_) => return Err(Error::Malformed),
                }
            }
            Ok(Document::Comments(out.into_boxed()))
        }
        Kind::Reviews => {
            let mut out = List::<types::Review>::with_capacity(cap);
            for row in rows {
                match row {
                    Row::Review(value) => out.push(value).expect("bounded rows"),
                    Row::User(_)
                    | Row::Label(_)
                    | Row::Item(_)
                    | Row::Comment(_)
                    | Row::Pull(_)
                    | Row::Status(_)
                    | Row::Remark(_)
                    | Row::Page(_)
                    | Row::Dependency(_)
                    | Row::Other(_) => return Err(Error::Malformed),
                }
            }
            Ok(Document::Reviews(out.into_boxed()))
        }
        Kind::Statuses => {
            let mut out = List::<types::Status>::with_capacity(cap);
            for row in rows {
                match row {
                    Row::Status(value) => out.push(value).expect("bounded rows"),
                    Row::User(_)
                    | Row::Label(_)
                    | Row::Item(_)
                    | Row::Comment(_)
                    | Row::Pull(_)
                    | Row::Review(_)
                    | Row::Remark(_)
                    | Row::Page(_)
                    | Row::Dependency(_)
                    | Row::Other(_) => return Err(Error::Malformed),
                }
            }
            Ok(Document::Statuses {
                state: state.ok_or(Error::Missing)?,
                total_count: total_count.ok_or(Error::Missing)?,
                statuses: out.into_boxed(),
            })
        }
        Kind::Remarks => {
            let mut out = List::<types::Remark>::with_capacity(cap);
            for row in rows {
                match row {
                    Row::Remark(value) => out.push(value).expect("bounded rows"),
                    Row::User(_)
                    | Row::Label(_)
                    | Row::Item(_)
                    | Row::Comment(_)
                    | Row::Pull(_)
                    | Row::Review(_)
                    | Row::Status(_)
                    | Row::Page(_)
                    | Row::Dependency(_)
                    | Row::Other(_) => return Err(Error::Malformed),
                }
            }
            Ok(Document::Remarks(out.into_boxed()))
        }
        Kind::Pages => {
            let mut out = List::<types::WikiPage>::with_capacity(cap);
            for row in rows {
                match row {
                    Row::Page(value) => out.push(value).expect("bounded rows"),
                    Row::User(_)
                    | Row::Label(_)
                    | Row::Item(_)
                    | Row::Comment(_)
                    | Row::Pull(_)
                    | Row::Review(_)
                    | Row::Status(_)
                    | Row::Remark(_)
                    | Row::Dependency(_)
                    | Row::Other(_) => return Err(Error::Malformed),
                }
            }
            Ok(Document::Pages(out.into_boxed()))
        }
        Kind::Dependencies => {
            let mut out = List::<types::Dependency>::with_capacity(cap);
            for row in rows {
                match row {
                    Row::Dependency(value) => out.push(value).expect("bounded rows"),
                    Row::User(_)
                    | Row::Label(_)
                    | Row::Item(_)
                    | Row::Comment(_)
                    | Row::Pull(_)
                    | Row::Review(_)
                    | Row::Status(_)
                    | Row::Remark(_)
                    | Row::Page(_)
                    | Row::Other(_) => return Err(Error::Malformed),
                }
            }
            Ok(Document::Dependencies(out.into_boxed()))
        }
        Kind::Settings
        | Kind::User
        | Kind::Repository
        | Kind::Item
        | Kind::Comment
        | Kind::Pull
        | Kind::Review
        | Kind::Permission
        | Kind::Branch
        | Kind::Page
        | Kind::Error
        | Kind::Done => {
            if cap != 1 {
                return Err(Error::Missing);
            }
            let row = rows.into_iter().next().expect("one row");
            match row {
                Row::User(value) => Ok(Document::User(value)),
                Row::Item(value) => Ok(Document::Item(value)),
                Row::Comment(value) => Ok(Document::Comment(value)),
                Row::Pull(value) => Ok(Document::Pull(value)),
                Row::Review(value) => Ok(Document::Review(value)),
                Row::Page(value) => Ok(Document::Page(value)),
                Row::Other(value) => Ok(value),
                Row::Label(_) | Row::Status(_) | Row::Remark(_) | Row::Dependency(_) => Err(Error::Malformed),
            }
        }
    }
}

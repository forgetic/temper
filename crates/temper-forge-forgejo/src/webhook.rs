//! Bounded Forgejo webhook documents and signature verification.
use crate::{Error, Limits, binary, json, types::ObjectFormat};
use alloc::boxed::Box;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use skein_json::{Token, writer::Encoder};
use skein_lib::List;
use skein_lib::bytes;
use subtle::ConstantTimeEq;

/// No signature or bearer secret is printable in traces.
#[expect(missing_debug_implementations, reason = "HMAC state contains the repository secret")]
pub struct Signature {
    mac: Hmac<Sha256>,
    bytes: u32,
    cap: u32,
    failed: bool,
}
impl Signature {
    #[must_use]
    pub fn new(secret: &[u8], cap: u32) -> Signature {
        Signature {
            mac: Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts arbitrary key length"),
            bytes: 0,
            cap,
            failed: false,
        }
    }
    pub fn update(&mut self, piece: &[u8]) -> Result<(), Error> {
        if self.failed {
            return Err(Error::TooLarge);
        }
        let Ok(length) = u32::try_from(piece.len()) else {
            self.failed = true;
            return Err(Error::TooLarge);
        };
        self.bytes = match self.bytes.checked_add(length) {
            Some(length) => length,
            None => {
                self.failed = true;
                return Err(Error::TooLarge);
            }
        };
        if self.bytes > self.cap {
            self.failed = true;
            return Err(Error::TooLarge);
        }
        self.mac.update(piece);
        Ok(())
    }
    pub fn sign(self) -> Result<[u8; 32], Error> {
        if self.failed {
            return Err(Error::TooLarge);
        }
        Ok(self.mac.finalize().into_bytes().into())
    }
    pub fn verify(self, header: &[u8]) -> Result<bool, Error> {
        let mut expected = [0; 32];
        binary::unhex(header, &mut expected)?;
        Ok(bool::from(self.sign()?.ct_eq(&expected)))
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Item,
    Push,
    Create,
    Delete,
    Status,
    Action,
    Wiki,
    Unknown,
}
#[must_use]
pub fn kind(event: &[u8]) -> Kind {
    match event {
        b"issues"
        | b"issue_comment"
        | b"pull_request"
        | b"pull_request_comment"
        | b"pull_request_approved"
        | b"pull_request_rejected" => Kind::Item,
        b"push" => Kind::Push,
        b"create" => Kind::Create,
        b"delete" => Kind::Delete,
        b"status" => Kind::Status,
        b"action_run_success" | b"action_run_recover" | b"action_run_failure" => Kind::Action,
        b"wiki" => Kind::Wiki,
        _ => Kind::Unknown,
    }
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Payload {
    pub repository: Box<[u8]>,
    pub repository_id: u64,
    pub by: Option<u64>,
    pub item: Option<u64>,
    pub branch: Option<Box<[u8]>>,
    pub commit: Option<[u8; 32]>,
    pub wiki: bool,
}

/// Keeps only webhook metadata. Commit lists and other unknown subtrees never
/// enter retained storage; the tokenizer owns at most its capped current string.
#[derive(Debug)]
pub struct Decoder {
    tokens: List<Token>,
    ignored: u32,
    skip: bool,
    depth: u32,
    max_depth: u32,
    whole: bool,
    name_bytes: u32,
    retained: u32,
    bytes: u32,
}
/// Selected tokens and text, the decoded payload copy, and encoded output.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let text = u64::from(limits.name_bytes).checked_mul(12)?.checked_add(1024)?;
    List::<Token>::worst_case(limits.tokens.min(128))?
        .checked_add(text.checked_mul(2)?)?
        .checked_add(u64::from(limits.hook_bytes))
}
impl Decoder {
    #[must_use]
    pub fn new(limits: &Limits) -> Decoder {
        Decoder {
            tokens: List::with_capacity(limits.tokens.min(128)),
            ignored: 0,
            skip: false,
            depth: 0,
            max_depth: limits.depth,
            whole: false,
            name_bytes: limits.name_bytes,
            retained: limits.name_bytes.saturating_mul(12).saturating_add(1024),
            bytes: 0,
        }
    }
    pub fn token(&mut self, token: Token) -> Result<(), Error> {
        if self.whole {
            return Err(Error::Malformed);
        }
        match &token {
            Token::ObjectStart | Token::ArrayStart => self.depth = self.depth.checked_add(1).ok_or(Error::TooLarge)?,
            Token::ObjectEnd | Token::ArrayEnd => self.depth = self.depth.checked_sub(1).ok_or(Error::Malformed)?,
            Token::Key(_) | Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null => {}
        }
        if self.depth > self.max_depth {
            return Err(Error::TooLarge);
        }
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
        if self.skip {
            self.skip = false;
            match token {
                Token::ObjectStart | Token::ArrayStart => self.ignored = 1,
                Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null => {}
                Token::ObjectEnd | Token::ArrayEnd | Token::Key(_) => return Err(Error::Malformed),
            }
            return Ok(());
        }
        match &token {
            Token::Key(key) => {
                if !selected(key) {
                    self.skip = true;
                    return Ok(());
                }
            }
            Token::String(text) => {
                bounded(text, self.name_bytes)?;
            }
            Token::Number(text) => {
                bounded(text, 32)?;
            }
            Token::ObjectStart
            | Token::ObjectEnd
            | Token::ArrayStart
            | Token::ArrayEnd
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
        self.bytes = self.bytes.checked_add(length).ok_or(Error::TooLarge)?;
        if self.bytes > self.retained {
            return Err(Error::TooLarge);
        }
        let closes = token == Token::ObjectEnd || token == Token::ArrayEnd;
        if self.tokens.push(token).is_err() {
            return Err(Error::TooLarge);
        }
        if closes && self.depth == 0 {
            self.whole = true;
        }
        Ok(())
    }
    pub fn finish(self, event: &[u8], format: ObjectFormat, limits: &Limits) -> Result<Option<Payload>, Error> {
        if !self.whole || self.depth != 0 {
            return Err(Error::Malformed);
        }
        decode(event, self.tokens.as_slice(), format, limits)
    }
}
#[expect(clippy::match_like_matches_macro, reason = "the programming subset uses exhaustive matches")]
fn selected(key: &[u8]) -> bool {
    match key {
        b"repository" | b"full_name" | b"id" | b"sender" | b"issue" | b"pull_request" | b"number" | b"ref"
        | b"ref_type" | b"after" | b"sha" | b"run" | b"trigger_user" | b"commit_sha" => true,
        _ => false,
    }
}
pub fn decode(event: &[u8], tokens: &[Token], format: ObjectFormat, limits: &Limits) -> Result<Option<Payload>, Error> {
    let kind = kind(event);
    if kind == Kind::Unknown {
        return Ok(None);
    }
    let source = match kind {
        Kind::Action => json::value(tokens, json::required(tokens, b"run")?)?,
        Kind::Item | Kind::Push | Kind::Create | Kind::Delete | Kind::Status | Kind::Wiki => tokens,
        Kind::Unknown => unreachable!("unknown event returned"),
    };
    let repository = json::value(source, json::required(source, b"repository")?)?;
    let repository_id = number(repository, b"id")?;
    let name = text(repository, b"full_name")?;
    bounded(&name, limits.name_bytes)?;
    let sender_key = match kind {
        Kind::Action => b"trigger_user".as_slice(),
        Kind::Item | Kind::Push | Kind::Create | Kind::Delete | Kind::Status | Kind::Wiki => b"sender".as_slice(),
        Kind::Unknown => unreachable!("unknown event returned"),
    };
    let by = match json::field(source, sender_key)? {
        Some(at) => {
            let sender = json::value(source, at)?;
            if sender == [Token::Null] { None } else { Some(number(sender, b"id")?) }
        }
        None => None,
    };
    let mut out = Payload {
        repository: name,
        repository_id,
        by,
        item: None,
        branch: None,
        commit: None,
        wiki: kind == Kind::Wiki,
    };
    match kind {
        Kind::Item => {
            let item = match json::field(tokens, b"issue")? {
                Some(at) => json::value(tokens, at)?,
                None => json::value(tokens, json::required(tokens, b"pull_request")?)?,
            };
            out.item = Some(number(item, b"number")?);
        }
        Kind::Push => {
            let reference = text(tokens, b"ref")?;
            let Some(branch) = reference.strip_prefix(b"refs/heads/") else { return Ok(None) };
            bounded(branch, limits.name_bytes)?;
            out.branch = Some(bytes::copy_of(branch));
            let after = text(tokens, b"after")?;
            let commit = binary::commit(&after, format)?;
            let mut zero = true;
            for byte in commit {
                if byte != 0 {
                    zero = false;
                }
            }
            if !zero {
                out.commit = Some(commit);
            }
        }
        Kind::Create | Kind::Delete => {
            if text(tokens, b"ref_type")?.as_ref() != b"branch" {
                return Ok(None);
            }
            let branch = text(tokens, b"ref")?;
            bounded(&branch, limits.name_bytes)?;
            out.branch = Some(branch);
        }
        Kind::Status => {
            let sha = text(tokens, b"sha")?;
            out.commit = Some(binary::commit(&sha, format)?);
        }
        Kind::Action => out.commit = Some(binary::commit(&text(source, b"commit_sha")?, format)?),
        Kind::Wiki => {}
        Kind::Unknown => unreachable!("unknown event returned"),
    }
    Ok(Some(out))
}
fn bounded(text: &[u8], cap: u32) -> Result<(), Error> {
    if text.len() > usize::try_from(cap).expect("u32 fits usize") { Err(Error::TooLarge) } else { Ok(()) }
}
fn text(tokens: &[Token], key: &[u8]) -> Result<Box<[u8]>, Error> {
    Ok(bytes::copy_of(json::text(json::value(tokens, json::required(tokens, key)?)?)?))
}
fn number(tokens: &[Token], key: &[u8]) -> Result<u64, Error> {
    json::unsigned(json::value(tokens, json::required(tokens, key)?)?)
}

/// Server-side delivery documents; their selected fields agree with the client.
pub fn encode(event: &[u8], payload: &Payload, format: ObjectFormat, limits: &Limits) -> Result<Box<[u8]>, Error> {
    let bounds = skein_json::writer::Limits { depth: limits.depth, length: limits.hook_bytes };
    let mut measure = Encoder::measure(&bounds);
    write(&mut measure, event, payload, format)?;
    let length = crate::request::measured(measure)?;
    let mut out = Encoder::write(length, &bounds);
    write(&mut out, event, payload, format)?;
    Ok(out.finish())
}
fn string(out: &mut Encoder, key: &[u8], text: &[u8]) {
    out.key(key);
    out.string(text);
}
fn write(out: &mut Encoder, event: &[u8], payload: &Payload, format: ObjectFormat) -> Result<(), Error> {
    if kind(event) == Kind::Action {
        return write_action(out, event, payload, format);
    }
    out.object_start();
    out.key(b"repository");
    out.object_start();
    out.key(b"id");
    out.unsigned(payload.repository_id);
    string(out, b"full_name", &payload.repository);
    out.object_end();
    out.key(b"sender");
    match payload.by {
        Some(id) => {
            out.object_start();
            out.key(b"id");
            out.unsigned(id);
            out.object_end();
        }
        None => out.null(),
    }
    match kind(event) {
        Kind::Item => {
            out.key(if event.starts_with(b"pull_request") { b"pull_request" } else { b"issue" });
            out.object_start();
            out.key(b"number");
            out.unsigned(payload.item.ok_or(Error::Missing)?);
            out.object_end();
        }
        Kind::Push => {
            let branch = payload.branch.as_ref().ok_or(Error::Missing)?;
            let mut reference = skein_lib::Writer::new(11_usize.checked_add(branch.len()).ok_or(Error::TooLarge)?);
            reference.put(b"refs/heads/").expect("reference measured");
            reference.put(branch).expect("measured");
            string(out, b"ref", &reference.finish());
            string(out, b"after", &binary::commit_hex(&payload.commit.unwrap_or([0; 32]), format)?);
            out.key(b"commits");
            out.array_start();
            out.array_end();
        }
        Kind::Create | Kind::Delete => {
            string(out, b"ref_type", b"branch");
            string(out, b"ref", payload.branch.as_ref().ok_or(Error::Missing)?);
        }
        Kind::Status => string(out, b"sha", &binary::commit_hex(&payload.commit.ok_or(Error::Missing)?, format)?),
        Kind::Wiki => {}
        Kind::Action => unreachable!("action documents written above"),
        Kind::Unknown => return Err(Error::Unsupported),
    }
    out.object_end();
    Ok(())
}

// Source-derived v15 ActionPayload/ActionRun metadata. Actions runner captures
// are still pending; this emits the selected fields, not a captured full run.
fn write_action(out: &mut Encoder, event: &[u8], payload: &Payload, format: ObjectFormat) -> Result<(), Error> {
    out.object_start();
    string(
        out,
        b"action",
        match event {
            b"action_run_success" => b"success",
            b"action_run_recover" => b"recover",
            b"action_run_failure" => b"failure",
            _ => return Err(Error::Unsupported),
        },
    );
    out.key(b"run");
    out.object_start();
    out.key(b"repository");
    out.object_start();
    out.key(b"id");
    out.unsigned(payload.repository_id);
    string(out, b"full_name", &payload.repository);
    out.object_end();
    out.key(b"trigger_user");
    match payload.by {
        Some(id) => {
            out.object_start();
            out.key(b"id");
            out.unsigned(id);
            out.object_end();
        }
        None => out.null(),
    }
    string(out, b"commit_sha", &binary::commit_hex(&payload.commit.ok_or(Error::Missing)?, format)?);
    out.object_end();
    string(out, b"prior_status", b"running");
    out.object_end();
    Ok(())
}

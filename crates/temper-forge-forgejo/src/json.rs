//! Bounded request documents; response history is never buffered here.
use crate::{Error, Limits};
use alloc::boxed::Box;
use skein_json::{Token, tokenizer};
use skein_lib::{Env, List, Queue, Time, Wall, bytes, stream};

#[derive(Debug)]
pub struct Json {
    tokens: Box<[Token]>,
}

/// Finite input token storage, tokenizer state, demanded bytes, and the final
/// boxed token-list copy. Total decoded token text never exceeds input bytes.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let bounds = tokenizer::Limits {
        depth: limits.depth,
        string: limits.document_bytes,
        number: 32,
        length: limits.document_bytes,
        chunk: 1024,
    };
    tokenizer::worst_case(&bounds)?
        .checked_add(List::<Token>::worst_case(limits.tokens)?.checked_mul(2)?)?
        .checked_add(u64::from(limits.document_bytes))?
        .checked_add(Queue::<tokenizer::Event>::worst_case(tokenizer::UP_MAX_OUT.above)?)?
        .checked_add(Queue::<stream::Down>::worst_case(tokenizer::UP_MAX_OUT.below)?)
}
impl Json {
    pub fn from_bytes(input: &[u8], limits: &Limits) -> Result<Json, Error> {
        if input.len() > usize::try_from(limits.document_bytes).expect("u32 fits usize") {
            return Err(Error::TooLarge);
        }
        let bounds = tokenizer::Limits {
            depth: limits.depth,
            string: limits.document_bytes,
            number: 32,
            length: limits.document_bytes,
            chunk: 1024,
        };
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: bounds };
        let mut machine = tokenizer::Tokenizer::new(&bounds);
        let mut events = Queue::with_capacity(tokenizer::UP_MAX_OUT.above);
        let mut demands = Queue::with_capacity(tokenizer::UP_MAX_OUT.below);
        let mut tokens = List::with_capacity(limits.tokens);
        let mut at = 0;
        tokenizer::down(&mut machine, &env, tokenizer::Request::Next, &mut events, &mut demands);
        for _ in 0..limits.document_bytes.saturating_mul(4).saturating_add(limits.tokens) {
            if let Some(event) = events.pop() {
                match event {
                    tokenizer::Event::Token(token) => {
                        if tokens.push(token).is_err() {
                            return Err(Error::TooLarge);
                        }
                        tokenizer::down(&mut machine, &env, tokenizer::Request::Next, &mut events, &mut demands);
                    }
                    tokenizer::Event::Done => return Ok(Json { tokens: tokens.into_boxed() }),
                    tokenizer::Event::Failed(_) | tokenizer::Event::Closed => return Err(Error::Malformed),
                }
            } else if let Some(demand) = demands.pop() {
                match demand {
                    stream::Down::Demand { read, room } => {
                        assert!(room == 0, "JSON tokenizer reads only");
                        let rest = input.get(at..).expect("cursor stays within input");
                        let length = match read {
                            stream::Read::Nothing => 0,
                            stream::Read::Fill(n) => usize::try_from(n).expect("u32 fits usize"),
                            stream::Read::Scan { until, max } => {
                                let cap = usize::try_from(max).expect("u32 fits usize").min(rest.len());
                                match bytes::find(rest.get(..cap).expect("bounded scan"), until.as_bytes()) {
                                    Some(position) => {
                                        position.checked_add(until.as_bytes().len()).expect("within input")
                                    }
                                    None => usize::try_from(max).expect("u32 fits usize"),
                                }
                            }
                            stream::Read::Line { .. } => return Err(Error::Unsupported),
                        };
                        let end = at.checked_add(length).ok_or(Error::TooLarge)?;
                        if length == 0 || end > input.len() {
                            tokenizer::up(&mut machine, &env, stream::Up::End, &mut events, &mut demands);
                        } else {
                            let part = bytes::copy_of(input.get(at..end).expect("checked delivery"));
                            at = end;
                            tokenizer::up(&mut machine, &env, stream::Up::Bytes(part), &mut events, &mut demands);
                        }
                    }
                    stream::Down::Send(_) | stream::Down::Finish => return Err(Error::Malformed),
                }
            } else {
                return Err(Error::Malformed);
            }
        }
        Err(Error::TooLarge)
    }
    #[must_use]
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }
}

pub fn extent(tokens: &[Token], at: usize) -> Result<usize, Error> {
    let mut depth: u32 = 0;
    for (index, token) in tokens.iter().enumerate().skip(at) {
        match token {
            Token::ObjectStart | Token::ArrayStart => depth = depth.checked_add(1).ok_or(Error::TooLarge)?,
            Token::ObjectEnd | Token::ArrayEnd => {
                depth = depth.checked_sub(1).ok_or(Error::Malformed)?;
            }
            Token::Key(_) => {
                if depth == 0 {
                    return Err(Error::Malformed);
                }
            }
            Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null => {}
        }
        if depth == 0 {
            return index.checked_add(1).ok_or(Error::TooLarge);
        }
    }
    Err(Error::Malformed)
}
pub fn value(tokens: &[Token], at: usize) -> Result<&[Token], Error> {
    tokens.get(at..extent(tokens, at)?).ok_or(Error::Malformed)
}
pub fn field(tokens: &[Token], name: &[u8]) -> Result<Option<usize>, Error> {
    if tokens.first() != Some(&Token::ObjectStart) || tokens.last() != Some(&Token::ObjectEnd) {
        return Err(Error::Malformed);
    }
    let mut next = 1;
    let mut found = None;
    for (at, token) in tokens.iter().enumerate().skip(1) {
        if at != next {
            continue;
        }
        match token {
            Token::Key(key) => {
                let start = at.checked_add(1).ok_or(Error::TooLarge)?;
                let end = extent(tokens, start)?;
                if key.as_ref() == name {
                    if found.is_some() {
                        return Err(Error::Duplicate);
                    }
                    found = Some(start);
                }
                next = end;
            }
            Token::ObjectEnd if at.checked_add(1) == Some(tokens.len()) => {}
            Token::ObjectStart
            | Token::ObjectEnd
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null => return Err(Error::Malformed),
        }
    }
    Ok(found)
}
pub fn required(tokens: &[Token], key: &[u8]) -> Result<usize, Error> {
    field(tokens, key)?.ok_or(Error::Missing)
}
pub fn text(tokens: &[Token]) -> Result<&[u8], Error> {
    match tokens {
        [Token::String(text)] => Ok(text),
        _ => Err(Error::Malformed),
    }
}
pub fn unsigned(tokens: &[Token]) -> Result<u64, Error> {
    match tokens {
        [Token::Number(text)] => decimal(text),
        _ => Err(Error::Malformed),
    }
}
pub fn decimal(text: &[u8]) -> Result<u64, Error> {
    if text.is_empty() {
        return Err(Error::Malformed);
    }
    let mut n: u64 = 0;
    for &byte in text {
        if !byte.is_ascii_digit() {
            return Err(Error::Malformed);
        }
        n = n
            .checked_mul(10)
            .ok_or(Error::TooLarge)?
            .checked_add(u64::from(byte.wrapping_sub(b'0')))
            .ok_or(Error::TooLarge)?;
    }
    Ok(n)
}
pub fn boolean(tokens: &[Token]) -> Result<bool, Error> {
    match tokens {
        [Token::True] => Ok(true),
        [Token::False] => Ok(false),
        _ => Err(Error::Malformed),
    }
}
pub fn array(tokens: &[Token], cap: u32) -> Result<Box<[usize]>, Error> {
    if tokens.first() != Some(&Token::ArrayStart) || tokens.last() != Some(&Token::ArrayEnd) {
        return Err(Error::Malformed);
    }
    let mut rows = List::with_capacity(cap);
    let mut next = 1;
    for (at, _token) in tokens.iter().enumerate().skip(1) {
        if at != next || at.checked_add(1) == Some(tokens.len()) {
            continue;
        }
        next = extent(tokens, at)?;
        if rows.push(at).is_err() {
            return Err(Error::TooLarge);
        }
    }
    Ok(rows.into_boxed())
}

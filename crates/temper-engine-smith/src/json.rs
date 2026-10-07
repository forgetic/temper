//! Bounded JSON-object decoding with Skein's tokenizer. The decoded tree is
//! temporary and never crosses the engine or Smith domain boundary.

use alloc::boxed::Box;
use skein_json::{Token, tokenizer};
use skein_lib::stream::{Down, Up};
use skein_lib::{Env, Intake, List, Queue, Time, Wall};

use crate::Problem;

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Value {
    Object(Box<[(Box<[u8]>, Value)]>),
    Array(Box<[Value]>),
    String(Box<[u8]>),
    Number(Box<[u8]>),
    Boolean(bool),
    Null,
}

pub(crate) fn parse(input: &[u8]) -> Result<Value, Problem> {
    if input.len() > 65_536 {
        return Err(Problem::TooLarge);
    }
    let Ok(length) = u32::try_from(input.len()) else { return Err(Problem::TooLarge) };
    let limits = tokenizer::Limits { depth: 16, string: length, number: 24, chunk: length.max(1), length };
    let mut machine = tokenizer::Tokenizer::new(&limits);
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut intake = Intake::with_capacity(length.max(tokenizer::largest_demand(&limits)));
    if intake.append(input).is_err() {
        return Err(Problem::TooLarge);
    }
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut tokens = List::with_capacity(length);
    let mut demanded = None;
    let steps = input.len().checked_mul(4).ok_or(Problem::TooLarge)?;
    let steps = steps.checked_add(8).ok_or(Problem::TooLarge)?;
    for _ in 0..steps {
        match demanded.take() {
            Some(read) => match intake.meet(read) {
                Some(bytes) => tokenizer::up(&mut machine, &env, Up::Bytes(bytes), &mut above, &mut below),
                None => tokenizer::up(&mut machine, &env, Up::End, &mut above, &mut below),
            },
            None => tokenizer::down(&mut machine, &env, tokenizer::Request::Next, &mut above, &mut below),
        }
        match below.pop() {
            Some(Down::Demand { read, .. }) => demanded = Some(read),
            Some(Down::Send(_) | Down::Finish) => return Err(Problem::Malformed),
            None => {}
        }
        match above.pop() {
            Some(tokenizer::Event::Token(token)) => {
                let Ok(()) = tokens.push(token) else { return Err(Problem::TooLarge) };
            }
            Some(tokenizer::Event::Done) => {
                let tokens = tokens.into_boxed();
                let mut at = 0;
                let value = value(&tokens, &mut at)?;
                if at != tokens.len() {
                    return Err(Problem::Malformed);
                }
                return match value {
                    Value::Object(_) => Ok(value),
                    Value::Array(_) | Value::String(_) | Value::Number(_) | Value::Boolean(_) | Value::Null => {
                        Err(Problem::Type)
                    }
                };
            }
            Some(tokenizer::Event::Failed(_) | tokenizer::Event::Closed) => return Err(Problem::Malformed),
            None => {}
        }
    }
    Err(Problem::Malformed)
}

fn value(tokens: &[Token], at: &mut usize) -> Result<Value, Problem> {
    let token = tokens.get(*at).ok_or(Problem::Malformed)?;
    *at = at.checked_add(1).ok_or(Problem::TooLarge)?;
    match token {
        Token::ObjectStart => {
            let Ok(capacity) = u32::try_from(tokens.len()) else { return Err(Problem::TooLarge) };
            let mut fields = List::with_capacity(capacity);
            for _ in 0..tokens.len() {
                match tokens.get(*at).ok_or(Problem::Malformed)? {
                    Token::ObjectEnd => {
                        *at = at.checked_add(1).ok_or(Problem::TooLarge)?;
                        return Ok(Value::Object(fields.into_boxed()));
                    }
                    Token::Key(key) => {
                        *at = at.checked_add(1).ok_or(Problem::TooLarge)?;
                        for (previous, _) in &fields {
                            if previous == key {
                                return Err(Problem::Malformed);
                            }
                        }
                        let item = value(tokens, at)?;
                        if fields.push((key.clone(), item)).is_err() {
                            return Err(Problem::TooLarge);
                        }
                    }
                    Token::ObjectStart
                    | Token::ArrayStart
                    | Token::ArrayEnd
                    | Token::String(_)
                    | Token::Number(_)
                    | Token::True
                    | Token::False
                    | Token::Null => return Err(Problem::Malformed),
                }
            }
            Err(Problem::Malformed)
        }
        Token::ArrayStart => {
            let Ok(capacity) = u32::try_from(tokens.len()) else { return Err(Problem::TooLarge) };
            let mut items = List::with_capacity(capacity);
            for _ in 0..tokens.len() {
                match tokens.get(*at).ok_or(Problem::Malformed)? {
                    Token::ArrayEnd => {
                        *at = at.checked_add(1).ok_or(Problem::TooLarge)?;
                        return Ok(Value::Array(items.into_boxed()));
                    }
                    Token::ObjectStart
                    | Token::ObjectEnd
                    | Token::ArrayStart
                    | Token::Key(_)
                    | Token::String(_)
                    | Token::Number(_)
                    | Token::True
                    | Token::False
                    | Token::Null => {
                        let item = value(tokens, at)?;
                        if items.push(item).is_err() {
                            return Err(Problem::TooLarge);
                        }
                    }
                }
            }
            Err(Problem::Malformed)
        }
        Token::String(text) => Ok(Value::String(text.clone())),
        Token::Number(number) => Ok(Value::Number(number.clone())),
        Token::True => Ok(Value::Boolean(true)),
        Token::False => Ok(Value::Boolean(false)),
        Token::Null => Ok(Value::Null),
        Token::ObjectEnd | Token::ArrayEnd | Token::Key(_) => Err(Problem::Malformed),
    }
}

impl Value {
    pub(crate) fn field(&self, name: &[u8]) -> Result<Option<&Value>, Problem> {
        match self {
            Value::Object(fields) => {
                for (key, value) in fields {
                    if key.as_ref() == name {
                        return Ok(Some(value));
                    }
                }
                Ok(None)
            }
            Value::Array(_) | Value::String(_) | Value::Number(_) | Value::Boolean(_) | Value::Null => {
                Err(Problem::Type)
            }
        }
    }

    pub(crate) fn required(&self, name: &[u8]) -> Result<&Value, Problem> {
        self.field(name)?.ok_or(Problem::Missing)
    }

    pub(crate) fn text(&self) -> Result<Box<[u8]>, Problem> {
        match self {
            Value::String(text) => Ok(text.clone()),
            Value::Object(_) | Value::Array(_) | Value::Number(_) | Value::Boolean(_) | Value::Null => {
                Err(Problem::Type)
            }
        }
    }

    pub(crate) fn number(&self) -> Result<u64, Problem> {
        match self {
            Value::Number(text) => crate::decimal(text),
            Value::Object(_) | Value::Array(_) | Value::String(_) | Value::Boolean(_) | Value::Null => {
                Err(Problem::Type)
            }
        }
    }

    pub(crate) fn boolean(&self) -> Result<bool, Problem> {
        match self {
            Value::Boolean(value) => Ok(*value),
            Value::Object(_) | Value::Array(_) | Value::String(_) | Value::Number(_) | Value::Null => {
                Err(Problem::Type)
            }
        }
    }

    pub(crate) fn array(&self) -> Result<&[Value], Problem> {
        match self {
            Value::Array(items) => Ok(items),
            Value::Object(_) | Value::String(_) | Value::Number(_) | Value::Boolean(_) | Value::Null => {
                Err(Problem::Type)
            }
        }
    }

    pub(crate) fn small(&self) -> Result<u32, Problem> {
        let Ok(value) = u32::try_from(self.number()?) else { return Err(Problem::Range) };
        Ok(value)
    }

    pub(crate) fn narrow(&self) -> Result<u16, Problem> {
        let Ok(value) = u16::try_from(self.number()?) else { return Err(Problem::Range) };
        Ok(value)
    }
}

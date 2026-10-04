//! Values stay in the protocol, by account and the two newest generations
//! (credentials.md, 7; channel.md, 9). Receipt anchors remaining validity.
use crate::{Error, Limits};
use alloc::boxed::Box;
use skein_lib::{Duration, List, Time};
use temper_agent_domain::{Grant, GrantName};
use temper_channel::wire;

#[expect(missing_debug_implementations, reason = "credential bytes must never occur in traces")]
pub struct Value {
    name: GrantName,
    token: Box<[u8]>,
    account_id: Box<[u8]>,
    expires: Time,
    rejected: bool,
    live: bool,
}
impl Value {
    #[must_use]
    pub const fn name(&self) -> GrantName {
        self.name
    }
    #[must_use]
    pub fn token(&self) -> &[u8] {
        &self.token
    }
    #[must_use]
    pub fn account_id(&self) -> &[u8] {
        &self.account_id
    }
    #[must_use]
    pub fn remaining(&self, now: Time) -> Duration {
        self.expires.saturating_since(now)
    }
}
struct Account {
    name: u32,
    newest: Option<Value>,
    previous: Option<Value>,
}
#[expect(missing_debug_implementations, reason = "credential bytes must never occur in traces")]
pub struct Table {
    accounts: Box<[Account]>,
}
impl Table {
    pub fn new(scope: &[u32], limits: &Limits) -> Result<Table, Error> {
        if scope.len() > usize::try_from(limits.accounts).expect("u32 fits usize") {
            return Err(Error::TooLarge);
        }
        let mut accounts = List::with_capacity(limits.accounts);
        for account in scope {
            for row in &accounts {
                let Account { name: other, .. } = row;
                if account == other {
                    return Err(Error::Grant);
                }
            }
            assert!(
                accounts.push(Account { name: *account, newest: None, previous: None }).is_ok(),
                "scope was bounded"
            );
        }
        Ok(Table { accounts: accounts.into_boxed() })
    }
    #[expect(clippy::manual_map, reason = "explicit bounded matches avoid closure-taking combinators")]
    pub fn insert(&mut self, grant: wire::Grant, now: Time, limits: &Limits) -> Result<Option<Grant>, Error> {
        if !bearer(&grant.token)
            || !header(&grant.account_id)
            || grant.token.len() > usize::try_from(limits.token_bytes).expect("u32 fits usize")
            || grant.account_id.len() > usize::try_from(limits.token_bytes).expect("u32 fits usize")
        {
            return Err(Error::Grant);
        }
        let row = self.row(grant.account).ok_or(Error::Grant)?;
        let name = GrantName { account: grant.account, generation: grant.generation };
        let value = Value {
            name,
            token: grant.token,
            account_id: grant.account_id,
            expires: now.saturating_add(grant.valid),
            rejected: false,
            live: true,
        };
        let valid = value.remaining(now);
        for slot in [&mut row.newest, &mut row.previous] {
            if let Some(existing) = slot
                && existing.name == name
            {
                if !existing.live || existing.expires <= now {
                    retire(existing);
                    return Ok(None);
                }
                if existing.token != value.token || existing.account_id != value.account_id {
                    return Err(Error::Grant);
                }
                existing.expires = existing.expires.min(value.expires);
                return Ok(Some(Grant { name, valid: existing.remaining(now) }));
            }
        }
        let newest = match &row.newest {
            Some(value) => Some(value.name.generation),
            None => None,
        };
        match newest {
            None => row.newest = Some(value),
            Some(generation) if name.generation > generation => {
                row.previous = row.newest.take();
                row.newest = Some(value);
            }
            Some(_) => {
                if let Some(previous) = &row.previous
                    && name.generation <= previous.name.generation
                {
                    return Ok(None);
                }
                row.previous = Some(value);
            }
        }
        Ok(Some(Grant { name, valid }))
    }
    #[must_use]
    pub fn get(&self, name: GrantName, now: Time, skew: Duration) -> Option<&Value> {
        for row in &self.accounts {
            if row.name == name.account {
                for value in [&row.newest, &row.previous] {
                    if let Some(value) = value
                        && value.name == name
                        && value.live
                        && !value.rejected
                        && value.remaining(now) > skew
                    {
                        return Some(value);
                    }
                }
            }
        }
        None
    }
    /// True once for this retained generation. The owner emits its rejection
    /// notice before the call's Unauthorized terminal.
    pub fn reject(&mut self, name: GrantName) -> bool {
        let Some(row) = self.row(name.account) else {
            return false;
        };
        for slot in [&mut row.newest, &mut row.previous] {
            if let Some(value) = slot
                && value.name == name
            {
                if value.rejected {
                    return false;
                }
                value.rejected = true;
                retire(value);
                return true;
            }
        }
        false
    }
    /// Releases expired values while retaining their retired generation names.
    pub fn expire(&mut self, now: Time) {
        for row in &mut self.accounts {
            for slot in [&mut row.newest, &mut row.previous] {
                if let Some(value) = slot
                    && value.expires <= now
                {
                    retire(value);
                }
            }
        }
    }
    /// Physical channel settlement releases values; retired names remain fenced.
    pub fn clear(&mut self) {
        for row in &mut self.accounts {
            if let Some(value) = &mut row.newest {
                retire(value);
            }
            if let Some(value) = &mut row.previous {
                retire(value);
            }
        }
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let mut next: Option<Time> = None;
        for row in &self.accounts {
            for value in [&row.newest, &row.previous] {
                if let Some(value) = value
                    && value.live
                {
                    next = Some(match next {
                        Some(old) => old.min(value.expires),
                        None => value.expires,
                    });
                }
            }
        }
        next
    }
    /// Only live newest names cross into the domain at delayed Start admission.
    #[must_use]
    pub fn names(&self, now: Time) -> Box<[Grant]> {
        let count = u32::try_from(self.accounts.len()).expect("admitted account count");
        let mut values = List::with_capacity(count);
        for row in &self.accounts {
            if let Some(value) = &row.newest
                && value.live
                && !value.rejected
                && value.expires > now
            {
                values.push(Grant { name: value.name, valid: value.remaining(now) }).expect("one name per account");
            }
        }
        values.into_boxed()
    }
    #[expect(clippy::manual_find, reason = "bounded loops replace closure-taking combinators in step code")]
    fn row(&mut self, account: u32) -> Option<&mut Account> {
        for row in &mut self.accounts {
            if row.name == account {
                return Some(row);
            }
        }
        None
    }
}
fn retire(value: &mut Value) {
    value.token = Box::new([]);
    value.account_id = Box::new([]);
    value.live = false;
}
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    List::<Account>::worst_case(limits.accounts)?
        .checked_add(u64::from(limits.accounts).checked_mul(4)?.checked_mul(u64::from(limits.token_bytes))?)
}
/// RFC 6750 b64token, before it can enter an Authorization header.
#[must_use]
pub fn bearer(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.first() == Some(&b'=') {
        return false;
    }
    let mut padding = false;
    for byte in bytes {
        if *byte == b'=' {
            padding = true;
        } else if padding
            || !(byte.is_ascii_alphanumeric()
                || *byte == b'-'
                || *byte == b'.'
                || *byte == b'_'
                || *byte == b'~'
                || *byte == b'+'
                || *byte == b'/')
        {
            return false;
        }
    }
    true
}
#[must_use]
pub fn header(bytes: &[u8]) -> bool {
    for byte in bytes {
        if !(0x21..=0x7E).contains(byte) {
            return false;
        }
    }
    true
}

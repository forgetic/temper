//! Values beside the domains' names: two generations per configured account.
use alloc::boxed::Box;
use skein_lib::{Duration, List, Time, bytes::copy_of};
use temper_channel::wire;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Accounts,
    Unknown,
    TooLarge,
    Conflict,
}
struct Value {
    live: bool,
    generation: u64,
    expires: Time,
    token: Box<[u8]>,
    account_id: Box<[u8]>,
}
struct Account {
    name: u32,
    newest: Option<Value>,
    older: Option<Value>,
}
#[expect(missing_debug_implementations, reason = "credential values must never occur in traces")]
pub struct Table {
    accounts: Box<[Account]>,
    token_bytes: u32,
}
impl Table {
    #[must_use]
    pub const fn token_bytes(&self) -> u32 {
        self.token_bytes
    }
    pub fn new(names: &[u32], accounts: u32, token_bytes: u32) -> Result<Table, Error> {
        if u32::try_from(names.len()).ok().ok_or(Error::Accounts)? > accounts {
            return Err(Error::Accounts);
        }
        let mut slots = List::with_capacity(u32::try_from(names.len()).expect("validated account count"));
        for (index, name) in names.iter().enumerate() {
            for other in names.iter().skip(index.saturating_add(1)) {
                if name == other {
                    return Err(Error::Accounts);
                }
            }
            assert!(slots.push(Account { name: *name, newest: None, older: None }).is_ok(), "source account count");
        }
        Ok(Table { accounts: slots.into_boxed(), token_bytes })
    }
    pub fn insert(&mut self, grant: wire::Grant, now: Time, skew: Duration) -> Result<(), Error> {
        if u32::try_from(grant.token.len()).ok().ok_or(Error::TooLarge)? > self.token_bytes
            || u32::try_from(grant.account_id.len()).ok().ok_or(Error::TooLarge)? > self.token_bytes
        {
            return Err(Error::TooLarge);
        }
        let value = Value {
            live: true,
            generation: grant.generation,
            expires: now.saturating_add(Duration::from_nanos(grant.valid.as_nanos().saturating_sub(skew.as_nanos()))),
            token: grant.token,
            account_id: grant.account_id,
        };
        for slot in &mut self.accounts {
            if slot.name == grant.account {
                return insert(slot, value, now);
            }
        }
        Err(Error::Unknown)
    }
    #[must_use]
    pub fn grant(&self, account: u32, generation: u64, now: Time) -> Option<wire::Grant> {
        for slot in &self.accounts {
            if slot.name == account {
                for held in [&slot.newest, &slot.older] {
                    if let Some(value) = held
                        && value.live
                        && value.generation == generation
                        && value.expires > now
                    {
                        return Some(wire::Grant {
                            account,
                            generation,
                            valid: value.expires.saturating_since(now),
                            token: copy_of(&value.token),
                            account_id: copy_of(&value.account_id),
                        });
                    }
                }
            }
        }
        None
    }
    pub fn expire(&mut self, now: Time) {
        for slot in &mut self.accounts {
            for held in [&mut slot.newest, &mut slot.older] {
                if let Some(value) = held
                    && value.expires <= now
                {
                    value.token = Box::new([]);
                    value.account_id = Box::new([]);
                    value.live = false;
                }
            }
        }
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let mut at: Option<Time> = None;
        for account in &self.accounts {
            for held in [&account.newest, &account.older] {
                if let Some(value) = held
                    && value.live
                {
                    at = Some(match at {
                        Some(previous) => previous.min(value.expires),
                        None => value.expires,
                    });
                }
            }
        }
        at
    }
}
fn insert(slot: &mut Account, value: Value, now: Time) -> Result<(), Error> {
    for held in [&mut slot.newest, &mut slot.older] {
        if let Some(previous) = held
            && previous.generation == value.generation
        {
            // Retain the retired name after releasing its secret bytes, so a
            // delayed delivery cannot resurrect an expired generation.
            if !previous.live || previous.expires <= now {
                return Ok(());
            }
            if previous.token != value.token || previous.account_id != value.account_id {
                return Err(Error::Conflict);
            }
            previous.expires = previous.expires.min(value.expires);
            return Ok(());
        }
    }
    if let Some(newest) = &slot.newest {
        if value.generation > newest.generation {
            slot.older = slot.newest.take();
            slot.newest = Some(value);
        } else {
            let replace = match &slot.older {
                Some(older) => value.generation > older.generation,
                None => true,
            };
            if replace {
                slot.older = Some(value);
            }
        }
    } else {
        slot.newest = Some(value);
    }
    Ok(())
}
#[must_use]
pub fn worst_case(accounts: u32, token_bytes: u32) -> Option<u64> {
    u64::from(accounts)
        .checked_mul(u64::try_from(size_of::<Account>()).ok()?.checked_add(u64::from(token_bytes).checked_mul(4)?)?)
}

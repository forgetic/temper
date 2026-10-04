//! Engine-owned values and refresh candidates (credentials.md, 4–6).
//! A candidate is invisible to link translation until durable Keep succeeds.
use crate::{connection::Transport, translate::Value};
use alloc::boxed::Box;
use skein_lib::{Duration, List, Time, Wall, bytes::copy_of};
use temper_engine_domain::Account;
use temper_oauth::{self as document, AccountKind, RefreshRequest, RefreshState, SavedToken, TokenResponse};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub accounts: u32,
    pub identities: u32,
    pub endpoint_bytes: u32,
    pub documents: document::Limits,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Limits,
    Account,
    Generation,
    Document(document::DecodeError),
    Candidate,
}
#[derive(Debug)]
pub struct Endpoint {
    pub address: skein_io::kernel::Addr,
    pub host: Box<[u8]>,
    pub path: Box<[u8]>,
    pub transport: Transport,
}
#[expect(missing_debug_implementations, reason = "startup refresh and saved tokens must never occur in traces")]
pub enum Initial {
    Refresh { generation: u64, token: Box<[u8]> },
    Saved { record: Box<[u8]> },
}
#[expect(missing_debug_implementations, reason = "configured credential values must never occur in traces")]
pub struct Config {
    pub account: u32,
    pub kind: AccountKind,
    pub endpoint: Endpoint,
    pub client: Box<[u8]>,
    pub initial: Initial,
}
#[expect(missing_debug_implementations, reason = "static git values must never occur in traces")]
pub struct Identity {
    pub account: u32,
    pub token: Box<[u8]>,
}
struct Candidate {
    record: SavedToken,
    expires: Time,
}
struct Row {
    kind: AccountKind,
    endpoint: Endpoint,
    client: Box<[u8]>,
    refresh: RefreshState,
    candidate: Option<Candidate>,
    initial: Account,
}
#[expect(
    missing_debug_implementations,
    reason = "refresh, access and pending rotated tokens must never occur in traces"
)]
pub struct Table {
    rows: Box<[Row]>,
    values: List<Value>,
}
impl Table {
    pub fn new(
        configs: Box<[Config]>,
        identities: Box<[Identity]>,
        now: Time,
        wall: Wall,
        limits: &Limits,
    ) -> Result<Table, Error> {
        let count = u32::try_from(configs.len()).ok().ok_or(Error::Limits)?;
        if count > limits.accounts || u32::try_from(identities.len()).ok().ok_or(Error::Limits)? > limits.identities {
            return Err(Error::Limits);
        }
        let capacity =
            limits.accounts.checked_mul(2).ok_or(Error::Limits)?.checked_add(limits.identities).ok_or(Error::Limits)?;
        if worst_case(limits).is_none() {
            return Err(Error::Limits);
        }
        let mut rows = List::with_capacity(count);
        let mut values = List::with_capacity(capacity);
        for config in configs {
            for row in &rows {
                let Row { refresh, .. } = row;
                if refresh.account == config.account {
                    return Err(Error::Account);
                }
            }
            let row = load(config, now, wall, limits, &mut values)?;
            assert!(rows.push(row).is_ok(), "configured account count");
        }
        for identity in identities {
            for row in &rows {
                if row.refresh.account == identity.account {
                    return Err(Error::Account);
                }
            }
            for value in &values {
                if value.account == identity.account {
                    return Err(Error::Account);
                }
            }
            let validation =
                TokenResponse { access_token: copy_of(&identity.token), refresh_token: None, expires_in: 1 };
            match document::encode_response(&validation, &limits.documents) {
                Ok(_) => {}
                Err(error) => return Err(Error::Document(error)),
            }
            assert!(
                values
                    .push(Value {
                        account: identity.account,
                        generation: 0,
                        expires: Time::from_nanos(u64::MAX),
                        token: identity.token,
                        account_id: Box::new([]),
                    })
                    .is_ok(),
                "configured static identity count"
            );
        }
        Ok(Table { rows: rows.into_boxed(), values })
    }
    /// Remaining validity is anchored at construction, matching `Domain::new`.
    #[must_use]
    pub fn metadata(&self) -> Box<[Account]> {
        let mut out = List::with_capacity(u32::try_from(self.rows.len()).expect("configured count"));
        for row in &self.rows {
            out.push(row.initial).expect("configured count");
        }
        out.into_boxed()
    }
    #[must_use]
    pub fn values(&self) -> &[Value] {
        self.values.as_slice()
    }
    #[must_use]
    pub fn endpoint(&self, account: u32) -> Option<&Endpoint> {
        Some(&self.row(account)?.endpoint)
    }
    pub fn validate_refresh(&self, account: u32, generation: u64) -> Result<(), Error> {
        let row = self.row(account).ok_or(Error::Account)?;
        if row.refresh.generation.checked_add(1) != Some(generation) {
            return Err(Error::Generation);
        }
        if row.candidate.is_some() {
            return Err(Error::Candidate);
        }
        Ok(())
    }
    pub fn request(&self, account: u32, generation: u64) -> Result<RefreshRequest, Error> {
        self.validate_refresh(account, generation)?;
        let row = self.row(account).expect("validated account");
        Ok(RefreshRequest { client_id: copy_of(&row.client), refresh_token: copy_of(&row.refresh.refresh_token) })
    }
    /// Capture the candidate's monotonic expiry once at response completion.
    pub fn prepare(
        &mut self,
        account: u32,
        generation: u64,
        response: TokenResponse,
        now: Time,
        wall: Wall,
        limits: &Limits,
    ) -> Result<Duration, Error> {
        let row = self.row_mut(account).ok_or(Error::Account)?;
        if row.candidate.is_some() {
            return Err(Error::Candidate);
        }
        if row.refresh.generation.checked_add(1) != Some(generation) {
            return Err(Error::Generation);
        }
        let record = match document::rotate(&row.refresh, &response, row.kind, wall, &limits.documents) {
            Ok(value) => value,
            Err(error) => return Err(Error::Document(error)),
        };
        let valid = record.remaining(wall);
        if valid == Duration::ZERO {
            return Err(Error::Document(document::DecodeError::Malformed));
        }
        let expires = now.checked_add(valid).ok_or(Error::Limits)?;
        row.candidate = Some(Candidate { record, expires });
        Ok(valid)
    }
    /// The external keeper owns these exact versioned bytes until its terminal.
    pub fn record(&self, account: u32, generation: u64, limits: &Limits) -> Result<Box<[u8]>, Error> {
        let candidate = self.row(account).ok_or(Error::Account)?.candidate.as_ref().ok_or(Error::Candidate)?;
        if candidate.record.generation != generation {
            return Err(Error::Generation);
        }
        match document::encode_record(&candidate.record, &limits.documents) {
            Ok(bytes) => Ok(bytes),
            Err(error) => Err(Error::Document(error)),
        }
    }
    pub fn candidate_valid(&self, account: u32, generation: u64, now: Time) -> Result<Duration, Error> {
        let candidate = self.row(account).ok_or(Error::Account)?.candidate.as_ref().ok_or(Error::Candidate)?;
        if candidate.record.generation != generation {
            return Err(Error::Generation);
        }
        Ok(candidate.expires.saturating_since(now))
    }
    /// Only a successful durable keeper terminal can install the candidate.
    pub fn saved(&mut self, account: u32, generation: u64, now: Time) -> Result<Duration, Error> {
        let row = self.row_mut(account).ok_or(Error::Account)?;
        let candidate = row.candidate.as_ref().ok_or(Error::Candidate)?;
        if candidate.record.generation != generation {
            return Err(Error::Generation);
        }
        let candidate = row.candidate.take().expect("checked candidate remains");
        let valid = candidate.expires.saturating_since(now);
        let SavedToken { account, generation, access_token, refresh_token, account_id, expires_at: _ } =
            candidate.record;
        row.refresh = RefreshState { account, generation, refresh_token };
        let value = Value {
            account,
            generation,
            expires: candidate.expires,
            token: access_token,
            account_id: account_id.unwrap_or_default(),
        };
        install(&mut self.values, value);
        Ok(valid)
    }
    pub fn expire(&mut self, now: Time) {
        for index in 0..self.values.len() {
            let value = self.values.get_mut(index).expect("bounded value index");
            if value.expires <= now {
                value.token = Box::new([]);
                value.account_id = Box::new([]);
            }
        }
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let mut next: Option<Time> = None;
        for value in &self.values {
            if !value.token.is_empty() && value.expires < Time::from_nanos(u64::MAX) {
                next = Some(match next {
                    Some(at) => at.min(value.expires),
                    None => value.expires,
                });
            }
        }
        next
    }
    #[expect(clippy::manual_find, reason = "bounded explicit account lookup without closures")]
    fn row(&self, account: u32) -> Option<&Row> {
        for row in &self.rows {
            if row.refresh.account == account {
                return Some(row);
            }
        }
        None
    }
    #[expect(clippy::manual_find, reason = "bounded explicit account lookup without closures")]
    fn row_mut(&mut self, account: u32) -> Option<&mut Row> {
        for row in &mut self.rows {
            if row.refresh.account == account {
                return Some(row);
            }
        }
        None
    }
}
fn install(values: &mut List<Value>, value: Value) {
    let mut previous = None;
    let mut oldest = None;
    for (index, held) in values.iter().enumerate() {
        if held.account == value.account {
            if let Some(first) = previous {
                let prior: &Value = values.get(first).expect("existing value index");
                oldest = Some(if prior.generation < held.generation {
                    first
                } else {
                    u32::try_from(index).expect("bounded values")
                });
            } else {
                previous = Some(u32::try_from(index).expect("bounded values"));
            }
        }
    }
    if let Some(index) = oldest {
        *values.get_mut(index).expect("held index") = value;
    } else {
        assert!(values.push(value).is_ok(), "two values per account and static identities were provisioned");
    }
}
fn load(config: Config, now: Time, wall: Wall, limits: &Limits, values: &mut List<Value>) -> Result<Row, Error> {
    let Config { account, kind, endpoint, client, initial } = config;
    for bytes in [&endpoint.host, &endpoint.path] {
        if bytes.is_empty() || u32::try_from(bytes.len()).ok().ok_or(Error::Limits)? > limits.endpoint_bytes {
            return Err(Error::Limits);
        }
        for &byte in bytes {
            if byte <= 32 || byte >= 127 {
                return Err(Error::Account);
            }
        }
    }
    if endpoint.path.first() != Some(&b'/')
        || (endpoint.transport == Transport::Loopback && !endpoint.address.ip().is_loopback())
    {
        return Err(Error::Account);
    }
    let (refresh, valid) = match initial {
        Initial::Refresh { generation, token } => (RefreshState { account, generation, refresh_token: token }, None),
        Initial::Saved { record } => {
            let mut saved = match document::decode_record(&record, &limits.documents) {
                Ok(value) => value,
                Err(error) => return Err(Error::Document(error)),
            };
            if saved.account != account {
                return Err(Error::Account);
            }
            if kind == AccountKind::ChatGpt {
                let claims = match document::read_claims(&saved.access_token, wall, &limits.documents) {
                    Ok(value) => value,
                    Err(error) => return Err(Error::Document(error)),
                };
                if saved.account_id.as_ref() != Some(&claims.account_id) {
                    return Err(Error::Account);
                }
                if let Some(at) = claims.expires_at {
                    saved.expires_at = saved.expires_at.min(at);
                }
            }
            let valid = saved.remaining(wall);
            let expires = now.checked_add(valid).ok_or(Error::Limits)?;
            let refresh = saved.refresh_state();
            assert!(
                values
                    .push(Value {
                        account,
                        generation: saved.generation,
                        expires,
                        token: saved.access_token,
                        account_id: saved.account_id.unwrap_or_default(),
                    })
                    .is_ok(),
                "one saved value per configured account"
            );
            (refresh, Some(valid))
        }
    };
    if refresh.generation == u64::MAX {
        return Err(Error::Generation);
    }
    let request = RefreshRequest { client_id: copy_of(&client), refresh_token: copy_of(&refresh.refresh_token) };
    match document::encode_request(&request, &limits.documents) {
        Ok(_) => {}
        Err(error) => return Err(Error::Document(error)),
    }
    Ok(Row {
        kind,
        endpoint,
        client,
        initial: Account { account, generation: refresh.generation, valid },
        refresh,
        candidate: None,
    })
}
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let slots = limits.accounts.checked_mul(2)?.checked_add(limits.identities)?;
    List::<Row>::worst_case(limits.accounts)?
        .checked_add(List::<Value>::worst_case(slots)?)?
        .checked_add(u64::from(limits.accounts).checked_mul(
            u64::from(limits.endpoint_bytes).checked_mul(2)?.checked_add(u64::from(limits.documents.client_bytes))?,
        )?)?
        .checked_add(
            u64::from(limits.accounts).checked_mul(
                u64::from(limits.documents.token_bytes)
                    .checked_mul(8)?
                    .checked_add(u64::from(limits.documents.record_bytes).checked_mul(2)?)?,
            )?,
        )?
        .checked_add(u64::from(limits.identities).checked_mul(u64::from(limits.documents.token_bytes))?)?
        .checked_add(document::worst_case(&limits.documents)?)
}

//! The accounts driver, its records in the secrets' store and secret-free
//! translation (llm.md, 5; store.md, 9).
use crate::{
    settings::{Files, Seeds},
    values::Values,
};
use alloc::boxed::Box;
use jig_domain_accounts as domain;
use jig_protocol_accounts::{installation, keys, moves, party, records, restart, table, translate};
use jig_protocol_config::{self as config, Path, Problems, What};
use jig_protocol_store as store;
use skein_io::{
    self as io,
    kernel::{Complete, Op, Submit},
};
use skein_lib::{CryptoRng, Env, List, Map, Queue, Secret, Time, Token, Wall};
use skein_oauth as oauth;
use skein_oauth_accounts as driver;
use temper_engine_domain as root;

const PREFIX: u64 = crate::tokens::ACCOUNTS;
/// The store's answer to the load of the sources' records; a replace is
/// answered under its source's number.
const SWEEP: Token = Token::new(PREFIX | (1_u64 << 34));
const MOVE_OUT: u64 = PREFIX | (1_u64 << 35);
const MOVE_IN: u64 = PREFIX | (1_u64 << 36);
const MOVE_BACK: u64 = PREFIX | (1_u64 << 37);
const MOVE_ADOPT: u64 = PREFIX | (1_u64 << 38);
const KEY_EXCHANGE: u64 = PREFIX | (1_u64 << 33);
#[derive(Debug)]
pub struct AccountsRuntime {
    /// LLM account sign-in endpoints and origins, keyed by provider number.
    pub providers: List<(u32, driver::Endpoint, List<driver::Origin>)>,
    /// Bounds of all prepared trust roots, supplied by their shell owner.
    pub trust_roots: u32,
    pub trust_root_bytes: u32,
}
/// The driver starts after the store supplies its durable installation identity (llm.md, 5.4).
enum Phase {
    Loading { crypto: CryptoRng },
    Running,
}
/// One durable erase per source; only sign-out continues to driver removal.
#[derive(Debug)]
enum Erasing {
    Revoked,
    SignOut { confirmed: bool },
    Removing,
}
#[expect(missing_debug_implementations, reason = "the accounts component holds token records")]
pub struct Accounts {
    component: Option<driver::Component>,
    phase: Phase,
    env: Env<driver::Limits>,
    providers: Map<u32, driver::Account>,
    required: Map<u32, Box<[u8]>>,
    erasing: Map<u32, Erasing>,
    records: records::Records,
    keys: keys::Keys,
    moves: moves::Moves,
    source_providers: Map<u32, u32>,
    move_out: Queue<moves::Next>,
    inventory: restart::Inventory,
    key_out: Queue<keys::Next>,
    key_events: Queue<domain::FromComponent>,
    forgotten: Queue<u32>,
    early: Queue<domain::Request>,
    pub(crate) io: Queue<io::Request>,
    above: Queue<driver::Event>,
    /// Empty: each source's keeper is the owner, so the driver files nothing.
    files: Queue<driver::FileLower>,
    /// Aborted: the store is closing, and takes no more records.
    aborting: bool,
    closed: bool,
}
fn problem(path: Path, what: What) -> Problems {
    let mut found = Problems::empty();
    found.push(path, what);
    found
}
impl Files {
    pub(crate) fn contents(&self, path: &[u8]) -> Option<&[u8]> {
        for (name, value) in &self.entries {
            if name.as_ref() == path {
                return Some(value.expose());
            }
        }
        None
    }
}
fn registration_input(
    configured: &config::llm::Registration,
    endpoint_transport: &driver::Transport,
) -> party::Registration {
    party::Registration {
        issuer: configured.issuer.clone(),
        client: configured.client.clone(),
        scopes: configured.scopes.clone(),
        name_hint: configured.name_hint.clone(),
        authorization: configured.authorization.clone(),
        token: configured.token.clone(),
        revocation: configured.revocation.clone(),
        keys: configured.keys.clone(),
        endpoint_transport: match endpoint_transport {
            driver::Transport::Tls { server_name, trust } => {
                driver::Transport::Tls { server_name: server_name.clone(), trust: trust.clone() }
            }
            driver::Transport::Plaintext => driver::Transport::Plaintext,
        },
        must_grant: configured.must_grant.clone(),
        resource: configured.resource.clone(),
        redirect_path: configured.redirect_path.clone(),
        redirect_port: configured.redirect_port,
    }
}
/// A provider registration copied without a source record.
fn unkept(account: &driver::Account) -> driver::Account {
    match account {
        driver::Account::HandedIn => driver::Account::HandedIn,
        driver::Account::SignIn { registration, endpoint, keeper: _, shared, origins } => driver::Account::SignIn {
            registration: oauth::Registration {
                authorization_url: registration.authorization_url.clone(),
                token_endpoint: registration.token_endpoint.clone(),
                client_id: registration.client_id.clone(),
                redirect_uri: registration.redirect_uri.clone(),
                scope: registration.scope.clone(),
                wire: registration.wire,
                client_secret: registration.client_secret.clone(),
                pkce_for_confidential: registration.pkce_for_confidential,
                metadata_claim: registration.metadata_claim.clone(),
                terms: registration.terms.clone(),
            },
            endpoint: driver::Endpoint {
                address: endpoint.address,
                transport: match &endpoint.transport {
                    driver::Transport::Tls { server_name, trust } => {
                        driver::Transport::Tls { server_name: server_name.clone(), trust: trust.clone() }
                    }
                    driver::Transport::Plaintext => driver::Transport::Plaintext,
                },
            },
            keeper: driver::Keeper::Owner { kept: None },
            shared: *shared,
            origins: copy_origins(origins),
        },
    }
}
fn provider_templates(
    config: &config::Engine,
    runtime: Option<&mut AccountsRuntime>,
    limits: &driver::Limits,
    crypto: &CryptoRng,
    installation_id: &[u8; 16],
) -> Result<Map<u32, driver::Account>, Problems> {
    let mut providers = Map::with_capacity(u32::try_from(config.llm.providers.len()).expect("provider count"));
    if let Some(runtime) = runtime {
        for (number, endpoint, origins) in
            ::core::mem::replace(&mut runtime.providers, List::with_capacity(0)).into_boxed()
        {
            let mut registration = None;
            for provider in &config.llm.providers {
                if provider.number == number {
                    registration = provider.sign_in.as_ref();
                    break;
                }
            }
            let registration = registration
                .ok_or(problem(Path::root().member(b"engine").member(b"llm").member(b"providers"), What::Refers))?;
            if (runtime.trust_roots == 0 || runtime.trust_root_bytes == 0) && has_tls(&endpoint, &origins) {
                return Err(problem(Path::root().member(b"engine").member(b"llm").member(b"providers"), What::Bound));
            }
            let account = party::party_account(
                &registration_input(registration, &endpoint.transport),
                endpoint,
                origins,
                installation_id,
            );
            let mut check = List::with_capacity(1);
            check
                .push(unkept(&account))
                .or(Err(problem(Path::root().member(b"engine").member(b"llm").member(b"providers"), What::Bound)))?;
            let _checked = driver::Component::new(check, limits, crypto.clone())
                .or(Err(problem(Path::root().member(b"engine").member(b"llm").member(b"providers"), What::Form)))?;
            providers
                .insert(number, account)
                .or(Err(problem(Path::root().member(b"engine").member(b"llm").member(b"providers"), What::Bound)))?;
        }
    }
    for provider in &config.llm.providers {
        if provider.sign_in.is_some() && !providers.contains_key(&provider.number) {
            return Err(problem(Path::root().member(b"engine").member(b"llm").member(b"providers"), What::Refers));
        }
    }
    Ok(providers)
}
impl Accounts {
    /// Takes the moved record once, for the exchange that asked for it.
    pub fn take_moved(&mut self, source: u32) -> Option<Secret> {
        self.moves.take(source)
    }

    /// Checks public provider registrations; restored sources are admitted after the store opens.
    pub fn new(
        application: Box<[u8]>,
        config: &config::Engine,
        seeds: &Seeds,
        mut runtime: Option<AccountsRuntime>,
        room: u32,
        secrets: &skein_kv::Limits,
    ) -> Result<Self, Problems> {
        let limits = config.limits.driver;
        if limits.client.document.record_bytes > secrets.value {
            return Err(problem(Path::root().member(b"engine").member(b"llm").member(b"providers"), What::Bound));
        }
        let crypto = CryptoRng::new(&seeds.crypto, 1)
            .ok_or(problem(Path::root().member(b"engine").member(b"llm").member(b"providers"), What::Bound))?;
        let providers = provider_templates(config, runtime.as_mut(), &limits, &crypto, &seeds.installation)?;
        let inventory = restart::Inventory::new(limits.accounts, secrets.page.rows);
        Ok(Self {
            component: None,
            phase: Phase::Loading { crypto },
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            providers,
            required: Map::with_capacity(limits.accounts),
            erasing: Map::with_capacity(limits.accounts),
            records: records::Records::new(limits.accounts),
            keys: keys::Keys::new(limits.accounts, limits.client.document.token_bytes),
            moves: moves::Moves::new(application, limits.accounts),
            source_providers: Map::with_capacity(limits.accounts),
            move_out: Queue::with_capacity(room),
            inventory,
            key_out: Queue::with_capacity(room),
            key_events: Queue::with_capacity(room),
            forgotten: Queue::with_capacity(room),
            early: Queue::with_capacity(room),
            io: Queue::with_capacity(room),
            above: Queue::with_capacity(room),
            files: Queue::with_capacity(room),
            aborting: false,
            closed: false,
        })
    }
    pub(crate) fn down(&mut self, request: driver::Request) {
        if let Some(component) = &mut self.component {
            component.down(&self.env, request, &mut self.above, &mut self.io, &mut self.files);
        }
    }
    fn down_room(&self) -> bool {
        self.above.room() >= driver::MAX_OUT_DOWN.above
            && self.io.room() >= driver::MAX_OUT_DOWN.io
            && self.files.room() >= driver::MAX_OUT_DOWN.files
    }
    #[must_use]
    pub fn up_room(&self) -> bool {
        self.above.room() >= driver::MAX_OUT_UP.above && self.io.room() >= driver::MAX_OUT_UP.io
    }
    #[must_use]
    pub fn batch_room(&self, batch: u32) -> bool {
        self.above.room() >= batch.checked_mul(driver::MAX_OUT_DOWN.above).expect("account above room")
            && self.io.room() >= batch.checked_mul(driver::MAX_OUT_DOWN.io).expect("account io room")
            && self.files.room() >= batch.checked_mul(driver::MAX_OUT_DOWN.files).expect("account files room")
            && self.move_out.room() >= batch
            && self.key_out.room() >= batch
            && self.key_events.room() >= batch
            && self.forgotten.room() >= batch
            && self.early.room() >= batch
    }
    /// The domain's request: to the driver once it runs, held until then.
    pub fn request(&mut self, request: domain::Request) {
        match request {
            domain::Request::Component(_) | domain::Request::Signing(_) => {
                self.early.push(request);
            }
            domain::Request::Programs { .. }
            | domain::Request::CheckProgram { .. }
            | domain::Request::SignOutProgram { .. }
            | domain::Request::Granted { .. }
            | domain::Request::Save { .. }
            | domain::Request::Erase { .. }
            | domain::Request::Answer { .. }
            | domain::Request::Closed => (),
        }
    }
    fn dispatch(&mut self, request: domain::Request, store: &mut crate::store::Store, values: &mut Values) {
        match request {
            domain::Request::Component(request) => match request {
                domain::ToComponent::Want { account } if self.keys.contains(account) => {
                    if let Some(event) = self.keys.want(account, self.env.now) {
                        self.key_events.push(event);
                    }
                }
                domain::ToComponent::Rejected { account, .. } | domain::ToComponent::Release { account }
                    if self.keys.contains(account) => {}
                domain::ToComponent::Want { account } => {
                    self.ready(account, false);
                    self.down(driver::Request::Grant { account });
                }
                domain::ToComponent::Close => {
                    self.moves.close();
                    self.down(driver::Request::Close);
                }
                other @ (domain::ToComponent::Rejected { .. } | domain::ToComponent::Release { .. }) => {
                    self.down(translate::down(other));
                }
            },
            domain::Request::Signing(signing) => self.signing(translate::down_signing(signing), store, values),
            domain::Request::Programs { .. }
            | domain::Request::CheckProgram { .. }
            | domain::Request::SignOutProgram { .. }
            | domain::Request::Granted { .. }
            | domain::Request::Save { .. }
            | domain::Request::Erase { .. }
            | domain::Request::Answer { .. }
            | domain::Request::Closed => (),
        }
    }
    fn signing(&mut self, signing: translate::Down, store: &mut crate::store::Store, values: &mut Values) {
        match signing {
            translate::Down::Restore { source, provider, kind } => {
                self.inventory.name(source);
                let value = match store.secrets(
                    Token::new(PREFIX | u64::from(source)),
                    store::Secrets::Get { owner: store::SecretOwner::Source, number: u64::from(source) },
                ) {
                    Some(store::Event::Secret { value, .. }) => value,
                    None => return,
                    Some(other) => unreachable!("Source Get answers with its value: {other:?}"),
                };
                match kind {
                    domain::Kind::Key => self
                        .keys
                        .restore(source, value, &mut values.accounts)
                        .expect("restored LLM key source fits table"),
                    domain::Kind::SignIn => self.restore_signed_in(source, provider, value),
                    domain::Kind::Program | domain::Kind::None => (),
                }
            }
            translate::Down::Withdraw { source, display } => {
                self.ready(source, false);
                let next = self.moves.withdraw(source, display);
                self.move_out.push(next);
            }
            translate::Down::Return { source } => {
                let next = self.moves.returning(source);
                self.move_out.push(next);
            }
            translate::Down::Refresh { source } => {
                self.ready(source, true);
            }
            translate::Down::Open { source, carried } => {
                let next = self.moves.open(source, carried);
                self.move_out.push(next);
            }
            translate::Down::Adopt { source, carried } => {
                let next = self.moves.adopt(source, carried, &self.env.limits.client.document);
                self.move_out.push(next);
            }
            translate::Down::Restored => self.inventory.restored(),
            translate::Down::Keep { source, carried, replaces } => {
                self.inventory.name(source);
                let next = self.keys.keep(source, carried, replaces);
                self.key_out.push(next);
            }
            translate::Down::Forget { source } => match self.moves.forget(source) {
                moves::Forget::Pending => (),
                moves::Forget::Withdrawn => {
                    self.records.remove(source);
                    let _required = self.required.remove(&source);
                    let _provider = self.source_providers.remove(&source);
                    self.inventory.forget(source);
                    self.forgotten.push(source);
                }
                moves::Forget::Source => {
                    if self.required.contains_key(&source) {
                        self.down(driver::Request::Remove { account: source });
                    }
                    self.keys.forget(source);
                    self.inventory.forget(source);
                    self.forgotten.push(source);
                }
            },
            translate::Down::CancelSignIn { source } => self.down(driver::Request::Cancel { account: source }),
            translate::Down::SignOut { source } => {
                self.ready(source, false);
                self.down(driver::Request::SignOut { account: source });
            }
            translate::Down::SignIn { source, provider } => {
                let Some(template) = self.providers.get(&provider) else {
                    self.above.push(driver::Event::Refused {
                        account: source,
                        asked: driver::Asked::Add,
                        why: driver::Refusal::Source,
                    });
                    return;
                };
                let account = unkept(template);
                let required = match &account {
                    driver::Account::SignIn { registration, .. } => match &registration.terms {
                        Some(terms) => terms.required_scopes.clone(),
                        None => None,
                    },
                    driver::Account::HandedIn => None,
                }
                .unwrap_or_default();
                let before = self.above.len();
                self.component.as_mut().expect("running driver").add(&self.env, source, account, &mut self.above);
                if self.above.len() != before {
                    return;
                }
                self.inventory.name(source);
                self.source_providers.insert(source, provider).expect("admitted source");
                self.required.insert(source, required).expect("driver admitted account");
                self.records.admit(source).expect("driver admitted account");
                self.down(driver::Request::SignIn { account: source, subject: None });
            }
        }
    }
    pub fn clock(&mut self, now: Time, wall: Wall) {
        self.env.now = now;
        self.env.wall = wall;
    }
    pub fn up(&mut self, event: io::Event) {
        if let Some(component) = &mut self.component {
            component.up(&self.env, event, &mut self.above, &mut self.io, &mut self.files);
        }
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        (self.aborting || (!self.inventory.active() && !self.keys.pending()))
            && self.move_out.is_empty()
            && self.key_out.is_empty()
            && self.key_events.is_empty()
            && self.forgotten.is_empty()
            && self.closed
            && self.io.is_empty()
            && self.above.is_empty()
            && self.files.is_empty()
            && self.early.is_empty()
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        match &self.component {
            Some(component) => component.next_deadline(),
            None => None,
        }
    }
    #[must_use]
    pub fn work_pending(&self, now: Time) -> bool {
        let starting = match &self.phase {
            Phase::Loading { .. } => true,
            Phase::Running => !self.early.is_empty(),
        };
        starting
            || self.inventory.work_pending()
            || !self.move_out.is_empty()
            || !self.key_out.is_empty()
            || !self.key_events.is_empty()
            || !self.forgotten.is_empty()
            || !self.above.is_empty()
            || !self.io.is_empty()
            || !self.files.is_empty()
            || match &self.component {
                Some(component) => component.has_work(),
                None => false,
            }
            || match self.next_deadline() {
                Some(deadline) => deadline <= now,
                None => false,
            }
    }
    pub fn abort(&mut self) {
        self.aborting = true;
        self.moves.close();
        for _ in 0..self.move_out.len() {
            let _unasked = self.move_out.pop();
        }
        if self.component.is_some() {
            self.down(driver::Request::Abort);
        } else {
            self.closed = true;
        }
        self.phase = Phase::Running;
        for _ in 0..self.early.len() {
            let _unasked = self.early.pop();
        }
        for _ in 0..self.key_out.len() {
            let _unkept = self.key_out.pop();
        }
        for _ in 0..self.key_events.len() {
            let _unheard = self.key_events.pop();
        }
        for _ in 0..self.forgotten.len() {
            let _unforgotten = self.forgotten.pop();
        }
    }
    pub fn iterate(
        &mut self,
        now: Time,
        wall: Wall,
        values: &mut Values,
        events: &mut Queue<root::Event>,
        store: &mut crate::store::Store,
    ) {
        self.clock(now, wall);
        self.answers(store, events);
        self.start(store, values);
        self.move_steps(events, store);
        self.key_steps(values, events, store);
        self.sweep(store);
        self.fire_driver();
        for _ in 0..self.above.len() {
            if events.room() == 0 || !store.room() || !self.down_room() || self.move_out.room() == 0 {
                break;
            }
            let event = self.above.pop().expect("account events");
            let number = event_account(&event);
            let required = match number {
                Some(number) => self.required.get(&number),
                None => None,
            };
            let required = match required {
                Some(value) => &**value,
                None => b"",
            };
            let event = match self.records.up(event, now, required) {
                translate::Up::Granted { mut event, account, generation, value, expires } => {
                    let granted_form = Self::granted(account, generation, value, expires, values);
                    match &mut event {
                        domain::FromComponent::Granted { form, .. } => *form = granted_form,
                        domain::FromComponent::Expiring { .. }
                        | domain::FromComponent::KeepFailed { .. }
                        | domain::FromComponent::Failed { .. }
                        | domain::FromComponent::Closed => unreachable!("a lent value accompanies its grant"),
                    }
                    event
                }
                translate::Up::Child(event) => event,
                translate::Up::Revoked { source } => {
                    values.accounts.remove(source);
                    self.erase(source, Erasing::Revoked, events, store);
                    domain::FromComponent::Failed { account: source, failure: domain::Failure::SignInAgain }
                }
                translate::Up::Visited { source, address } => {
                    events.push(root::Event::Account(domain::Event::Visited { source, address }));
                    continue;
                }
                translate::Up::SignedIn { source, subject, display } => {
                    events.push(root::Event::Account(domain::Event::SignedIn {
                        source,
                        subject,
                        display,
                        declined: false,
                    }));
                    continue;
                }
                translate::Up::SignInFailed { source, failure } => {
                    events.push(root::Event::Account(domain::Event::SignInFailed { source, failure }));
                    // Every failed account sign-in gives its driver place back.
                    if self.required.contains_key(&source) {
                        self.down(driver::Request::SignOut { account: source });
                    }
                    continue;
                }
                translate::Up::SignedOut { source, confirmed } => {
                    self.erase(source, Erasing::SignOut { confirmed }, events, store);
                    continue;
                }
                translate::Up::Removed { source } => {
                    self.removed(source, values);
                    continue;
                }
                translate::Up::Keep { account, record } => {
                    if self.aborting {
                        continue;
                    }
                    match self.records.keep(account, record.0, &self.env.limits.client.document) {
                        records::Next::Driver(request) => self.down(request),
                        records::Next::Store(keep) => replace(store, keep),
                    }
                    continue;
                }
            };
            match &event {
                domain::FromComponent::Granted { .. }
                | domain::FromComponent::Expiring { .. }
                | domain::FromComponent::Failed { .. }
                | domain::FromComponent::KeepFailed { .. } => {}
                domain::FromComponent::Closed => self.closed = true,
            }
            events.push(root::Event::Account(domain::Event::Component(event)));
        }
        assert!(self.files.is_empty(), "owner keepers file nothing");
        values.accounts.expire(now);
        if let Some(component) = &mut self.component {
            component.reclaim();
        }
    }
    fn removed(&mut self, source: u32, values: &mut Values) {
        if let Some(next) = self.moves.removed(source) {
            self.move_out.push(next);
            return;
        }
        let _erase = self.erasing.remove(&source);
        let _required = self.required.remove(&source);
        let _provider = self.source_providers.remove(&source);
        self.records.remove(source);
        self.inventory.forget(source);
        values.accounts.remove(source);
    }
    fn fire_driver(&mut self) {
        if let Some(component) = &mut self.component
            && self.above.room() >= 3
            && self.io.room() >= 3
            && self.files.room() >= 1
        {
            component.fire(&self.env, &mut self.above, &mut self.io, &mut self.files);
        }
    }
    fn restore_signed_in(&mut self, source: u32, provider: u32, value: Option<Secret>) {
        let Some(template) = self.providers.get(&provider) else {
            self.key_events.push(domain::FromComponent::Failed {
                account: source,
                failure: domain::Failure::Unloaded { at: domain::Place::File, why: domain::Unread::Unreadable },
            });
            return;
        };
        let account =
            party::restore(unkept(template), source, value, &mut self.records, &self.env.limits.client.document);
        let required = match &account {
            driver::Account::SignIn { registration, .. } => match &registration.terms {
                Some(terms) => terms.required_scopes.clone().unwrap_or_default(),
                None => Box::default(),
            },
            driver::Account::HandedIn => unreachable!("party sources use provider registrations"),
        };
        self.required.insert(source, required).expect("restored source fits driver");
        self.source_providers.insert(source, provider).expect("restored source fits driver");
        self.moves.stage(source, account);
    }
    fn ready(&mut self, source: u32, refresh: bool) {
        if let Some(account) = self.moves.ready(source, refresh) {
            self.component.as_mut().expect("running driver").add(&self.env, source, account, &mut self.above);
        }
    }
    fn move_steps(&mut self, events: &mut Queue<root::Event>, store: &mut crate::store::Store) {
        if self.aborting {
            return;
        }
        for _ in 0..self.move_out.len() {
            if !store.room() || events.room() == 0 || !self.down_room() {
                break;
            }
            match self.move_out.pop().expect("move steps") {
                moves::Next::Driver(request) => self.down(request),
                moves::Next::Child(event) => events.push(root::Event::Account(event)),
                moves::Next::Store { source, request } => {
                    let (prefix, request) = match request {
                        moves::Store::ReadOut => (
                            MOVE_OUT,
                            store::Secrets::Get { owner: store::SecretOwner::Source, number: u64::from(source) },
                        ),
                        moves::Store::ReadBack => (
                            MOVE_BACK,
                            store::Secrets::Get { owner: store::SecretOwner::Source, number: u64::from(source) },
                        ),
                        moves::Store::ReadIn { carried } => {
                            (MOVE_IN, store::Secrets::Get { owner: store::SecretOwner::Application, number: carried })
                        }
                        moves::Store::AdoptValue { carried, holds, value } => (
                            MOVE_ADOPT,
                            store::Secrets::AdoptValue {
                                carried,
                                holds,
                                owner: store::SecretOwner::Source,
                                number: u64::from(source),
                                value,
                                replaces: None,
                            },
                        ),
                    };
                    if let Some(event) = store.secrets(Token::new(prefix | u64::from(source)), request) {
                        self.move_answer(event, events);
                    }
                }
            }
        }
    }
    fn move_answer(&mut self, event: store::Event, events: &mut Queue<root::Event>) {
        if self.aborting {
            return;
        }
        match event {
            store::Event::Secret { owner, value } if owner.raw() & MOVE_OUT == MOVE_OUT => {
                let source = move_source(owner);
                let provider = self.source_providers.get(&source).expect("withdrawn source provider");
                let registration = self.providers.get(provider).expect("source provider");
                let next = self.moves.read(source, value, registration, &self.env.limits.client.document);
                self.move_out.push(next);
            }
            store::Event::Secret { owner, value } if owner.raw() & MOVE_BACK == MOVE_BACK => {
                let source = move_source(owner);
                let provider = *self.source_providers.get(&source).expect("returning source provider");
                self.restore_signed_in(source, provider, value);
            }
            store::Event::Secret { owner, value } => {
                let next =
                    self.moves.opened(move_source(owner), value, &self.providers, &self.env.limits.client.document);
                self.move_out.push(next);
            }
            store::Event::Kept { owner } => {
                let source = move_source(owner);
                if let Some(adoption) = self.moves.adopted(source, true) {
                    self.inventory.name(source);
                    self.restore_signed_in(source, adoption.provider, Some(adoption.value));
                    events.push(root::Event::Account(domain::Event::Adopted { source, kept: true }));
                }
            }
            store::Event::Stale { owner } => {
                let source = move_source(owner);
                let _unkept = self.moves.adopted(source, false);
                events.push(root::Event::Account(domain::Event::Adopted { source, kept: false }));
            }
            store::Event::Site { .. }
            | store::Event::Patched { .. }
            | store::Event::Opened { .. }
            | store::Event::OpenRefused { .. }
            | store::Event::Committed { .. }
            | store::Event::Failed { .. }
            | store::Event::Restored { .. }
            | store::Event::Loaded { .. }
            | store::Event::Read { .. }
            | store::Event::Got { .. }
            | store::Event::Turns { .. }
            | store::Event::Unread { .. }
            | store::Event::Carried { .. }
            | store::Event::Erased { .. }
            | store::Event::SecretsLoaded { .. }
            | store::Event::Corrupt { .. } => unreachable!("move exchange answers read or adoption"),
        }
    }
    fn sweep_loaded(&mut self, rows: Box<[store::SecretRow]>, next: Option<u64>) {
        for row in rows {
            if row.owner == store::SecretOwner::Source {
                self.inventory.row(row.number);
            }
        }
        self.inventory.loaded(next);
    }
    fn sweep(&mut self, store: &mut crate::store::Store) {
        if self.aborting || !store.room() {
            return;
        }
        let Some(step) = self.inventory.step() else { return };
        let request = match step {
            restart::Sweep::Load { after } => store::Secrets::Load { owner: store::SecretOwner::Source, after },
            restart::Sweep::Erase { number } => store::Secrets::Erase { owner: store::SecretOwner::Source, number },
        };
        match store.secrets(SWEEP, request) {
            Some(store::Event::SecretsLoaded { rows, next, .. }) => self.sweep_loaded(rows, next),
            Some(store::Event::Erased { .. }) => self.inventory.erased(),
            None => (),
            Some(other) => unreachable!("Source sweep answers load or erase: {other:?}"),
        }
    }
    fn key_steps(&mut self, values: &mut Values, events: &mut Queue<root::Event>, store: &mut crate::store::Store) {
        if self.aborting {
            return;
        }
        for _ in 0..self.forgotten.len() {
            if !store.room() {
                break;
            }
            let source = self.forgotten.pop().expect("forgotten LLM key");
            values.accounts.remove(source);
            let _later = store.secrets(
                Token::new(PREFIX | u64::from(source)),
                store::Secrets::Erase { owner: store::SecretOwner::Source, number: u64::from(source) },
            );
        }
        for _ in 0..self.key_out.len() {
            if !store.room() || events.room() == 0 {
                break;
            }
            match self.key_out.pop().expect("LLM key steps") {
                keys::Next::Store { exchange, request } => {
                    let owner = Token::new(KEY_EXCHANGE | exchange.raw());
                    let request = match request {
                        keys::Store::Get { carried } => {
                            store::Secrets::Get { owner: store::SecretOwner::Application, number: carried }
                        }
                        keys::Store::Adopt { carried, source, replaces } => store::Secrets::Adopt {
                            carried,
                            owner: store::SecretOwner::Source,
                            number: u64::from(source),
                            replaces,
                        },
                    };
                    if let Some(store::Event::Secret { value, .. }) = store.secrets(owner, request)
                        && let Some(next) = self.keys.read(exchange, value)
                    {
                        self.key_out.push(next);
                    }
                }
                keys::Next::Child(event) => {
                    match &event {
                        domain::FromComponent::KeepFailed { account, .. } => {
                            if !self.keys.contains(*account) {
                                self.inventory.forget(*account);
                            }
                        }
                        domain::FromComponent::Granted { .. }
                        | domain::FromComponent::Expiring { .. }
                        | domain::FromComponent::Failed { .. }
                        | domain::FromComponent::Closed => (),
                    }
                    events.push(root::Event::Account(domain::Event::Component(event)));
                }
                keys::Next::Granted { event, source, generation, value } => {
                    values
                        .accounts
                        .granted(source, generation, value, Time::from_nanos(u64::MAX))
                        .expect("admitted LLM key fits table");
                    events.push(root::Event::Account(domain::Event::Component(event)));
                }
            }
        }
        for _ in 0..self.key_events.len() {
            if events.room() == 0 {
                break;
            }
            let event = self.key_events.pop().expect("LLM key grant events");
            events.push(root::Event::Account(domain::Event::Component(event)));
        }
    }
    fn granted(account: u32, generation: u64, value: table::Value, expires: Time, values: &mut Values) -> domain::Form {
        let form = translate::form(&value.scheme);
        values
            .accounts
            .granted(account, generation, value, expires)
            .expect("startup checks token and account id bounds");
        form
    }
    fn erase(
        &mut self,
        source: u32,
        reason: Erasing,
        events: &mut Queue<root::Event>,
        store: &mut crate::store::Store,
    ) {
        if let Some(pending) = self.erasing.get_mut(&source) {
            match pending {
                Erasing::Revoked => match reason {
                    Erasing::SignOut { confirmed } => *pending = Erasing::SignOut { confirmed },
                    Erasing::Revoked | Erasing::Removing => (),
                },
                Erasing::SignOut { .. } | Erasing::Removing => (),
            }
            return;
        }
        self.erasing.insert(source, reason).expect("one erase per driver account");
        if let Some(store::Event::Erased { owner }) = store.secrets(
            Token::new(PREFIX | u64::from(source)),
            store::Secrets::Erase { owner: store::SecretOwner::Source, number: u64::from(source) },
        ) {
            self.erased(source, owner, events);
        }
    }
    fn erased(&mut self, number: u32, _owner: Token, events: &mut Queue<root::Event>) {
        let Some(pending) = self.erasing.remove(&number) else { return };
        match pending {
            Erasing::Revoked => self.records.erased(number),
            Erasing::SignOut { confirmed } => {
                self.records.erased(number);
                events.push(root::Event::Account(domain::Event::SignedOut { source: number, confirmed }));
                if self.required.contains_key(&number) {
                    self.erasing.insert(number, Erasing::Removing).expect("the removed erase's place");
                    self.down(driver::Request::Remove { account: number });
                }
            }
            Erasing::Removing => {
                self.erasing.insert(number, Erasing::Removing).expect("the removed erase's place");
            }
        }
    }
    /// The store's answers: the records it holds, and whether each write was
    /// kept (store.md 9).
    fn answers(&mut self, store: &mut crate::store::Store, events: &mut Queue<root::Event>) {
        for _ in 0..store.sources.len() {
            if !self.down_room() || events.room() == 0 || self.key_out.room() == 0 || self.move_out.room() == 0 {
                break;
            }
            match store.sources.pop().expect("source answers") {
                event @ store::Event::Secret { .. } => self.move_answer(event, events),
                event @ (store::Event::Kept { owner } | store::Event::Stale { owner })
                    if owner.raw() & MOVE_ADOPT == MOVE_ADOPT =>
                {
                    self.move_answer(event, events);
                }
                store::Event::SecretsLoaded { owner: SWEEP, rows, next } => self.sweep_loaded(rows, next),
                store::Event::SecretsLoaded { .. } => unreachable!("only the source inventory loads pages"),
                store::Event::Kept { owner } if owner.raw() & KEY_EXCHANGE == KEY_EXCHANGE => {
                    if !self.aborting
                        && let Some(next) = self.keys.kept(Token::new(owner.raw() & !KEY_EXCHANGE), true, self.env.now)
                    {
                        self.key_out.push(next);
                    }
                }
                store::Event::Stale { owner } if owner.raw() & KEY_EXCHANGE == KEY_EXCHANGE => {
                    if !self.aborting
                        && let Some(next) = self.keys.kept(Token::new(owner.raw() & !KEY_EXCHANGE), false, self.env.now)
                    {
                        self.key_out.push(next);
                    }
                }
                store::Event::Kept { owner } => {
                    if let Some(request) = self.records.kept(source(owner)) {
                        self.down(request);
                    }
                }
                store::Event::Stale { owner } => {
                    if let Some(request) = self.records.stale(source(owner)) {
                        self.down(request);
                    }
                }
                store::Event::Erased { owner: SWEEP } => self.inventory.erased(),
                store::Event::Erased { owner } => self.erased(source(owner), owner, events),
                store::Event::Carried { .. } => {
                    unreachable!("the accounts only load and replace their records")
                }
                store::Event::Site { .. }
                | store::Event::Patched { .. }
                | store::Event::Opened { .. }
                | store::Event::OpenRefused { .. }
                | store::Event::Committed { .. }
                | store::Event::Failed { .. }
                | store::Event::Restored { .. }
                | store::Event::Loaded { .. }
                | store::Event::Read { .. }
                | store::Event::Got { .. }
                | store::Event::Turns { .. }
                | store::Event::Unread { .. }
                | store::Event::Corrupt { .. } => {
                    unreachable!("the store hands the accounts only their secrets' answers")
                }
            }
        }
    }
    /// The durable installation identity replaces this start's draw before any restore or sign-in.
    fn opened(&mut self, kept: &[u8; 16]) {
        let Phase::Loading { crypto } = ::core::mem::replace(&mut self.phase, Phase::Running) else {
            unreachable!("loading");
        };
        let component = driver::Component::new(List::with_capacity(0), &self.env.limits, crypto)
            .expect("startup checked driver bounds");
        // The stored installation id, rather than this start's draw, goes on account sign-ins.
        let mut after = 0;
        for _ in 0..self.providers.len() {
            let Some((&number, _)) = self.providers.range(after..).next() else { break };
            if let Some(driver::Account::SignIn { registration, .. }) = self.providers.get_mut(&number)
                && let Some(terms) = &mut registration.terms
            {
                terms.parameters.first_mut().expect("installation parameter").value = installation::urn(kept);
            }
            let Some(next) = number.checked_add(1) else { break };
            after = next;
        }
        self.component = Some(component);
    }
    /// Wait for the store, then route the child's restored sources and runtime requests.
    fn start(&mut self, store: &mut crate::store::Store, values: &mut Values) {
        match &mut self.phase {
            Phase::Loading { .. } => {
                if store.opened {
                    self.opened(&store.installation);
                }
            }
            Phase::Running => {
                for _ in 0..self.early.len() {
                    if !self.down_room()
                        || !store.room()
                        || !self.move_out.is_empty()
                        || self.key_out.room() == 0
                        || self.key_events.room() == 0
                        || self.forgotten.room() == 0
                    {
                        break;
                    }
                    let request = self.early.pop().expect("early requests");
                    self.dispatch(request, store, values);
                }
            }
        }
    }
}
/// Asks the store to keep a source's record in place of the one it holds.
fn replace(store: &mut crate::store::Store, keep: records::Keep) {
    let records::Keep { account, value, replaces } = keep;
    let _later = store.secrets(
        Token::new(PREFIX | u64::from(account)),
        store::Secrets::Replace { owner: store::SecretOwner::Source, number: u64::from(account), value, replaces },
    );
}
fn move_source(owner: Token) -> u32 {
    u32::try_from(owner.raw() & u64::from(u32::MAX)).expect("source number")
}
fn source(owner: Token) -> u32 {
    u32::try_from(owner.raw() & !(15_u64 << 60)).expect("a source's number")
}
pub(crate) fn earliest(left: Option<Time>, right: Option<Time>) -> Option<Time> {
    match right {
        Some(right) => Some(match left {
            Some(left) => left.min(right),
            None => right,
        }),
        None => left,
    }
}

#[derive(Debug)]
pub(crate) struct Kernel {
    routes: Map<Token, (Token, Option<Token>)>,
    retired: Map<Token, Token>,
    next: u64,
}
impl Kernel {
    pub fn new(room: u32) -> Self {
        Self { routes: Map::with_capacity(room), retired: Map::with_capacity(room), next: 1 }
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty() && self.retired.is_empty()
    }
    pub fn publish(&mut self, source: &mut Queue<Submit>, target: &mut Queue<Submit>) {
        for _ in 0..source.len() {
            if target.room() == 0 || self.routes.len() == self.routes.capacity() {
                break;
            }
            let mut submit = source.pop().expect("kernel submissions");
            let mut cancel = None;
            if let Op::Cancel { target } = &mut submit.kind {
                cancel = Some(*target);
                let mut found = None;
                for (external, (internal, _)) in &self.routes {
                    if *internal == *target {
                        found = Some(*external);
                        break;
                    }
                }
                *target = match found {
                    Some(external) => external,
                    None => self.retired.remove(target).expect("cancel names submitted operation"),
                };
            }
            let external = Token::new(self.next);
            self.next = self.next.checked_add(1).expect("kernel operation ids");
            self.routes.insert(external, (submit.op, cancel)).expect("kernel route room");
            submit.op = external;
            target.push(submit);
        }
    }
    pub fn complete(&mut self, mut complete: Complete, sockets: &Queue<Submit>) -> Complete {
        let (internal, cancel) = self.routes.remove(&complete.op).expect("submitted operation route");
        for queued in sockets {
            if let Op::Cancel { target } = queued.kind
                && target == internal
            {
                self.retired.insert(internal, complete.op).expect("pending cancellation route");
                break;
            }
        }
        complete.op = internal;
        if let Some(internal) = cancel
            && let Op::Cancel { target } = &mut complete.kind
        {
            *target = internal;
        }
        complete
    }
}

pub(crate) fn owns(token: Token) -> bool {
    token.raw() & (15_u64 << 60) == PREFIX
}
pub(crate) fn tag(mut request: io::Request) -> io::Request {
    match &mut request {
        io::Request::Listen { owner, .. }
        | io::Request::Connect { owner, .. }
        | io::Request::Bind { owner, .. }
        | io::Request::Spawn { owner, .. }
        | io::Request::Usage { owner }
        | io::Request::Window { owner, .. } => {
            assert!(owner.raw() < (1_u64 << 60_u32), "bounded accounts exchange slots");
            *owner = Token::new(owner.raw() | PREFIX);
        }
        io::Request::Reject { .. }
        | io::Request::Stream { .. }
        | io::Request::Output { .. }
        | io::Request::Signal { .. }
        | io::Request::Close { .. }
        | io::Request::Abort { .. } => {}
    }
    request
}
pub(crate) fn untag(mut event: io::Event) -> io::Event {
    match &mut event {
        io::Event::Listening { owner, .. }
        | io::Event::Accepted { owner, .. }
        | io::Event::Connecting { owner, .. }
        | io::Event::Connected { owner }
        | io::Event::Stream { owner, .. }
        | io::Event::Output { owner, .. }
        | io::Event::Spawned { owner, .. }
        | io::Event::Exited { owner, .. }
        | io::Event::Usage { owner, .. }
        | io::Event::Window { owner, .. }
        | io::Event::Failed { owner, .. }
        | io::Event::Closed { owner } => {
            *owner = Token::new(owner.raw() & !(15_u64 << 60));
        }
        io::Event::Resized | io::Event::Shutdown { .. } => {}
    }
    event
}
/// Bounded source records, required scopes, key flights and queues beside the driver.
pub(crate) fn worst_case(limits: &driver::Limits, room: u32, page_rows: u32) -> Option<u64> {
    let record = u64::from(limits.client.document.record_bytes);
    driver::worst_case(limits)?
        .checked_add(u64::try_from(size_of::<Accounts>()).ok()?)?
        .checked_add(Queue::<io::Request>::worst_case(room)?)?
        .checked_add(Queue::<driver::Event>::worst_case(room)?)?
        .checked_add(Queue::<driver::FileLower>::worst_case(room)?)?
        .checked_add(Queue::<domain::Request>::worst_case(room)?)?
        .checked_add(records::worst_case(limits.accounts, &limits.client.document)?)?
        .checked_add(moves::worst_case(limits.accounts, &limits.client.document)?)?
        .checked_add(Map::<u32, u32>::worst_case(limits.accounts)?)?
        .checked_add(Queue::<moves::Next>::worst_case(room)?)?
        .checked_add(keys::worst_case(limits.accounts, limits.client.document.token_bytes)?)?
        .checked_add(restart::worst_case(limits.accounts, page_rows)?)?
        .checked_add(Queue::<keys::Next>::worst_case(room)?)?
        .checked_add(Queue::<domain::FromComponent>::worst_case(room)?)?
        .checked_add(Queue::<u32>::worst_case(room)?)?
        .checked_add(Map::<u32, Box<[u8]>>::worst_case(limits.accounts)?)?
        .checked_add(Map::<u32, Erasing>::worst_case(limits.accounts)?)?
        .checked_add(u64::from(limits.accounts).checked_mul(u64::from(limits.client.scope_bytes))?)?
        .checked_add(u64::from(room).checked_mul(record.checked_add(4096)?)?)
}

fn copy_endpoint(endpoint: &driver::Endpoint) -> driver::Endpoint {
    driver::Endpoint {
        address: endpoint.address,
        transport: match &endpoint.transport {
            driver::Transport::Plaintext => driver::Transport::Plaintext,
            driver::Transport::Tls { server_name, trust } => {
                driver::Transport::Tls { server_name: server_name.clone(), trust: trust.clone() }
            }
        },
    }
}
fn copy_origins(origins: &List<driver::Origin>) -> List<driver::Origin> {
    let mut copy = List::with_capacity(origins.len());
    for origin in origins {
        copy.push(driver::Origin { uri: origin.uri.clone(), endpoint: copy_endpoint(&origin.endpoint) })
            .expect("configured origins");
    }
    copy
}
fn has_tls(endpoint: &driver::Endpoint, origins: &List<driver::Origin>) -> bool {
    match endpoint.transport {
        driver::Transport::Tls { .. } => return true,
        driver::Transport::Plaintext => (),
    }
    for origin in origins {
        match origin.endpoint.transport {
            driver::Transport::Tls { .. } => return true,
            driver::Transport::Plaintext => (),
        }
    }
    false
}
fn event_account(event: &driver::Event) -> Option<u32> {
    match event {
        driver::Event::Granted { account, .. }
        | driver::Event::Expiring { account, .. }
        | driver::Event::Failed { account, .. }
        | driver::Event::Refused { account, .. }
        | driver::Event::Visit { account, .. }
        | driver::Event::Taken { account }
        | driver::Event::SignedIn { account, .. }
        | driver::Event::Keep { account, .. }
        | driver::Event::Removed { account }
        | driver::Event::SignedOut { account, .. }
        | driver::Event::Lock { account }
        | driver::Event::Unlock { account } => Some(*account),
        driver::Event::Closed => None,
    }
}

/// The public account sign-in templates retained beside the driver. Two
/// one-account driver bounds include their origins and the temporary startup copy.
pub(crate) fn configured_bytes(config: &config::Engine) -> Option<u64> {
    let count = u32::try_from(config.llm.providers.len()).ok()?;
    let limits = driver::Limits { accounts: 1, exchanges: 1, listeners: 1, ..config.limits.driver };
    Map::<u32, driver::Account>::worst_case(count)?
        .checked_add(driver::worst_case(&limits)?.checked_mul(u64::from(count))?.checked_mul(2)?)
}

#[cfg(test)]
#[path = "tests/accounts_host.rs"]
mod tests;

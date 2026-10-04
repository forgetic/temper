//! Bounded reply routing across independently owned HTTP connections.
use crate::{config::Config, connection::Limits};
use skein_lib::{Id, Map, ReplyTo, Slab, Token};
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Routed {
    pub owner: Token,
    pub call: Token,
}
#[derive(Debug)]
struct Call {
    owner: Token,
    active: bool,
}
#[expect(missing_debug_implementations, reason = "service configuration retains API tokens")]
pub struct Service {
    pub(crate) config: Config,
    calls: Slab<Call>,
    owners: Map<Token, Id<Call>>,
}
impl Service {
    #[must_use]
    pub fn new(config: Config, limits: &Limits) -> Option<Service> {
        if !limits.valid() || !config.valid(&limits.documents) || limits.calls == 0 {
            return None;
        }
        Some(Service { config, calls: Slab::with_capacity(limits.calls), owners: Map::with_capacity(limits.calls) })
    }
    pub(crate) fn allocate(&mut self, owner: Token) -> Option<Token> {
        assert!(!self.owners.contains_key(&owner), "one pending exchange per connection");
        let id = self.calls.insert(Call { owner, active: true }).ok()?;
        self.owners.insert(owner, id).expect("one owner per live call fits");
        Some(id.token())
    }
    /// Consume the domain's one terminal right and remove its live route.
    pub fn route(&mut self, to: ReplyTo) -> Option<Routed> {
        let call = to.into_token();
        let stored = self.calls.get(Id::<Call>::from_token(call))?;
        if !stored.active {
            return None;
        }
        let owner = stored.owner;
        self.finish(call);
        Some(Routed { owner, call })
    }
    pub(crate) fn finish(&mut self, to: Token) {
        let id = Id::<Call>::from_token(to);
        if let Some(call) = self.calls.get_mut(id) {
            if !call.active {
                return;
            }
            call.active = false;
            assert!(self.owners.remove(&call.owner) == Some(id), "live call owns its connection route");
            self.calls.retire(id);
        }
    }
    pub(crate) fn lost(&mut self, owner: Token) {
        if let Some(id) = self.owners.remove(&owner) {
            self.calls.get_mut(id).expect("live owner route").active = false;
            self.calls.retire(id);
        }
    }
    pub fn reclaim(&mut self) {
        self.calls.reclaim();
    }
}
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let fields = u64::from(limits.documents.fields);
    let names = u64::from(limits.documents.name_bytes);
    let users = fields
        .checked_mul(u64::try_from(size_of::<crate::config::Identity>()).ok()?.checked_add(names.checked_mul(2)?)?)?;
    let repositories = fields.checked_mul(
        u64::try_from(size_of::<crate::config::Repository>()).ok()?.checked_add(names)?.checked_add(
            fields.checked_mul(
                u64::try_from(size_of::<temper_forge_forgejo::types::Label>()).ok()?.checked_add(names)?,
            )?,
        )?,
    )?;
    Slab::<Call>::worst_case(limits.calls)?
        .checked_add(Map::<Token, Id<Call>>::worst_case(limits.calls)?)?
        .checked_add(users)?
        .checked_add(repositories)
}

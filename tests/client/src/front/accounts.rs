//! The accounts story restores the fixture person's deployment membership.
use super::{Driver, Joint, Store, client, engine, engine_limits, fixtures, walking};
impl Joint {
    pub(crate) fn with_model_connections(seed: u64, limits: client::Limits) -> Self {
        let mut bounds = engine_limits();
        bounds.accounts.accounts = 3;
        bounds.accounts.holder_accounts = 2;
        bounds.accounts.credentials = 3;
        bounds.authority.accounts = 4;
        bounds.fleet.grants = 6;
        bounds.journal.writes = bounds.journal.writes.checked_add(16).expect("four grant cohorts fit");
        let configs = std::array::from_fn(|_| walking::config(seed));
        Self::with_engine(seed, &limits, Store::new(), configs, &bounds, 1)
    }

    pub(crate) fn with_accounts(seed: u64, limits: client::Limits) -> Self {
        let mut bounds = engine_limits();
        bounds.people.projects = 2; // Project membership and the deployment role row.
        let mut first = Driver::with_repositories(
            Store::new(),
            [walking::config(seed), walking::config(seed)],
            &bounds,
            fixtures::REPOSITORIES,
        );
        first.settle();
        let membership = jig_domain_people::Stored::Roles {
            project: 0,
            holdings: Box::new([jig_domain_people::Holding {
                person: fixtures::PARTY,
                role: jig_domain_people::Role::Owner,
            }]),
        };
        let row = engine::Record::Core(jig_domain_core::Record::People(membership));
        first.store.seed(Box::new([row]), Box::new([]));
        Self::with_engine(seed, &limits, first.store, [walking::config(seed), walking::config(seed)], &bounds, 1)
    }
}

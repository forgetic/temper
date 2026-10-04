//! What the deployment configures (engine-domain.md, section 13): its
//! repositories and its home, the labels the forge child domain owns and the
//! engine's forge user, its rules, the plan's configuration and templates,
//! the step a session carries, and what a run is given beyond its plan.
//! Repositories are named by their place in the deployment's list
//! everywhere in the domain; the protocol layer maps each to its forge name,
//! its remote and the identity workers use for it.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use temper_engine_domain_views::Policy;
use temper_legacy_engine_domain_forge as forge;
use temper_legacy_engine_domain_plan as plan;
use temper_legacy_engine_domain_rules as rules;

/// A deployment's configuration, handed to [`crate::Domain::new`].
#[derive(Debug)]
pub struct Config {
    /// Startup metadata; values are owned by the protocol credential table.
    pub accounts: Box<[Account]>,
    /// Endpoint index to LLM account, repository index to static git account.
    pub endpoints: Box<[u32]>,
    pub identities: Box<[u32]>,
    /// The deployment's repositories, by their place in its list: what
    /// changes may land into in each, and the templates (the plan's).
    pub plan: plan::Config,
    /// The repository whose wiki holds the deployment's notes.
    pub home: u32,
    /// The engine's forge user and the labels it owns.
    pub forge: forge::Config,
    /// The deployment's rules (engine-domain.md, section 7).
    pub rules: rules::Rules,
    /// What a session opened from the web, or handed in, carries.
    pub session: plan::SessionSpec,
    /// The models a run's charter names, as the agent takes them.
    pub models: Box<[crate::Model]>,
    /// What a run's trace keeps of what it reports.
    pub policy: Policy,
    /// The branch an item's change is pushed to is this, then the item's
    /// number; its unfinished work is saved to `saved`, then its number.
    pub branches: Box<[u8]>,
    pub saved: Box<[u8]>,
}

impl Config {
    /// How many repositories the deployment has.
    #[must_use]
    pub fn repositories(&self) -> u32 {
        u32::try_from(self.plan.repositories.len()).unwrap_or(u32::MAX)
    }

    /// The step a session in `repository` carries.
    pub(crate) fn session_step(&self, repository: u32) -> plan::Step {
        plan::Step {
            name: copy_of(b"session"),
            repository: plan::Repository(repository),
            work: plan::Work::Session(self.session.clone()),
            after: Box::new([]),
            gates: Box::new([]),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Account {
    pub account: u32,
    pub generation: u64,
    /// Remaining validity at the `now` handed to [`crate::Domain::new`].
    pub valid: Option<skein_lib::Duration>,
}

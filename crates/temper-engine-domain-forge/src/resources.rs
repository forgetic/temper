//! Resource roles and hold admission (jig's domain/connectors.md, section 3).
//! Forge repositories expose no counted pool resources. Landing branches and
//! participating objects are shared; private task branches have one holder.
use crate::{Access, Domain, Name, Role, What};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HoldKind {
    Shared,
    Exclusive { wait: bool },
}
/// Admission rule for the forge's private branch kind.
pub const BRANCH_HOLD: HoldKind = HoldKind::Exclusive { wait: true };

impl Domain {
    /// Report each named resource from adopted repository facts.
    #[must_use]
    pub fn resource_facts(&self, name: &Name) -> Option<(Access, HoldKind)> {
        let repository = self.repository(temper_engine_domain_forge_client::api::Repository {
            forge: name.forge,
            repository: name.repository,
        })?;
        if !repository.kinds.read {
            return Some((Access::Unavailable, HoldKind::Shared));
        }
        if repository.role == Role::Context {
            return Some((Access::Context, HoldKind::Shared));
        }
        match &name.what {
            What::Branch(_) => {
                let own = crate::items::branch_starts_with(name, &repository.prefix);
                Some(if own { (Access::Owned, BRANCH_HOLD) } else { (Access::Participant, HoldKind::Shared) })
            }
            What::Repository => Some((
                match repository.role {
                    Role::Owned => Access::Owned,
                    Role::Adopted | Role::Fork => Access::Participant,
                    Role::Context => Access::Context,
                },
                HoldKind::Shared,
            )),
            What::Pull(number) => {
                let mut own = false;
                for (_, row) in &self.changes {
                    if row.repository == repository.provider && row.pull == Some(*number) {
                        own = true;
                    }
                }
                Some((if own { Access::Owned } else { Access::Participant }, HoldKind::Shared))
            }
            What::Issue(number) => {
                let mut own = false;
                for (_, row) in &self.issues {
                    if row.repository == repository.provider && row.number == Some(*number) {
                        own = true;
                    }
                }
                Some((if own { Access::Owned } else { Access::Participant }, HoldKind::Shared))
            }
        }
    }
}

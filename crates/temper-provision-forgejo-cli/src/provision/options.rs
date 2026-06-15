//! CLI-facing provisioning options for the demo provision-forgejo subcommand.

use temper_forge::AccessScope;

/// Options that tune [`provision_world`](super::provision_world) away from its
/// throwaway-repo defaults.
///
/// Both fields default to today's behavior, so `ProvisionOptions::default()`
/// leaves the throwaway `reference-delivery` / `basic-delivery` flows unchanged.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProvisionOptions {
    /// Provision onto a repo that must already exist: require the repo up front
    /// (erroring if absent), and skip the marker CI commit and the CI sentinel
    /// commit so the repo's own `.forgejo/workflows/ci.yml` and history are
    /// never touched. Labels, the webhook, and `enable_actions` still apply.
    pub existing_repo: bool,
    /// How role users and the `bot` are granted access to the repo.
    pub access: AccessScope,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provision_options_default_is_back_compat() {
        // Defaults must reproduce today's throwaway behavior: create the repo
        // (and commit CI) and join the Owners team.
        let options = ProvisionOptions::default();
        assert!(!options.existing_repo);
        assert_eq!(options.access, AccessScope::OrgOwners);
        assert_eq!(AccessScope::default(), AccessScope::OrgOwners);
    }
}

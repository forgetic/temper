//! Role → capability mapping.

/// The capability a role runs with. Engineer mutates the checkout; architect and
/// reviewer are read-only analysts that emit a verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Capability {
    /// Edit tools; leaves a product diff. Maps to the engineer role.
    CodingWorkspace,
    /// Read-only analysis; emits a verdict + authored body / children.
    TriageWorkspace,
    /// Read-only diff + CI review; emits an approve / changes / escalate verdict.
    ReviewWorkspace,
}

impl Capability {
    /// Maps a workflow role id to its capability. Unknown roles default to the
    /// read-only triage capability so an unexpected role can never silently
    /// mutate the checkout.
    pub fn for_role(role: &str) -> Self {
        match role {
            "engineer" => Capability::CodingWorkspace,
            "reviewer" | "tester" => Capability::ReviewWorkspace,
            _ => Capability::TriageWorkspace,
        }
    }

    /// Whether the capability is allowed to mutate the working tree.
    pub fn is_writable(self) -> bool {
        matches!(self, Capability::CodingWorkspace)
    }
}

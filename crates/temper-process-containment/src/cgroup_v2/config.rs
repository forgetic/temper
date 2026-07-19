use std::io;

use super::*;

const DEFAULT_SUBTREE: &str = "temper";

/// Deterministic path context for cgroups owned by one factory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CgroupV2FactoryConfig {
    pub(super) owner: String,
    pub(super) owner_fence: Option<CgroupV2OwnerFence>,
    pub(super) job: String,
    pub(super) attempt: String,
    pub(super) subtree: String,
}

impl CgroupV2FactoryConfig {
    pub fn new(job: impl AsRef<str>, attempt: impl AsRef<str>) -> io::Result<Self> {
        Self::for_owner("process", job, attempt)
    }

    /// Bind every cgroup made by this factory to one logical owner. The system
    /// factory adds the current process-incarnation fence before probing.
    pub fn for_owner(
        owner: impl AsRef<str>,
        job: impl AsRef<str>,
        attempt: impl AsRef<str>,
    ) -> io::Result<Self> {
        Ok(Self {
            owner: encode_component(owner.as_ref(), "owner")?,
            owner_fence: None,
            job: encode_component(job.as_ref(), "job")?,
            attempt: encode_component(attempt.as_ref(), "attempt")?,
            subtree: DEFAULT_SUBTREE.to_owned(),
        })
    }

    /// Override the dedicated Temper-owned subtree name.
    pub fn with_subtree(mut self, subtree: impl AsRef<str>) -> io::Result<Self> {
        self.subtree = encode_component(subtree.as_ref(), "subtree")?;
        Ok(self)
    }

    /// Override the process-incarnation fence. Production factories derive
    /// this from their own `/proc` identity; explicit injection supports
    /// deterministic delegated-cgroup fixtures.
    pub fn with_owner_fence(mut self, owner_fence: CgroupV2OwnerFence) -> Self {
        self.owner_fence = Some(owner_fence);
        self
    }

    pub fn job(&self) -> &str {
        &self.job
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn owner_fence(&self) -> Option<&CgroupV2OwnerFence> {
        self.owner_fence.as_ref()
    }

    pub fn attempt(&self) -> &str {
        &self.attempt
    }

    pub fn subtree(&self) -> &str {
        &self.subtree
    }
}

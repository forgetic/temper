use crate::{LaunchConfig, ProviderBootstrapFailure};
use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;
use temper_process_containment::{CleanupReport, ContainmentFactory};

#[derive(Clone, Default)]
pub struct ProviderOwnerManager;
#[derive(Clone)]
pub struct ProviderBootstrap;
#[derive(Clone)]
pub struct ProviderCompletion;

impl ProviderOwnerManager {
    pub fn reap_completed(&self) {}
    pub fn completions(&self) -> Vec<ProviderCompletion> {
        Vec::new()
    }
}
impl ProviderBootstrap {
    pub fn start(
        _: &ProviderOwnerManager,
        _: LaunchConfig,
        _: &ContainmentFactory,
        _: &Path,
    ) -> Result<Self, ProviderBootstrapFailure> {
        Err(ProviderBootstrapFailure::UnsupportedPlatform)
    }
    pub fn serving_admitted(&self) {}
    pub fn admission_remaining(&self) -> Duration {
        Duration::ZERO
    }
    pub fn admission_pending(&self) -> bool {
        false
    }
    pub fn expire_admission(&self) -> bool {
        true
    }
    pub fn completion(&self) -> ProviderCompletion {
        ProviderCompletion
    }
}
impl ProviderCompletion {
    pub fn wait(&self, _: Duration) -> Option<CleanupReport> {
        None
    }
}
#[doc(hidden)]
pub fn dispatch_provider_bootstrap_helper(
    args: impl IntoIterator<Item = OsString>,
) -> Option<ExitCode> {
    (args.into_iter().next().as_deref() == Some(std::ffi::OsStr::new(crate::config::HELPER_MODE)))
        .then_some(ExitCode::FAILURE)
}

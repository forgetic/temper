//! A temporary provider bootstrap, owned outside individual Temper jobs.
//!
//! The configured public MCP frontend causes native daemon activation. After
//! serving admission the bootstrap session disconnects; the helper retains
//! descendant ownership until the provider's final account session ends.

mod admission;
mod config;
#[cfg(target_os = "linux")]
mod frontend;
#[cfg(target_os = "linux")]
mod helper;
#[cfg(target_os = "linux")]
mod owner;
mod retention;

pub use admission::prepare_invocation;
pub use config::{LaunchConfig, ProviderBootstrapFailure};
#[cfg(target_os = "linux")]
pub use helper::dispatch_provider_bootstrap_helper;
#[cfg(target_os = "linux")]
pub use owner::{ProviderBootstrap, ProviderCompletion, ProviderOwnerManager};
pub use retention::retain_admission_until_cleanup;

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
pub use unsupported::*;

mod agent;
mod parse;
mod roles;
mod types;

#[cfg(test)]
mod parse_native_tests;
#[cfg(test)]
mod parse_test_support;
#[cfg(test)]
mod parse_tests;
#[cfg(test)]
mod role_tests;
#[cfg(test)]
mod types_tests;

pub use agent::TEMPER_AGENT_PROGRAM;
pub use parse::{USAGE, parse};
pub use roles::role_identities_from_env;
pub use types::{
    AgentProviderChoice, AgentSurface, AnvilNativeAgentSurface, CapabilitySpec, CodingSurface,
    DEFAULT_POLL_BACKOFF, ExecutorSelection, ParseOutcome, WorkerAgentTraceConfig, WorkerConfig,
    WorkerParams,
};

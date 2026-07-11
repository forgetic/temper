// SPDX-License-Identifier: MPL-2.0

//! Shared deployment loading, desired state, and credential persistence.

mod credentials;
mod load;
mod model;

pub use credentials::{durable_credentials_path, merge_provisioned_credentials};
pub use load::load_deployment;
pub use model::{
    DeploymentBundle, DeploymentMetadata, DesiredRepository, DesiredWebhook, ForgeAuthentication,
};

#[cfg(test)]
mod tests;

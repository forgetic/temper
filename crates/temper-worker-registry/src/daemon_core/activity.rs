// SPDX-License-Identifier: MPL-2.0

//! Agent activity authorization for the daemon worker registry.

use temper_protocol_worker::WorkerAuth;

use super::{DaemonCore, WorkerAuthError};

impl DaemonCore {
    /// Authenticates a worker for durable activity ingestion. Network carriers
    /// require configured pool authentication; a co-resident trusted carrier
    /// may bypass the credential requirement, but never registration/health.
    pub fn authorize_activity_worker(
        &self,
        worker_id: &str,
        role: &str,
        repository: &str,
        auth: Option<&WorkerAuth>,
        trusted_transport: bool,
    ) -> Result<(), WorkerAuthError> {
        if worker_id.trim().is_empty() {
            return Err(WorkerAuthError::new(
                "activity batch worker_id must not be empty",
            ));
        }
        if !trusted_transport {
            if !self.worker_auth.is_enabled() {
                return Err(WorkerAuthError::new(
                    "distributed activity ingestion requires configured worker authentication",
                ));
            }
            self.authenticate_registered_worker(worker_id, None, auth)?;
        }
        if !self.coordinator.registry().is_healthy(worker_id) {
            return Err(WorkerAuthError::new(format!(
                "worker `{worker_id}` is not registered and healthy"
            )));
        }
        if !self
            .coordinator
            .registry()
            .can_handle(worker_id, role, repository)
        {
            return Err(WorkerAuthError::new(format!(
                "worker `{worker_id}` is not authorized for activity role `{role}` in `{repository}`"
            )));
        }
        Ok(())
    }
}

// SPDX-License-Identifier: MPL-2.0

//! Per-process runtime overrides for `temper serve` compatibility flags.

use temper_config::{Capability, Resolved, WorkerPoolSettings};

use crate::Service;

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct RuntimeOverrides {
    /// Stable process identity override. For standalone this is applied to both
    /// the in-process engine daemon id and worker id; for single-service modes it
    /// applies to the selected service identity.
    pub process_id: Option<String>,
    /// Selected target-era worker pool name (`temper serve worker --pool`).
    pub worker_pool: Option<String>,
    /// Per-process worker capacity override (`temper serve worker --capacity`).
    pub worker_capacity: Option<u32>,
    /// Per-process worker daemon/engine URL override (`temper serve worker --engine-url`).
    pub worker_engine_url: Option<String>,
}

pub(crate) fn apply_runtime_overrides(
    resolved: &mut Resolved,
    service: Option<Service>,
    runtime: &RuntimeOverrides,
) -> Result<(), String> {
    match service {
        None => apply_standalone_runtime_overrides(resolved, runtime),
        Some(Service::Engine) => apply_engine_runtime_overrides(resolved, runtime),
        Some(Service::Worker) => apply_worker_runtime_overrides(resolved, runtime),
    }
}

fn apply_standalone_runtime_overrides(
    resolved: &mut Resolved,
    runtime: &RuntimeOverrides,
) -> Result<(), String> {
    reject_worker_only_runtime_overrides(runtime, "standalone")?;
    if let Some(process_id) = runtime.process_id.as_deref() {
        let process_id = non_empty_runtime_override("--id", process_id)?;
        resolved.engine.daemon_id = process_id.to_string();
        resolved.worker.worker_id = process_id.to_string();
    }
    Ok(())
}

fn apply_engine_runtime_overrides(
    resolved: &mut Resolved,
    runtime: &RuntimeOverrides,
) -> Result<(), String> {
    reject_worker_only_runtime_overrides(runtime, "engine")?;
    if let Some(process_id) = runtime.process_id.as_deref() {
        resolved.engine.daemon_id = non_empty_runtime_override("--id", process_id)?.to_string();
    }
    Ok(())
}

fn apply_worker_runtime_overrides(
    resolved: &mut Resolved,
    runtime: &RuntimeOverrides,
) -> Result<(), String> {
    if let Some(process_id) = runtime.process_id.as_deref() {
        resolved.worker.worker_id = non_empty_runtime_override("--id", process_id)?.to_string();
    }
    if let Some(engine_url) = runtime.worker_engine_url.as_deref() {
        resolved.worker.daemon_url =
            non_empty_runtime_override("--engine-url", engine_url)?.to_string();
    }
    if let Some(pool_name) = runtime.worker_pool.as_deref() {
        let pool_name = non_empty_runtime_override("--pool", pool_name)?;
        let pool = resolved
            .worker
            .pools
            .iter()
            .find(|pool| pool.name == pool_name)
            .ok_or_else(|| format!("unknown worker pool `{pool_name}`"))?;
        let capabilities = capabilities_from_pool(pool)?;
        if let Some(capacity) = pool.max_concurrent_jobs {
            resolved.worker.max_concurrent_jobs = capacity;
        }
        resolved.worker.capabilities = capabilities;
    }
    if let Some(capacity) = runtime.worker_capacity {
        if capacity == 0 {
            return Err("--capacity must be greater than zero".to_string());
        }
        resolved.worker.max_concurrent_jobs = capacity;
    }
    Ok(())
}

fn reject_worker_only_runtime_overrides(
    runtime: &RuntimeOverrides,
    component: &str,
) -> Result<(), String> {
    if let Some(pool) = &runtime.worker_pool {
        return Err(format!(
            "`--pool` cannot be used with `temper serve {component}` (got `{pool}`); use `temper serve worker`"
        ));
    }
    if runtime.worker_capacity.is_some() {
        return Err(format!(
            "`--capacity` cannot be used with `temper serve {component}`; use `temper serve worker`"
        ));
    }
    if let Some(url) = &runtime.worker_engine_url {
        return Err(format!(
            "`--engine-url` cannot be used with `temper serve {component}` (got `{url}`); use `temper serve worker`"
        ));
    }
    Ok(())
}

fn capabilities_from_pool(pool: &WorkerPoolSettings) -> Result<Vec<Capability>, String> {
    if pool.roles.is_empty() {
        return Err(format!(
            "worker pool `{}` does not declare any roles, so it cannot produce runtime capabilities",
            pool.name
        ));
    }
    if pool.repos.is_empty() {
        return Err(format!(
            "worker pool `{}` does not declare any repos, so it cannot produce runtime capabilities",
            pool.name
        ));
    }

    let mut capabilities = Vec::with_capacity(pool.roles.len() * pool.repos.len());
    for repo in &pool.repos {
        for role in &pool.roles {
            capabilities.push(Capability {
                repo: repo.display(),
                role: role.clone(),
            });
        }
    }
    Ok(capabilities)
}

fn non_empty_runtime_override<'a>(flag: &str, value: &'a str) -> Result<&'a str, String> {
    if value.trim().is_empty() {
        Err(format!("{flag} requires a non-empty value"))
    } else {
        Ok(value)
    }
}

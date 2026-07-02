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
    if !resolved.worker.pools.is_empty() {
        let pool_name = standalone_pool_name(&resolved.worker.pools)?;
        apply_worker_pool_policy(resolved, &pool_name, None)?;
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

    match runtime.worker_pool.as_deref() {
        Some(pool_name) => {
            let pool_name = non_empty_runtime_override("--pool", pool_name)?;
            apply_worker_pool_policy(resolved, pool_name, runtime.worker_capacity)?;
        }
        None if !resolved.worker.pools.is_empty() => {
            return Err(
                "worker pools are configured; select one with `temper serve worker --pool <NAME>`"
                    .to_string(),
            );
        }
        None => apply_legacy_capacity_override(resolved, runtime.worker_capacity)?,
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

fn standalone_pool_name(pools: &[WorkerPoolSettings]) -> Result<String, String> {
    for preferred in ["local", "default"] {
        if let Some(pool) = pools.iter().find(|pool| pool.name == preferred) {
            return Ok(pool.name.clone());
        }
    }
    if pools.len() == 1 {
        return Ok(pools[0].name.clone());
    }
    Err(
        "standalone worker pools are configured; configure a `local` or `default` pool so standalone can select target-era capabilities"
            .to_string(),
    )
}

fn apply_worker_pool_policy(
    resolved: &mut Resolved,
    pool_name: &str,
    capacity_override: Option<u32>,
) -> Result<(), String> {
    let pool = resolved
        .worker
        .pools
        .iter()
        .find(|pool| pool.name == pool_name)
        .cloned()
        .ok_or_else(|| format!("unknown worker pool `{pool_name}`"))?;

    validate_pool_agent_profile(resolved, &pool)?;
    let policy_capacity = pool.max_concurrent_jobs.ok_or_else(|| {
        format!(
            "worker pool `{}` must set max_concurrent_jobs before it can be used at runtime",
            pool.name
        )
    })?;
    let runtime_capacity = match capacity_override {
        Some(0) => return Err("--capacity must be greater than zero".to_string()),
        Some(capacity) if capacity > policy_capacity => {
            return Err(format!(
                "--capacity {capacity} exceeds worker pool `{}` max_concurrent_jobs {policy_capacity}",
                pool.name
            ));
        }
        Some(capacity) => capacity,
        None => policy_capacity,
    };

    resolved.worker.capabilities = capabilities_from_pool(&pool)?;
    resolved.worker.max_concurrent_jobs = runtime_capacity;
    resolved.worker.selected_pool = Some(pool.name);
    Ok(())
}

fn validate_pool_agent_profile(
    resolved: &Resolved,
    pool: &WorkerPoolSettings,
) -> Result<(), String> {
    if let Some(profile_name) = pool.agent_profile.as_deref() {
        if !resolved.agent.profiles.contains_key(profile_name) {
            return Err(format!(
                "worker pool `{}` references missing agent profile `{profile_name}`",
                pool.name
            ));
        }
    }
    Ok(())
}

fn apply_legacy_capacity_override(
    resolved: &mut Resolved,
    capacity: Option<u32>,
) -> Result<(), String> {
    if let Some(capacity) = capacity {
        if capacity == 0 {
            return Err("--capacity must be greater than zero".to_string());
        }
        resolved.worker.max_concurrent_jobs = capacity;
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

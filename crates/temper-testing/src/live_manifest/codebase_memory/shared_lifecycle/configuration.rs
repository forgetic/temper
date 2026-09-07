//! Narrow generated-config changes for two concurrent first-party jobs.
use std::path::Path;

pub(in crate::live_manifest) fn tune(path: &Path) -> Result<(), String> {
    let raw =
        std::fs::read_to_string(path).map_err(|error| format!("read lifecycle config: {error}"))?;
    let mut doc: toml::Value = raw
        .parse()
        .map_err(|error| format!("parse lifecycle config: {error}"))?;
    let worker = doc
        .get_mut("worker")
        .and_then(toml::Value::as_table_mut)
        .ok_or("missing worker config")?;
    for (key, value) in [
        ("heartbeat_interval_ms", 250),
        ("max_concurrent_jobs", 2),
        ("graceful_cancellation_grace_secs", 1),
        ("forced_termination_grace_secs", 1),
    ] {
        worker.insert(key.into(), toml::Value::Integer(value));
    }
    let pools = worker
        .get_mut("pools")
        .and_then(toml::Value::as_array_mut)
        .ok_or("missing worker pools")?;
    for pool in pools {
        pool.as_table_mut()
            .ok_or("malformed worker pool")?
            .insert("max_concurrent_jobs".into(), toml::Value::Integer(2));
    }
    let codebase = doc
        .get_mut("agent")
        .and_then(|value| value.get_mut("tools"))
        .and_then(|value| value.get_mut("codebase_memory"))
        .and_then(toml::Value::as_table_mut)
        .ok_or("missing codebase memory config")?;
    codebase.insert("startup_timeout_secs".into(), toml::Value::Integer(10));
    codebase.insert("index_timeout_secs".into(), toml::Value::Integer(10));
    // This acceptance starts with job A's cold activation. Periodic cache
    // maintenance is independent work and must not create a prelude cohort.
    codebase.insert(
        "retention".into(),
        toml::Value::Table(toml::map::Map::from_iter([(
            "enabled".into(),
            toml::Value::Boolean(false),
        )])),
    );
    std::fs::write(
        path,
        toml::to_string_pretty(&doc)
            .map_err(|error| format!("serialize lifecycle config: {error}"))?,
    )
    .map_err(|error| format!("write lifecycle config: {error}"))
}

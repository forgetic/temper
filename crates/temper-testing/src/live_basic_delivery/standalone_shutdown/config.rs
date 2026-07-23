use super::*;

pub(super) struct ShutdownFixtureConfig<'a> {
    pub(super) fixture: &'a Path,
    pub(super) identities: &'a Path,
    pub(super) ready: &'a Path,
    pub(super) obstruction_trigger: &'a Path,
    pub(super) obstruction_ready: &'a Path,
    pub(super) state_root: &'a Path,
    pub(super) workspace_root: &'a Path,
}

pub(super) fn tune_shutdown_config(
    config_path: &Path,
    fixture: &ShutdownFixtureConfig<'_>,
) -> Result<(), String> {
    let source = fs::read_to_string(config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?;
    let mut document: TomlValue = source
        .parse()
        .map_err(|error| format!("parse {} as TOML: {error}", config_path.display()))?;
    let root = document
        .as_table_mut()
        .ok_or_else(|| "config.toml root must be a table".to_string())?;

    table(root, "deployment")?.insert(
        "standalone_shutdown_budget_secs".to_string(),
        TomlValue::Integer(SHUTDOWN_BUDGET.as_secs() as i64),
    );
    let paths = table(root, "paths")?;
    paths.insert(
        "state_dir".to_string(),
        TomlValue::String(fixture.state_root.display().to_string()),
    );
    paths.insert(
        "workspace_dir".to_string(),
        TomlValue::String(fixture.workspace_root.display().to_string()),
    );
    let worker = table(root, "worker")?;
    worker.insert(
        "workspace".to_string(),
        TomlValue::String(fixture.workspace_root.display().to_string()),
    );
    worker.insert(
        "graceful_cancellation_grace_secs".to_string(),
        TomlValue::Integer(1),
    );
    worker.insert(
        "forced_termination_grace_secs".to_string(),
        TomlValue::Integer(1),
    );

    let observability = table(root, "observability")?;
    let traces = table(observability, "agent_traces")?;
    traces.insert(
        "capture".to_string(),
        TomlValue::String("metadata".to_string()),
    );

    let agent = table(root, "agent")?;
    let tools = table(agent, "tools")?;
    let mut codebase = toml::map::Map::new();
    codebase.insert(
        "mode".to_string(),
        TomlValue::String("required".to_string()),
    );
    codebase.insert(
        "command".to_string(),
        TomlValue::String(fixture.fixture.display().to_string()),
    );
    codebase.insert(
        "args".to_string(),
        TomlValue::Array(
            [
                "mcp".to_string(),
                fixture.identities.display().to_string(),
                fixture.ready.display().to_string(),
                fixture.obstruction_trigger.display().to_string(),
                fixture.obstruction_ready.display().to_string(),
            ]
            .into_iter()
            .map(TomlValue::String)
            .collect(),
        ),
    );
    codebase.insert(
        "roles".to_string(),
        TomlValue::Array(vec![TomlValue::String("engineer".to_string())]),
    );
    codebase.insert("index".to_string(), TomlValue::String("off".to_string()));
    codebase.insert("startup_timeout_secs".to_string(), TomlValue::Integer(5));
    codebase.insert("index_timeout_secs".to_string(), TomlValue::Integer(5));
    tools.insert("codebase_memory".to_string(), TomlValue::Table(codebase));

    fs::write(
        config_path,
        toml::to_string_pretty(&document)
            .map_err(|error| format!("serialize standalone shutdown config: {error}"))?,
    )
    .map_err(|error| format!("write {}: {error}", config_path.display()))
}

fn table<'a>(
    parent: &'a mut toml::map::Map<String, TomlValue>,
    name: &str,
) -> Result<&'a mut toml::map::Map<String, TomlValue>, String> {
    parent
        .entry(name.to_string())
        .or_insert_with(|| TomlValue::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| format!("config `{name}` must be a table"))
}

pub(super) fn worker_token(bundle: &Path) -> Result<String, String> {
    let config: TomlValue = fs::read_to_string(bundle.join("config.toml"))
        .map_err(|error| format!("read generated config: {error}"))?
        .parse()
        .map_err(|error| format!("parse generated config: {error}"))?;
    let secret_name = config
        .get("worker")
        .and_then(|worker| worker.get("pools"))
        .and_then(TomlValue::as_array)
        .and_then(|pools| pools.first())
        .and_then(|pool| pool.get("worker_token"))
        .and_then(TomlValue::as_str)
        .ok_or_else(|| "generated standalone pool has no worker_token reference".to_string())?;
    let credentials: TomlValue = fs::read_to_string(bundle.join("credentials.toml"))
        .map_err(|error| format!("read generated credentials: {error}"))?
        .parse()
        .map_err(|error| format!("parse generated credentials: {error}"))?;
    credentials
        .get("secrets")
        .and_then(|secrets| secrets.get(secret_name))
        .and_then(|secret| secret.get("token"))
        .and_then(TomlValue::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("generated credentials have no token for `{secret_name}`"))
}

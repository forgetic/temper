// SPDX-License-Identifier: MPL-2.0

//! `temper config paths` path-reporting command.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};
use temper_cli_common::{EnvMap, LoadOptions, OutputFormat, PathResolver};
use temper_config::{
    Config, ConfigError, Credentials, config_path, paired_credentials_path, resolve, state_dir,
};

/// Runs `temper config paths`.
pub(crate) fn command(
    args: &[String],
    options: &LoadOptions,
    format: OutputFormat,
    env: &EnvMap,
    base_paths: &PathResolver,
) -> Result<ExitCode, String> {
    super::parse_options(args, false)?;
    let report = PathReport::resolve(options, env, base_paths)?;
    match format {
        OutputFormat::Human => print!("{}", report.render_human()),
        OutputFormat::Json => println!("{}", report.render_json()?),
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct PathReport {
    config_root: Option<PathBuf>,
    config_file: Option<PathBuf>,
    credentials_source: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    workspace_dir: PathBuf,
    workflow_file: Option<PathBuf>,
}

impl PathReport {
    fn resolve(
        options: &LoadOptions,
        env: &EnvMap,
        base_paths: &PathResolver,
    ) -> Result<Self, String> {
        let explicit = options.config.is_some() || options.credentials.is_some();
        let empty = PathResolver::default();
        let discovery_paths = if explicit { &empty } else { base_paths };

        let config_file = config_path(options.config.clone(), discovery_paths, env);
        let credentials_source = paired_credentials_path(
            options.credentials.clone(),
            options.config.clone(),
            discovery_paths,
            env,
        );
        let config_root = config_file.as_deref().and_then(parent_dir);
        let config = load_config_if_present(config_file.as_deref())?;
        let resolved = resolve(&config, &Credentials::default(), env).map_err(|error| {
            format!("resolving path-related settings from the config file: {error}")
        })?;

        Ok(Self {
            config_root,
            config_file,
            credentials_source,
            state_dir: state_dir(base_paths),
            workspace_dir: resolved.worker.workspace_root,
            workflow_file: resolved.engine.workflow_file,
        })
    }

    fn render_human(&self) -> String {
        format!(
            "config root:        {}\n\
             config file:        {}\n\
             credentials source: {}\n\
             state dir:          {}\n\
             workspace dir:      {}\n\
             workflow file:      {}\n",
            display_optional(self.config_root.as_deref(), "(unavailable)"),
            display_optional(self.config_file.as_deref(), "(unavailable)"),
            display_optional(self.credentials_source.as_deref(), "(unavailable)"),
            display_optional(self.state_dir.as_deref(), "(unavailable)"),
            self.workspace_dir.display(),
            display_optional(
                self.workflow_file.as_deref(),
                "(bundled reference-delivery)"
            ),
        )
    }

    fn render_json(&self) -> Result<String, String> {
        let value = json!({
            "config_root": json_path(self.config_root.as_deref()),
            "config_file": json_path(self.config_file.as_deref()),
            "credentials_source": json_path(self.credentials_source.as_deref()),
            "state_dir": json_path(self.state_dir.as_deref()),
            "workspace_dir": self.workspace_dir.display().to_string(),
            "workflow_file": json_path(self.workflow_file.as_deref()),
        });
        serde_json::to_string_pretty(&value).map_err(|error| format!("render paths JSON: {error}"))
    }
}

fn load_config_if_present(path: Option<&Path>) -> Result<Config, String> {
    let Some(path) = path else {
        return Ok(Config::default());
    };
    match Config::load(path) {
        Ok(config) => Ok(config),
        Err(ConfigError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(Config::default())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn parent_dir(path: &Path) -> Option<PathBuf> {
    path.parent().map(|parent| {
        if parent.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            parent.to_path_buf()
        }
    })
}

fn display_optional(path: Option<&Path>, unavailable: &str) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| unavailable.to_string())
}

fn json_path(path: Option<&Path>) -> Value {
    path.map(|path| Value::String(path.display().to_string()))
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("temper-cli-config-paths-{tag}-{pid}-{nonce}"));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn env_with_home(home: &Path) -> EnvMap {
        let mut env = EnvMap::new();
        env.insert("HOME", home.to_string_lossy().into_owned());
        env
    }

    fn toml_path(path: &Path) -> String {
        path.display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    }

    #[test]
    fn report_resolves_explicit_bundle_paths_and_configured_runtime_paths() {
        let dir = scratch("explicit-bundle");
        let bundle = dir.join("bundle");
        std::fs::create_dir_all(&bundle).expect("create bundle");
        let home = dir.join("home");
        let workspace = home.join("workspaces");
        let workflow = home.join("flows").join("workflow.json");
        let config = format!(
            "schema_version = 1\n\
             [engine]\n\
             workflow = \"{}\"\n\
             [worker]\n\
             workspace = \"{}\"\n",
            toml_path(&workflow),
            toml_path(&workspace),
        );
        std::fs::write(bundle.join("config.toml"), config).expect("write config");

        let env = env_with_home(&home);
        let base_paths = PathResolver::from_env(&env);
        let report = PathReport::resolve(
            &LoadOptions {
                config: Some(bundle.clone()),
                credentials: None,
            },
            &env,
            &base_paths,
        )
        .expect("report resolves");

        assert_eq!(report.config_root.as_deref(), Some(bundle.as_path()));
        assert_eq!(
            report.config_file.as_deref(),
            Some(bundle.join("config.toml").as_path())
        );
        assert_eq!(
            report.credentials_source.as_deref(),
            Some(bundle.join("credentials.toml").as_path())
        );
        assert_eq!(
            report.state_dir.as_deref(),
            Some(home.join(".local/state/temper").as_path())
        );
        assert_eq!(report.workspace_dir, workspace);
        assert_eq!(report.workflow_file.as_deref(), Some(workflow.as_path()));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_explicit_bundle_still_reports_candidate_file_paths() {
        let dir = scratch("missing-explicit-bundle");
        let bundle = dir.join("bundle");
        let home = dir.join("home");
        let env = env_with_home(&home);
        let base_paths = PathResolver::from_env(&env);

        let report = PathReport::resolve(
            &LoadOptions {
                config: Some(bundle.clone()),
                credentials: None,
            },
            &env,
            &base_paths,
        )
        .expect("missing explicit config is still reportable");

        assert_eq!(report.config_root.as_deref(), Some(bundle.as_path()));
        assert_eq!(
            report.config_file.as_deref(),
            Some(bundle.join("config.toml").as_path())
        );
        assert_eq!(
            report.credentials_source.as_deref(),
            Some(bundle.join("credentials.toml").as_path())
        );
        assert_eq!(report.workflow_file, None);
        assert_eq!(
            report.workspace_dir,
            home.join(".local/state/temper/workspace")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn json_uses_required_snake_case_keys_and_null_for_unavailable_paths() {
        let env = EnvMap::new();
        let report = PathReport::resolve(&LoadOptions::default(), &env, &PathResolver::default())
            .expect("empty inputs resolve to path report");

        let rendered = report.render_json().expect("json renders");
        let value: Value = serde_json::from_str(&rendered).expect("valid json");

        assert_eq!(value["config_root"], Value::Null);
        assert_eq!(value["config_file"], Value::Null);
        assert_eq!(value["credentials_source"], Value::Null);
        assert_eq!(value["state_dir"], Value::Null);
        assert_eq!(value["workspace_dir"], ".temper/workspace");
        assert_eq!(value["workflow_file"], Value::Null);
    }
}

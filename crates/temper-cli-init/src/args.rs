// SPDX-License-Identifier: MPL-2.0

//! Argument-only parsing and flag value types for `temper init`.

use std::path::PathBuf;

use temper_cli_common::{LoadOptions, next_value};

use crate::collect::PROVIDER_DEEPSEEK;

/// The local topology shape `temper init` can prepare today.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum InitTopology {
    /// A single local process hosts the engine, worker, and agent.
    #[default]
    Standalone,
}

impl InitTopology {
    /// Parses the `--topology` flag. Distributed deployments are intentionally
    /// out of scope for the first-run local developer path.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "standalone" => Ok(Self::Standalone),
            "distributed" => Err("distributed topology is not implemented yet; \
                 use `--topology standalone` for local developer onboarding"
                .to_string()),
            other => Err(format!(
                "unknown topology `{other}`; only `standalone` is supported"
            )),
        }
    }
}

/// A repository selected for the initialized deployment.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RepoSelection {
    /// Repository owner or organization.
    pub owner: String,
    /// Repository name.
    pub name: String,
}

impl RepoSelection {
    /// Parses `owner/name`, rejecting partial paths so provisioning and config
    /// artifacts agree on a single managed repository.
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut parts = value.split('/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(owner), Some(name), None) if !owner.is_empty() && !name.is_empty() => Ok(Self {
                owner: owner.to_string(),
                name: name.to_string(),
            }),
            _ => Err(
                "--repo must be an owner/name repository path (for example acme/service)"
                    .to_string(),
            ),
        }
    }

    /// The `owner/name` path written into config artifacts.
    pub fn path(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// Non-interactive values supplied by local-dev `temper init` flags.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct InitOverrides {
    /// Forgejo base URL supplied by `--forge`, skipping the forge prompt.
    pub forge_url: Option<String>,
    /// Daemon bind / webhook advertise address supplied by `--bind`, skipping
    /// the webhook prompt.
    pub bind: Option<String>,
    /// Managed repository supplied by `--repo`.
    pub repo: Option<RepoSelection>,
    /// Workflow supplied by `--workflow`: a builtin name or a JSON/YAML file path.
    pub workflow: Option<String>,
    /// Provider supplied by `--provider` (only `deepseek` is accepted today).
    pub provider: Option<String>,
    /// Forgejo admin username supplied by `--admin-user` (only non-interactive).
    pub admin_user: Option<String>,
    /// Forgejo admin password from `TEMPER_INIT_ADMIN_PASSWORD` (only non-interactive).
    pub admin_password: Option<String>,
    /// LLM provider API key from `TEMPER_INIT_PROVIDER_KEY` (only non-interactive).
    pub provider_key: Option<String>,
    /// LLM provider base URL override for `[agent.providers.<name>].url`.
    pub provider_url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ParsedInitArgs {
    pub(crate) help: bool,
    pub(crate) options: LoadOptions,
    pub(crate) force: bool,
    pub(crate) existing_repo: bool,
    pub(crate) apply: bool,
    pub(crate) yes: bool,
    pub(crate) topology: InitTopology,
    pub(crate) overrides: InitOverrides,
    pub(crate) workspace: Option<PathBuf>,
    pub(crate) non_interactive: bool,
}

pub(crate) fn parse_init_args(
    args: Vec<String>,
    options: LoadOptions,
) -> Result<ParsedInitArgs, String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(ParsedInitArgs {
            help: true,
            options,
            ..Default::default()
        });
    }

    let mut parsed = ParsedInitArgs {
        options,
        ..Default::default()
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--force" => parsed.force = true,
            "--existing-repo" => parsed.existing_repo = true,
            "--apply" => parsed.apply = true,
            "--yes" => parsed.yes = true,
            "--topology" => {
                let topology = init_value(&mut rest, "--topology")?;
                parsed.topology = InitTopology::parse(&topology)?;
            }
            "--repo" => {
                let repo = init_value(&mut rest, "--repo")?;
                parsed.overrides.repo = Some(RepoSelection::parse(&repo)?);
            }
            "--workflow" => {
                parsed.overrides.workflow = Some(init_value(&mut rest, "--workflow")?);
            }
            "--forge" => {
                parsed.overrides.forge_url = Some(init_value(&mut rest, "--forge")?);
            }
            "--bind" => {
                parsed.overrides.bind = Some(init_value(&mut rest, "--bind")?);
            }
            "--workspace" => {
                parsed.workspace = Some(PathBuf::from(init_value(&mut rest, "--workspace")?));
            }
            "--provider" => {
                let provider = init_value(&mut rest, "--provider")?;
                if provider != PROVIDER_DEEPSEEK {
                    return Err(format!(
                        "unsupported provider `{provider}`; only `{PROVIDER_DEEPSEEK}` is supported"
                    ));
                }
                parsed.overrides.provider = Some(provider);
            }
            "--non-interactive" => parsed.non_interactive = true,
            "--admin-user" => {
                parsed.overrides.admin_user = Some(init_value(&mut rest, "--admin-user")?);
            }
            "--provider-url" => {
                parsed.overrides.provider_url = Some(init_value(&mut rest, "--provider-url")?);
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(parsed)
}

fn init_value<'a>(
    iter: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<String, String> {
    let value = next_value(iter, flag)?;
    if value.starts_with("--") {
        Err(format!("{flag} requires a value"))
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: Vec<String>) -> Result<ParsedInitArgs, String> {
        parse_init_args(args, LoadOptions::default())
    }

    #[test]
    fn parse_accepts_local_dev_flags_and_global_options() {
        let parsed = parse_init_args(
            vec![
                "--topology".to_string(),
                "standalone".to_string(),
                "--repo".to_string(),
                "acme/widget".to_string(),
                "--workflow".to_string(),
                "reference-delivery".to_string(),
                "--forge".to_string(),
                "http://localhost:3000".to_string(),
                "--workspace".to_string(),
                "/tmp/temper-workspaces".to_string(),
                "--provider".to_string(),
                "deepseek".to_string(),
                "--non-interactive".to_string(),
                "--admin-user".to_string(),
                "myuser".to_string(),
                "--force".to_string(),
                "--existing-repo".to_string(),
                "--apply".to_string(),
                "--yes".to_string(),
            ],
            LoadOptions {
                config: Some(PathBuf::from("config.toml")),
                credentials: Some(PathBuf::from("credentials.toml")),
            },
        )
        .expect("flags parse");

        assert!(!parsed.help);
        assert_eq!(parsed.options.config, Some(PathBuf::from("config.toml")));
        assert_eq!(
            parsed.options.credentials,
            Some(PathBuf::from("credentials.toml"))
        );
        assert!(parsed.force);
        assert!(parsed.existing_repo);
        assert!(parsed.apply);
        assert!(parsed.yes);
        assert!(parsed.non_interactive);
        assert_eq!(parsed.overrides.admin_user.as_deref(), Some("myuser"));
        assert_eq!(parsed.topology, InitTopology::Standalone);
        assert_eq!(
            parsed.overrides.repo,
            Some(RepoSelection {
                owner: "acme".to_string(),
                name: "widget".to_string(),
            })
        );
        assert_eq!(
            parsed.overrides.forge_url.as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            parsed.overrides.workflow.as_deref(),
            Some("reference-delivery")
        );
        assert_eq!(parsed.overrides.provider.as_deref(), Some("deepseek"));
        assert_eq!(parsed.overrides.bind, None);
        assert_eq!(
            parsed.workspace,
            Some(PathBuf::from("/tmp/temper-workspaces"))
        );
    }

    #[test]
    fn parse_rejects_local_config_and_secrets_flags() {
        let err = parse(vec!["--config".to_string(), "config.toml".to_string()])
            .expect_err("--config is global-only");
        assert!(err.contains("--config"), "{err}");

        let err = parse(vec![
            "--secrets".to_string(),
            "credentials.toml".to_string(),
        ])
        .expect_err("--secrets is global-only");
        assert!(err.contains("--secrets"), "{err}");
    }

    #[test]
    fn parse_bind_flag() {
        let parsed =
            parse(vec!["--bind".to_string(), "127.0.0.1:38100".to_string()]).expect("parse");

        assert_eq!(parsed.overrides.bind.as_deref(), Some("127.0.0.1:38100"));
    }

    #[test]
    fn parse_bind_absent_by_default() {
        let parsed = parse(Vec::new()).expect("parse");

        assert_eq!(parsed.overrides.bind, None);
    }

    #[test]
    fn parse_workspace_flag() {
        let parsed = parse(vec![
            "--workspace".to_string(),
            "./run/workspaces".to_string(),
        ])
        .expect("parse");

        assert_eq!(parsed.workspace, Some(PathBuf::from("./run/workspaces")));
    }

    #[test]
    fn bind_without_value_fails() {
        let err = parse(vec!["--bind".to_string()]).expect_err("--bind requires a value");
        assert!(err.contains("requires a value"), "{err}");

        let err = parse(vec!["--bind".to_string(), "--force".to_string()])
            .expect_err("--bind requires a value before another flag");
        assert!(err.contains("requires a value"), "{err}");
    }

    #[test]
    fn parse_rejects_distributed_topology_for_now() {
        let err = parse(vec!["--topology".to_string(), "distributed".to_string()])
            .expect_err("distributed is out of scope");
        assert!(
            err.contains("distributed topology is not implemented yet"),
            "{err}"
        );
    }

    #[test]
    fn parse_rejects_repo_without_owner_and_name() {
        let err = parse(vec!["--repo".to_string(), "service".to_string()])
            .expect_err("repo requires owner/name");
        assert!(err.contains("owner/name"), "{err}");
    }

    #[test]
    fn parse_accepts_non_interactive_and_admin_user() {
        let parsed = parse(vec![
            "--non-interactive".to_string(),
            "--admin-user".to_string(),
            "myuser".to_string(),
        ])
        .expect("parse");
        assert!(parsed.non_interactive);
        assert_eq!(parsed.overrides.admin_user.as_deref(), Some("myuser"));
    }

    #[test]
    fn parse_provider_url_flag() {
        let parsed = parse(vec![
            "--provider".to_string(),
            "deepseek".to_string(),
            "--provider-url".to_string(),
            "http://localhost:9999/v1".to_string(),
        ])
        .expect("parse");
        assert_eq!(parsed.overrides.provider.as_deref(), Some("deepseek"));
        assert_eq!(
            parsed.overrides.provider_url.as_deref(),
            Some("http://localhost:9999/v1")
        );
    }

    #[test]
    fn admin_user_without_value_fails() {
        let err = parse(vec!["--admin-user".to_string(), "--force".to_string()])
            .expect_err("--admin-user requires a value");
        assert!(err.contains("requires a value"), "{err}");
    }

    #[test]
    fn workspace_without_value_fails() {
        let err = parse(vec!["--workspace".to_string(), "--force".to_string()])
            .expect_err("--workspace requires a value");
        assert!(err.contains("requires a value"), "{err}");
    }
}

// SPDX-License-Identifier: MPL-2.0

//! Argument-only parsing and flag value types for `temper init`.

use std::path::PathBuf;

use temper_cli_common::{LoadOptions, next_value};

/// Provider choices accepted by `temper init` intake.
pub const PROVIDER_ANTHROPIC: &str = "anthropic";
pub const PROVIDER_CHATGPT: &str = "chatgpt";
pub const PROVIDER_DEEPSEEK: &str = "deepseek";
pub const PROVIDER_NONE: &str = "none";

/// The local topology shape `temper init` can prepare.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum InitTopology {
    /// A single local process hosts the engine, worker, and agent.
    #[default]
    Standalone,
    /// Separate engine/worker/agent processes. Parsed and collected for target
    /// bundles; the current emitted files remain compatible with standalone
    /// local development until the target-bundle writer lands.
    Distributed,
}

impl InitTopology {
    /// Parses the `--topology` flag / answers-file field.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "standalone" => Ok(Self::Standalone),
            "distributed" => Ok(Self::Distributed),
            other => Err(format!(
                "unknown topology `{other}`; expected `standalone` or `distributed`"
            )),
        }
    }

    /// The config / answers-file spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::Distributed => "distributed",
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
    /// artifacts agree on a single managed repository path.
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

/// Values supplied by local-dev `temper init` flags, answers files, and
/// non-interactive secret environment variables.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct InitOverrides {
    /// Forgejo base URL supplied by `--forge` / answers file, skipping the forge
    /// prompt.
    pub forge_url: Option<String>,
    /// Daemon bind / webhook advertise address supplied by `--bind` / answers
    /// file, skipping the webhook prompt.
    pub bind: Option<String>,
    /// Managed repositories supplied by repeatable `--repo` or `repos = [...]`.
    pub repos: Vec<RepoSelection>,
    /// Workflow supplied by `--workflow`: a builtin name or a JSON/YAML file path.
    pub workflow: Option<String>,
    /// Provider supplied by `--provider` / answers file.
    pub provider: Option<String>,
    /// Forgejo admin username supplied by `--admin-user`, skipping the admin prompt.
    pub admin_user: Option<String>,
    /// Forgejo admin password from answers file or `TEMPER_INIT_ADMIN_PASSWORD`
    /// (only honoured in non-interactive collection).
    pub admin_password: Option<String>,
    /// LLM provider API key from answers file or `TEMPER_INIT_PROVIDER_KEY`
    /// (only honoured in non-interactive collection, and only for `deepseek`).
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
    pub(crate) topology: Option<InitTopology>,
    pub(crate) overrides: InitOverrides,
    pub(crate) workspace: Option<PathBuf>,
    pub(crate) non_interactive: bool,
    pub(crate) answers: Option<PathBuf>,
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
                parsed.topology = Some(InitTopology::parse(&topology)?);
            }
            "--repo" => {
                let repo = init_value(&mut rest, "--repo")?;
                parsed.overrides.repos.push(RepoSelection::parse(&repo)?);
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
                parsed.overrides.provider = Some(parse_provider_choice(&provider)?);
            }
            "--provider-url" => {
                parsed.overrides.provider_url = Some(init_value(&mut rest, "--provider-url")?);
            }
            "--answers" => {
                parsed.answers = Some(PathBuf::from(init_value(&mut rest, "--answers")?));
            }
            "--non-interactive" => parsed.non_interactive = true,
            "--admin-user" => {
                parsed.overrides.admin_user = Some(init_value(&mut rest, "--admin-user")?);
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(parsed)
}

/// Parses and validates a provider choice while preserving the public spelling.
pub(crate) fn parse_provider_choice(provider: &str) -> Result<String, String> {
    match provider {
        PROVIDER_ANTHROPIC | PROVIDER_CHATGPT | PROVIDER_DEEPSEEK | PROVIDER_NONE => {
            Ok(provider.to_string())
        }
        other => Err(format!(
            "unsupported provider `{other}`; expected `{PROVIDER_ANTHROPIC}`, \
             `{PROVIDER_CHATGPT}`, `{PROVIDER_DEEPSEEK}`, or `{PROVIDER_NONE}`"
        )),
    }
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
                "distributed".to_string(),
                "--repo".to_string(),
                "acme/widget".to_string(),
                "--repo".to_string(),
                "acme/docs".to_string(),
                "--workflow".to_string(),
                "reference-delivery".to_string(),
                "--forge".to_string(),
                "http://localhost:3000".to_string(),
                "--workspace".to_string(),
                "/tmp/temper-workspaces".to_string(),
                "--provider".to_string(),
                "anthropic".to_string(),
                "--answers".to_string(),
                "answers.toml".to_string(),
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
        assert_eq!(parsed.answers, Some(PathBuf::from("answers.toml")));
        assert_eq!(parsed.overrides.admin_user.as_deref(), Some("myuser"));
        assert_eq!(parsed.topology, Some(InitTopology::Distributed));
        assert_eq!(
            parsed.overrides.repos,
            vec![
                RepoSelection {
                    owner: "acme".to_string(),
                    name: "widget".to_string(),
                },
                RepoSelection {
                    owner: "acme".to_string(),
                    name: "docs".to_string(),
                }
            ]
        );
        assert_eq!(
            parsed.overrides.forge_url.as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            parsed.overrides.workflow.as_deref(),
            Some("reference-delivery")
        );
        assert_eq!(parsed.overrides.provider.as_deref(), Some("anthropic"));
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
    fn parse_answers_flag() {
        let parsed = parse(vec![
            "--answers".to_string(),
            "init-answers.toml".to_string(),
        ])
        .expect("parse");

        assert_eq!(parsed.answers, Some(PathBuf::from("init-answers.toml")));
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
    fn parse_accepts_topology_choices() {
        let parsed = parse(vec!["--topology".to_string(), "distributed".to_string()])
            .expect("distributed topology parses");
        assert_eq!(parsed.topology, Some(InitTopology::Distributed));

        let parsed = parse(vec!["--topology".to_string(), "standalone".to_string()])
            .expect("standalone topology parses");
        assert_eq!(parsed.topology, Some(InitTopology::Standalone));
    }

    #[test]
    fn parse_rejects_unknown_topology() {
        let err = parse(vec!["--topology".to_string(), "clustered".to_string()])
            .expect_err("unknown topology rejected");
        assert!(
            err.contains("standalone") && err.contains("distributed"),
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
    fn parse_accepts_admin_user_without_non_interactive() {
        let parsed = parse(vec!["--admin-user".to_string(), "myuser".to_string()]).expect("parse");
        assert!(!parsed.non_interactive);
        assert_eq!(parsed.overrides.admin_user.as_deref(), Some("myuser"));
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
    fn parse_accepts_provider_choices() {
        for provider in [
            PROVIDER_ANTHROPIC,
            PROVIDER_CHATGPT,
            PROVIDER_DEEPSEEK,
            PROVIDER_NONE,
        ] {
            let parsed = parse(vec!["--provider".to_string(), provider.to_string()])
                .expect("provider parses");
            assert_eq!(parsed.overrides.provider.as_deref(), Some(provider));
        }
    }

    #[test]
    fn parse_rejects_unknown_provider() {
        let err = parse(vec!["--provider".to_string(), "ollama".to_string()])
            .expect_err("unknown provider rejected");
        assert!(err.contains("anthropic"), "{err}");
        assert!(err.contains("chatgpt"), "{err}");
        assert!(err.contains("deepseek"), "{err}");
        assert!(err.contains("none"), "{err}");
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

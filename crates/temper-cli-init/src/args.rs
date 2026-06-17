// SPDX-License-Identifier: MPL-2.0

//! Argument-only parsing and flag value types for `temper init`.

use temper_cli_common::{LoadOptions, next_value, parse_common_args};

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
    /// Managed repository supplied by `--repo`.
    pub repo: Option<RepoSelection>,
    /// Provider supplied by `--provider` (only `deepseek` is accepted today).
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ParsedInitArgs {
    pub(crate) help: bool,
    pub(crate) options: LoadOptions,
    pub(crate) force: bool,
    pub(crate) existing_repo: bool,
    pub(crate) topology: InitTopology,
    pub(crate) overrides: InitOverrides,
}

pub(crate) fn parse_init_args(args: Vec<String>) -> Result<ParsedInitArgs, String> {
    let common = parse_common_args(args)?;
    if common.help {
        return Ok(ParsedInitArgs {
            help: true,
            options: common.options,
            ..Default::default()
        });
    }

    let mut parsed = ParsedInitArgs {
        options: common.options,
        ..Default::default()
    };
    let mut rest = common.rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--force" => parsed.force = true,
            "--existing-repo" => parsed.existing_repo = true,
            "--topology" => {
                let topology = init_value(&mut rest, "--topology")?;
                parsed.topology = InitTopology::parse(&topology)?;
            }
            "--repo" => {
                let repo = init_value(&mut rest, "--repo")?;
                parsed.overrides.repo = Some(RepoSelection::parse(&repo)?);
            }
            "--forge" => {
                parsed.overrides.forge_url = Some(init_value(&mut rest, "--forge")?);
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

    #[test]
    fn parse_accepts_local_dev_flags() {
        let parsed = parse_init_args(vec![
            "--config".to_string(),
            "config.toml".to_string(),
            "--credentials".to_string(),
            "credentials.toml".to_string(),
            "--topology".to_string(),
            "standalone".to_string(),
            "--repo".to_string(),
            "acme/widget".to_string(),
            "--forge".to_string(),
            "http://localhost:3000".to_string(),
            "--provider".to_string(),
            "deepseek".to_string(),
            "--force".to_string(),
            "--existing-repo".to_string(),
        ])
        .expect("flags parse");

        assert!(!parsed.help);
        assert!(parsed.force);
        assert!(parsed.existing_repo);
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
        assert_eq!(parsed.overrides.provider.as_deref(), Some("deepseek"));
    }

    #[test]
    fn parse_rejects_distributed_topology_for_now() {
        let err = parse_init_args(vec!["--topology".to_string(), "distributed".to_string()])
            .expect_err("distributed is out of scope");
        assert!(
            err.contains("distributed topology is not implemented yet"),
            "{err}"
        );
    }

    #[test]
    fn parse_rejects_repo_without_owner_and_name() {
        let err = parse_init_args(vec!["--repo".to_string(), "service".to_string()])
            .expect_err("repo requires owner/name");
        assert!(err.contains("owner/name"), "{err}");
    }
}

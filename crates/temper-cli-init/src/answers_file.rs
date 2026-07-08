// SPDX-License-Identifier: MPL-2.0

//! TOML answers-file intake for reproducible `temper init` runs.
//!
//! The file supplies prompt-equivalent answers only. Operational side-effect
//! switches such as `--apply`, `--force`, and `--yes` deliberately are not part
//! of this schema, so an answers file can reproduce local collection without
//! authorizing forge mutations.

use std::path::Path;

use serde::Deserialize;

use crate::InitOverrides;
use crate::args::{InitTopology, RepoSelection, parse_provider_choice};

pub(crate) const ANSWERS_SCHEMA_VERSION: u32 = 1;

/// Validated answers-file contents, ready to merge with CLI flags.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(crate) struct AnswersFile {
    pub(crate) topology: Option<InitTopology>,
    pub(crate) overrides: InitOverrides,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnswersFile {
    schema_version: Option<u32>,
    topology: Option<String>,
    forge_url: Option<String>,
    workflow: Option<String>,
    webhook_addr: Option<String>,
    admin_user: Option<String>,
    admin_password: Option<String>,
    provider: Option<String>,
    provider_key: Option<String>,
    provider_url: Option<String>,
    repos: Option<Vec<String>>,
}

pub(crate) fn load_answers_file(path: &Path) -> Result<AnswersFile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read answers file {}: {error}", path.display()))?;
    let raw: RawAnswersFile = toml::from_str(&text)
        .map_err(|error| format!("parse answers file {}: {error}", path.display()))?;
    raw.into_answers()
        .map_err(|error| format!("answers file {}: {error}", path.display()))
}

impl RawAnswersFile {
    fn into_answers(self) -> Result<AnswersFile, String> {
        match self.schema_version {
            Some(ANSWERS_SCHEMA_VERSION) => {}
            Some(found) => {
                return Err(format!(
                    "unsupported schema_version {found}; expected {ANSWERS_SCHEMA_VERSION}"
                ));
            }
            None => {
                return Err(format!(
                    "missing schema_version; set schema_version = {ANSWERS_SCHEMA_VERSION}"
                ));
            }
        }

        let topology = self
            .topology
            .as_deref()
            .map(InitTopology::parse)
            .transpose()?;
        let repos = self.repos.map(parse_repos).transpose()?.unwrap_or_default();
        let provider = self
            .provider
            .as_deref()
            .map(parse_provider_choice)
            .transpose()?;

        Ok(AnswersFile {
            topology,
            overrides: InitOverrides {
                forge_url: non_blank(self.forge_url, "forge_url")?,
                bind: non_blank(self.webhook_addr, "webhook_addr")?,
                repos,
                workflow: non_blank(self.workflow, "workflow")?,
                provider,
                admin_user: non_blank(self.admin_user, "admin_user")?,
                admin_password: non_blank(self.admin_password, "admin_password")?,
                provider_key: non_blank(self.provider_key, "provider_key")?,
                provider_url: non_blank(self.provider_url, "provider_url")?,
            },
        })
    }
}

fn parse_repos(values: Vec<String>) -> Result<Vec<RepoSelection>, String> {
    if values.is_empty() {
        return Err("repos must contain at least one owner/name value when set".to_string());
    }
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let value = value.trim().to_string();
            if value.is_empty() {
                return Err(format!("repos[{index}] must not be empty"));
            }
            RepoSelection::parse(&value).map_err(|error| format!("repos[{index}]: {error}"))
        })
        .collect()
}

fn non_blank(value: Option<String>, field: &str) -> Result<Option<String>, String> {
    value
        .map(|value| {
            let value = value.trim().to_string();
            if value.is_empty() {
                Err(format!("{field} must not be empty when set"))
            } else {
                Ok(value)
            }
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<AnswersFile, String> {
        let raw: RawAnswersFile = toml::from_str(text).map_err(|error| error.to_string())?;
        raw.into_answers()
    }

    #[test]
    fn parses_full_answers_file_schema() {
        let answers = parse(
            r#"
schema_version = 1
topology = "distributed"
forge_url = "http://forge.local:3000"
workflow = "reference-delivery"
webhook_addr = "http://127.0.0.1:38100"
admin_user = "root"
admin_password = "admin-pw"
provider = "chatgpt"
provider_key = "unused-if-provider-overridden"
provider_url = "http://provider.local/v1"
repos = ["acme/service", "acme/docs"]
"#,
        )
        .expect("answers parse");

        assert_eq!(answers.topology, Some(InitTopology::Distributed));
        assert_eq!(
            answers.overrides.forge_url.as_deref(),
            Some("http://forge.local:3000")
        );
        assert_eq!(
            answers.overrides.workflow.as_deref(),
            Some("reference-delivery")
        );
        assert_eq!(
            answers.overrides.bind.as_deref(),
            Some("http://127.0.0.1:38100")
        );
        assert_eq!(answers.overrides.admin_user.as_deref(), Some("root"));
        assert_eq!(
            answers.overrides.admin_password.as_deref(),
            Some("admin-pw")
        );
        assert_eq!(answers.overrides.provider.as_deref(), Some("chatgpt"));
        assert_eq!(
            answers.overrides.provider_key.as_deref(),
            Some("unused-if-provider-overridden")
        );
        assert_eq!(
            answers.overrides.provider_url.as_deref(),
            Some("http://provider.local/v1")
        );
        assert_eq!(
            answers.overrides.repos,
            vec![
                RepoSelection {
                    owner: "acme".to_string(),
                    name: "service".to_string(),
                },
                RepoSelection {
                    owner: "acme".to_string(),
                    name: "docs".to_string(),
                }
            ]
        );
    }

    #[test]
    fn rejects_missing_schema_version() {
        let err = parse("forge_url = \"http://forge\"\n").expect_err("schema version required");
        assert!(err.contains("schema_version"), "{err}");
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let err = parse("schema_version = 2\n").expect_err("schema version checked");
        assert!(err.contains("unsupported schema_version 2"), "{err}");
    }

    #[test]
    fn rejects_unknown_fields_so_apply_stays_explicit() {
        let err =
            parse("schema_version = 1\napply = true\n").expect_err("unknown apply field rejected");
        assert!(
            err.contains("unknown field") && err.contains("apply"),
            "{err}"
        );
    }

    #[test]
    fn rejects_invalid_provider_and_repo() {
        let err =
            parse("schema_version = 1\nprovider = \"ollama\"\n").expect_err("provider rejected");
        assert!(err.contains("unsupported provider"), "{err}");

        let err = parse("schema_version = 1\nrepos = [\"service\"]\n").expect_err("repo rejected");
        assert!(
            err.contains("repos[0]") && err.contains("owner/name"),
            "{err}"
        );
    }
}

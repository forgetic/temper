//! Library backing the `harness-testing-provision` operator binary.
//!
//! This is the **operator** entry point for the real-world example (Track B of
//! `plans/real-world-example/`): given a running Forgejo and an admin token, it
//! runs the server-agnostic [`provision_world`](crate::forgejo_server::provision_world)
//! step, seeds one intake issue
//! ([`seed_intake_issue`](crate::forgejo_server::seed_intake_issue)), and writes
//! the per-role `{user, token, password}` to a POSIX-sourceable secrets file the
//! launch script reads.
//!
//! It deliberately reuses the existing provisioning library rather than
//! duplicating it; the only new behaviour here is argument parsing, the
//! credential/seed orchestration, and the secrets-file emission.
//!
//! # Secrets discipline
//!
//! The admin token arrives via the environment ([`ADMIN_TOKEN_ENV`]), never on
//! argv. Per-role tokens and passwords are written **only** to the `--out` file
//! (with `0600` permissions on Unix) and are never printed to stdout/stderr;
//! status output names only non-secret facts (repo, role count, issue number).

use crate::forgejo_server::{provision_world, seed_intake_issue, ProvisionError, Provisioned};
use crate::runner_config;
use std::fmt;
use std::path::PathBuf;

/// Environment variable carrying the Forgejo admin access token.
///
/// The token never travels on argv (other processes can read a command line);
/// the launch script mints it via the `forgejo` CLI and exports it here.
pub const ADMIN_TOKEN_ENV: &str = "HARNESS_FORGEJO_ADMIN_TOKEN";

/// One-line usage string for `--help` and error context.
pub const USAGE: &str = concat!(
    "harness-testing-provision --base-url <url> --owner <org> --name <repo> --out <path>\n",
    "  the admin token comes from the environment, never argv: ",
    "HARNESS_FORGEJO_ADMIN_TOKEN (required)",
);

/// Outcome of parsing the raw argument vector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    /// A fully validated provisioning invocation.
    Run(ProvisionArgs),
    /// `--help` was requested; print usage and exit zero.
    Help,
}

/// Fully parsed and validated operator-provisioning invocation.
#[derive(Clone, Eq, PartialEq)]
pub struct ProvisionArgs {
    /// Forgejo base URL, e.g. `http://127.0.0.1:3000`.
    pub base_url: String,
    /// Repository owner (the org provisioning creates/reuses).
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Path of the secrets file to write (sourced by the launch script).
    pub out: PathBuf,
    /// Admin access token (from [`ADMIN_TOKEN_ENV`]).
    pub admin_token: String,
}

impl fmt::Debug for ProvisionArgs {
    /// Redacts the admin token so a `{:?}` can never leak it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProvisionArgs")
            .field("base_url", &self.base_url)
            .field("owner", &self.owner)
            .field("name", &self.name)
            .field("out", &self.out)
            .field("admin_token", &"<redacted>")
            .finish()
    }
}

/// An argument-parsing failure with a user-facing message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArgsError(String);

impl ArgsError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ArgsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArgsError {}

/// Parses the process argument vector (excluding the program name).
///
/// The admin token is read from the process environment ([`ADMIN_TOKEN_ENV`]);
/// use [`parse_with_env`] to inject a lookup in tests.
pub fn parse<I>(args: I) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
{
    parse_with_env(args, |key| std::env::var(key).ok())
}

/// Parses arguments, reading the admin token through `env`.
pub fn parse_with_env<I, E>(args: I, env: E) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
    E: Fn(&str) -> Option<String>,
{
    let mut base_url = None;
    let mut owner = None;
    let mut name = None;
    let mut out = None;
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            "--base-url" => base_url = Some(value_for(&flag, &mut iter)?),
            "--owner" => owner = Some(value_for(&flag, &mut iter)?),
            "--name" => name = Some(value_for(&flag, &mut iter)?),
            "--out" => out = Some(value_for(&flag, &mut iter)?),
            other => {
                return Err(ArgsError::new(format!(
                    "unrecognized argument '{other}'\nusage: {USAGE}"
                )))
            }
        }
    }
    let admin_token = env(ADMIN_TOKEN_ENV)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ArgsError::new(format!(
                "missing required environment variable {ADMIN_TOKEN_ENV}"
            ))
        })?;
    Ok(ParseOutcome::Run(ProvisionArgs {
        base_url: require(base_url, "--base-url")?,
        owner: require(owner, "--owner")?,
        name: require(name, "--name")?,
        out: PathBuf::from(require(out, "--out")?),
        admin_token,
    }))
}

fn value_for<I>(flag: &str, iter: &mut I) -> Result<String, ArgsError>
where
    I: Iterator<Item = String>,
{
    iter.next()
        .ok_or_else(|| ArgsError::new(format!("flag '{flag}' expects a value")))
}

fn require(value: Option<String>, flag: &str) -> Result<String, ArgsError> {
    value.ok_or_else(|| ArgsError::new(format!("missing required {flag}\nusage: {USAGE}")))
}

/// A non-zero-exit failure from the operator binary.
#[derive(Debug)]
pub enum RunError {
    /// Building the async runtime failed.
    Runtime(String),
    /// Provisioning or seeding failed.
    Provision(ProvisionError),
    /// Writing the secrets file failed.
    Io(std::io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Runtime(why) => write!(formatter, "failed to start async runtime: {why}"),
            RunError::Provision(err) => write!(formatter, "{err}"),
            RunError::Io(err) => write!(formatter, "writing secrets file failed: {err}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<ProvisionError> for RunError {
    fn from(err: ProvisionError) -> Self {
        RunError::Provision(err)
    }
}

impl From<std::io::Error> for RunError {
    fn from(err: std::io::Error) -> Self {
        RunError::Io(err)
    }
}

/// Runs the full operator provision + seed, then writes the secrets file.
///
/// Returns a short, secret-free status line for the caller to print. Role
/// bindings and the default branch come from [`runner_config`], so role logins
/// stay derived from config and are never hardcoded.
pub fn run(args: &ProvisionArgs) -> Result<String, RunError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| RunError::Runtime(err.to_string()))?;

    let config = runner_config();
    let provisioned = runtime.block_on(provision_world(
        &args.base_url,
        &args.admin_token,
        &args.owner,
        &args.name,
        &config.role_bindings,
        &config.repository.default_branch,
    ))?;

    let issue = runtime.block_on(seed_intake_issue(
        &args.base_url,
        &args.admin_token,
        &args.owner,
        &args.name,
    ))?;

    write_secrets_file(&args.out, &format_secrets_env(&provisioned))?;

    Ok(format!(
        "provisioned {}/{}: {} role(s), intake issue #{}; secrets written to {}",
        provisioned.owner,
        provisioned.name,
        provisioned.roles.len(),
        issue,
        args.out.display(),
    ))
}

/// Formats the per-role secrets as a POSIX-sourceable env file.
///
/// Each role contributes `HARNESS_FORGEJO_USER_<ROLE>`,
/// `HARNESS_FORGEJO_TOKEN_<ROLE>`, and `HARNESS_FORGEJO_PASSWORD_<ROLE>` (role id
/// uppercased, non-alphanumerics replaced with `_`). Owner/repo are emitted for
/// the script's convenience. Values are single-quoted (with embedded quotes
/// escaped) so they source safely under `sh`. The admin token is **not** written
/// — the launch script owns it.
pub fn format_secrets_env(provisioned: &Provisioned) -> String {
    let mut out = String::new();
    out.push_str("# Generated by harness-testing-provision — live credentials, do not commit.\n");
    out.push_str(&format!(
        "HARNESS_FORGEJO_OWNER={}\n",
        sh_quote(&provisioned.owner)
    ));
    out.push_str(&format!(
        "HARNESS_FORGEJO_REPO={}\n",
        sh_quote(&provisioned.name)
    ));
    // `roles` is a BTreeMap, so iteration is deterministic (role-id ordered).
    for (role, identity) in &provisioned.roles {
        let key = env_role_key(role.as_str());
        out.push_str(&format!(
            "HARNESS_FORGEJO_USER_{key}={}\n",
            sh_quote(&identity.user)
        ));
        out.push_str(&format!(
            "HARNESS_FORGEJO_TOKEN_{key}={}\n",
            sh_quote(&identity.token)
        ));
        out.push_str(&format!(
            "HARNESS_FORGEJO_PASSWORD_{key}={}\n",
            sh_quote(&identity.password)
        ));
    }
    out
}

/// Uppercases a role id and replaces every non-alphanumeric character with `_`
/// so it is a valid POSIX shell variable-name suffix.
fn env_role_key(role: &str) -> String {
    role.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Single-quotes a value for safe `sh` sourcing, escaping embedded single quotes
/// with the standard `'\''` sequence.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Writes `contents` to `path`, creating parent dirs, with `0600` perms on Unix.
fn write_secrets_file(path: &PathBuf, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, contents)?;
    restrict_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(path: &PathBuf) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &PathBuf) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forgejo_server::RoleIdentity;
    use harness_forge::RepositoryId;
    use harness_workflow::RoleId;
    use std::collections::BTreeMap;

    fn sample_provisioned() -> Provisioned {
        let mut roles = BTreeMap::new();
        roles.insert(
            RoleId::new("engineer"),
            RoleIdentity {
                user: "engineer".into(),
                email: "engineer@example.invalid".into(),
                token: "tok-engineer".into(),
                password: "pw-with-'-quote".into(),
            },
        );
        roles.insert(
            RoleId::new("architect"),
            RoleIdentity {
                user: "architect".into(),
                email: "architect@example.invalid".into(),
                token: "tok-architect".into(),
                password: "pw-architect".into(),
            },
        );
        Provisioned {
            admin_token: "admin-secret".into(),
            owner: "acme".into(),
            name: "service".into(),
            repository: RepositoryId::new("acme/service"),
            roles,
        }
    }

    #[test]
    fn secrets_env_is_sourceable_and_role_keyed() {
        let env = format_secrets_env(&sample_provisioned());
        // Deterministic, role-id ordered (architect before engineer).
        assert!(env.contains("HARNESS_FORGEJO_OWNER='acme'\n"));
        assert!(env.contains("HARNESS_FORGEJO_REPO='service'\n"));
        assert!(env.contains("HARNESS_FORGEJO_USER_ARCHITECT='architect'\n"));
        assert!(env.contains("HARNESS_FORGEJO_TOKEN_ARCHITECT='tok-architect'\n"));
        assert!(env.contains("HARNESS_FORGEJO_TOKEN_ENGINEER='tok-engineer'\n"));
        let architect = env.find("ARCHITECT").expect("architect present");
        let engineer = env.find("ENGINEER").expect("engineer present");
        assert!(architect < engineer, "roles emitted in BTreeMap order");
    }

    #[test]
    fn secrets_env_never_writes_the_admin_token() {
        let env = format_secrets_env(&sample_provisioned());
        assert!(
            !env.contains("admin-secret"),
            "admin token must not be written to the secrets file",
        );
    }

    #[test]
    fn secrets_env_escapes_single_quotes_in_values() {
        let env = format_secrets_env(&sample_provisioned());
        // A literal single quote in a password becomes the `'\''` sequence.
        assert!(env.contains(r"HARNESS_FORGEJO_PASSWORD_ENGINEER='pw-with-'\''-quote'"));
    }

    #[test]
    fn env_role_key_sanitizes_to_a_shell_name_suffix() {
        assert_eq!(env_role_key("engineer"), "ENGINEER");
        assert_eq!(env_role_key("code-reviewer"), "CODE_REVIEWER");
    }

    #[test]
    fn parse_requires_admin_token_from_env() {
        let args = [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "secrets/roles.env",
        ]
        .into_iter()
        .map(String::from);
        let error = parse_with_env(args, |_| None).unwrap_err();
        assert!(error.to_string().contains(ADMIN_TOKEN_ENV));
    }

    #[test]
    fn parse_reads_token_from_env_and_keeps_it_off_debug() {
        let args = [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "secrets/roles.env",
        ]
        .into_iter()
        .map(String::from);
        let outcome = parse_with_env(args, |key| {
            (key == ADMIN_TOKEN_ENV).then(|| "super-secret-admin".to_string())
        })
        .expect("parses");
        let ParseOutcome::Run(parsed) = outcome else {
            panic!("expected a run outcome");
        };
        assert_eq!(parsed.admin_token, "super-secret-admin");
        let rendered = format!("{parsed:?}");
        assert!(!rendered.contains("super-secret-admin"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn parse_help_short_circuits() {
        let outcome = parse_with_env(["--help".to_string()], |_| None).expect("parses");
        assert_eq!(outcome, ParseOutcome::Help);
    }
}

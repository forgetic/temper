//! Reference-delivery workflow defaults shared by deployable Temper tools.
//!
//! This crate contains only lightweight demo/reference-delivery configuration:
//! the bundled workflow fixture, default repository input, role actor mapping,
//! and runner defaults. Runtime processes compose it with narrower production
//! crates instead of depending on an aggregate production crate.

use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::Duration;
use temper_forge::{CreateRepository, User, UserId};
use temper_runner::RunnerConfig;
use temper_workflow::{RawWorkflowSpec, ValidatedWorkflow};

mod forgejo_demo;

pub use forgejo_demo::{
    CI_PASS_MARKER, CI_WORKFLOW, DEFAULT_INTAKE_BODY, DEFAULT_INTAKE_TITLE, ci_seed_commits,
    ci_sentinel_commit,
};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

/// The bundled **basic-delivery** workflow JSON: the minimal,
/// no-human-in-the-loop reference shape (architect + engineer + mechanical;
/// CI-gated landing, no review gate). Embedded with the same `include_str!`
/// pattern as the reference-delivery default.
const BASIC_DELIVERY_FIXTURE: &str =
    include_str!("../../temper-workflow/fixtures/basic-delivery.json");

/// Failure loading a runtime-selected workflow from a file.
#[derive(Debug)]
pub enum WorkflowLoadError {
    /// The workflow file could not be read.
    Read {
        path: String,
        source: std::io::Error,
    },
    /// The workflow file is not valid JSON for [`RawWorkflowSpec`].
    Parse { path: String, detail: String },
    /// The workflow parsed but failed static validation.
    Validate { path: String, detail: String },
}

impl fmt::Display for WorkflowLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkflowLoadError::Read { path, source } => {
                write!(formatter, "failed to read workflow file {path}: {source}")
            }
            WorkflowLoadError::Parse { path, detail } => {
                write!(
                    formatter,
                    "workflow file {path} is not valid JSON: {detail}"
                )
            }
            WorkflowLoadError::Validate { path, detail } => {
                write!(
                    formatter,
                    "workflow file {path} failed validation:\n{detail}"
                )
            }
        }
    }
}

impl Error for WorkflowLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            WorkflowLoadError::Read { source, .. } => Some(source),
            WorkflowLoadError::Parse { .. } | WorkflowLoadError::Validate { .. } => None,
        }
    }
}

/// Loads the bundled reference-delivery workflow used by the demo binaries.
pub fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("fixture parses");
    spec.validate().expect("reference fixture validates")
}

/// The bundled **basic-delivery** workflow JSON, verbatim.
///
/// This is the canonical copy of `examples/basic-delivery/config/workflow.json`,
/// embedded at build time so a deployment carries it without a file on disk
/// (`temper init` writes deployments that reference it). A fixture-vs-example
/// equality test asserts the two cannot drift.
pub fn basic_delivery_workflow_json() -> &'static str {
    BASIC_DELIVERY_FIXTURE
}

/// Parses + validates the bundled [`basic_delivery_workflow_json`].
///
/// A malformed bundled fixture is a build/test bug (the fixture ships with the
/// crate), hence the panics rather than a `Result`.
pub fn basic_delivery_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery fixture parses");
    spec.validate().expect("basic-delivery fixture validates")
}

/// Loads and validates a workflow from `path`.
///
/// This is the runtime workflow source behind the binaries' `--workflow` flag.
/// Errors are reported against `path` so an operator can see which file failed
/// and why (read, parse, or validation).
pub fn load_workflow(path: impl AsRef<Path>) -> Result<ValidatedWorkflow, WorkflowLoadError> {
    let path = path.as_ref();
    let display = path.display().to_string();
    let contents = std::fs::read_to_string(path).map_err(|source| WorkflowLoadError::Read {
        path: display.clone(),
        source,
    })?;
    let spec: RawWorkflowSpec =
        serde_json::from_str(&contents).map_err(|error| WorkflowLoadError::Parse {
            path: display.clone(),
            detail: error.to_string(),
        })?;
    spec.validate()
        .map_err(|errors| WorkflowLoadError::Validate {
            path: display,
            detail: errors.to_string(),
        })
}

/// Resolves the workflow to operate against: the file at `path` when supplied,
/// otherwise the bundled reference-delivery fixture (back-compat default).
pub fn resolve_workflow(
    path: Option<impl AsRef<Path>>,
) -> Result<ValidatedWorkflow, WorkflowLoadError> {
    match path {
        Some(path) => load_workflow(path),
        None => Ok(workflow()),
    }
}

/// Reference-delivery repository input.
pub fn repo_input() -> CreateRepository {
    CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }
}

/// Builds a Forge user whose id and handle are identical.
pub fn actor_user(role: &str) -> User {
    User {
        id: UserId::new(role),
        handle: role.into(),
        display_name: None,
        email: None,
    }
}

/// Runner config shared by the reference-delivery binaries.
///
/// Role bindings are derived from workflow roles that subscribe to queues, so
/// adding a user-defined process role to the spec does not require another Rust
/// hard-coded id. Automation-only authorities such as `mechanical` have no role
/// worker or role-decision process. The demo provisioning convention keeps Forge
/// user id == role id.
pub fn runner_config() -> RunnerConfig {
    runner_config_for(&workflow(), repo_input())
}

/// Derives a runner config from any validated workflow and repository input.
///
/// Role→actor bindings come from the workflow's roles that subscribe to queues
/// (the demo provisioning convention keeps Forge user id == role id), so a
/// runtime-selected workflow binds its own roles without another hard-coded id.
/// Repository identity/branch come from the caller (the provision/worker CLI
/// args, or [`repo_input`] as a sane default).
pub fn runner_config_for(
    workflow: &ValidatedWorkflow,
    repository: CreateRepository,
) -> RunnerConfig {
    let mut config = RunnerConfig::new(repository)
        .with_lease_ttl(Duration::minutes(30))
        .with_poll_interval(Duration::seconds(1));
    for role in workflow
        .roles()
        .iter()
        .filter(|role| !role.queues.is_empty())
    {
        config.set_role_binding(role.id.clone(), actor_user(role.id.as_str()));
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE_FIXTURE_PATH: &str = "../temper-workflow/fixtures/reference-delivery.json";

    /// The canonical example workflow the bundled basic-delivery fixture mirrors,
    /// relative to this crate's manifest dir.
    const BASIC_DELIVERY_EXAMPLE_PATH: &str = "../../examples/basic-delivery/config/workflow.json";

    #[test]
    fn basic_delivery_workflow_parses_and_validates() {
        // Panics inside `basic_delivery_workflow` would fail the test; this also
        // confirms the embedded fixture is non-trivial.
        let workflow = basic_delivery_workflow();
        assert!(
            !workflow.roles().is_empty(),
            "basic-delivery should define roles"
        );
    }

    #[test]
    fn basic_delivery_fixture_matches_example() {
        // The bundled fixture and the example must be byte-for-byte identical so a
        // change to one cannot silently diverge from the other. Pick the example
        // as the canonical source and assert the embedded copy equals it.
        let example = std::fs::read_to_string(BASIC_DELIVERY_EXAMPLE_PATH)
            .expect("basic-delivery example workflow is present");
        assert_eq!(
            basic_delivery_workflow_json(),
            example,
            "the embedded basic-delivery fixture has drifted from \
             examples/basic-delivery/config/workflow.json; copy the example over \
             crates/temper-workflow/fixtures/basic-delivery.json"
        );
    }

    #[test]
    fn resolve_workflow_defaults_to_bundled_reference() {
        let resolved = resolve_workflow(None::<&Path>).expect("default resolves");
        assert_eq!(resolved, workflow());
    }

    #[test]
    fn load_workflow_reads_and_validates_a_file() {
        let loaded = load_workflow(REFERENCE_FIXTURE_PATH).expect("fixture loads");
        // The on-disk reference fixture is the same document the binaries bundle,
        // so an explicit `--workflow <reference>` reproduces the default exactly.
        assert_eq!(loaded, workflow());
    }

    #[test]
    fn resolve_workflow_loads_the_given_file() {
        let resolved =
            resolve_workflow(Some(REFERENCE_FIXTURE_PATH)).expect("explicit path resolves");
        assert_eq!(resolved, workflow());
    }

    #[test]
    fn load_workflow_reports_a_missing_file() {
        let error = load_workflow("/definitely/not/here.json").unwrap_err();
        assert!(matches!(error, WorkflowLoadError::Read { .. }));
        assert!(error.to_string().contains("/definitely/not/here.json"));
    }

    #[test]
    fn runner_config_for_binds_each_queue_subscribing_role() {
        let workflow = workflow();
        let config = runner_config_for(&workflow, repo_input());
        // Every role with at least one queue gets a binding whose Forge user id
        // equals the role id (the demo provisioning convention).
        for role in workflow.roles().iter().filter(|r| !r.queues.is_empty()) {
            let binding = config
                .role_binding(&role.id)
                .expect("queue-subscribing role is bound");
            assert_eq!(binding.user.id.as_str(), role.id.as_str());
        }
        assert_eq!(config.repository, repo_input());
    }

    #[test]
    fn runner_config_for_matches_legacy_runner_config() {
        assert_eq!(
            runner_config_for(&workflow(), repo_input()),
            runner_config()
        );
    }
}

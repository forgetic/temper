//! Process-layout-independent runner configuration.
//!
//! [`RunnerConfig`] names the repository a runner should operate on, binds each
//! workflow role to the Forge user it acts as, and carries runtime-only inputs
//! such as pull-request creation templates and cadence settings. The shape is
//! deliberately independent of topology: the same value can configure an
//! in-process [`crate::stage::InProcessStage`] or later one-worker-per-process
//! binaries.

use chrono::Duration;
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use temper_forge::{CreatePullRequest, CreateRepository, User};
use temper_workflow::{
    CompiledWorkflow, ExecutionContext, ExternalToolId, RoleId, RoleManifest, TransitionId,
};

/// A workflow role and the Forge user that acts for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleBinding {
    /// Workflow role id.
    pub role: RoleId,
    /// Forge user used by that role's worker.
    pub user: User,
}

/// Runtime input for one `CreatePullRequest` effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestCreateBinding {
    /// Role whose worker receives this create input.
    pub role: RoleId,
    /// Transition containing the `CreatePullRequest` effect.
    pub transition: TransitionId,
    /// Zero-based index among that transition's `CreatePullRequest` effects.
    pub effect_index: usize,
    /// Concrete Forge create input supplied at runtime.
    pub input: CreatePullRequest,
}

/// Runner metadata binding for one declared external tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalToolBinding {
    /// Workflow role whose worker may receive this external tool.
    pub role: RoleId,
    /// Declared role-level external tool id.
    pub tool: ExternalToolId,
    /// Provider/binding name selected by the runner, such as `workspace-local`.
    pub provider: String,
}

/// Error returned when runner external-tool bindings exceed workflow authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalToolBindingError {
    /// A required tool declaration has no matching runner binding.
    MissingRequired { role: RoleId, tool: ExternalToolId },
    /// A binding names a role absent from the compiled workflow.
    UnknownRole { role: RoleId },
    /// A binding names a tool the workflow role did not declare.
    UndeclaredTool { role: RoleId, tool: ExternalToolId },
    /// More than one binding targets the same role/tool pair.
    DuplicateBinding { role: RoleId, tool: ExternalToolId },
    /// An executable provider was registered without the corresponding runner
    /// metadata binding, so the prompt/context would not have declared it
    /// available.
    ExecutableWithoutBinding { role: RoleId, tool: ExternalToolId },
}

impl fmt::Display for ExternalToolBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExternalToolBindingError::MissingRequired { role, tool } => write!(
                formatter,
                "role `{role}` requires external tool `{tool}`, but no runner binding was configured"
            ),
            ExternalToolBindingError::UnknownRole { role } => write!(
                formatter,
                "external tool binding references undeclared role `{role}`"
            ),
            ExternalToolBindingError::UndeclaredTool { role, tool } => write!(
                formatter,
                "external tool binding `{tool}` for role `{role}` is not declared by the workflow"
            ),
            ExternalToolBindingError::DuplicateBinding { role, tool } => write!(
                formatter,
                "external tool `{tool}` for role `{role}` has multiple runner bindings"
            ),
            ExternalToolBindingError::ExecutableWithoutBinding { role, tool } => write!(
                formatter,
                "executable external tool `{tool}` for role `{role}` has no runner binding"
            ),
        }
    }
}

impl Error for ExternalToolBindingError {}

/// Configuration shared by every runner topology.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerConfig {
    /// Repository to create/open for the stage.
    pub repository: CreateRepository,
    /// Forge identity for each workflow role.
    pub role_bindings: Vec<RoleBinding>,
    /// Runtime pull-request create inputs, keyed by role and transition.
    pub pull_request_creates: Vec<PullRequestCreateBinding>,
    /// Runtime bindings for user-declared external tools.
    pub external_tools: Vec<ExternalToolBinding>,
    /// Lease time-to-live used by mechanical recovery.
    pub lease_ttl: Duration,
    /// Poll cadence used by the later production poll loop.
    pub poll_interval: Duration,
}

impl RunnerConfig {
    /// Creates a config with no role bindings and conservative local defaults.
    pub fn new(repository: CreateRepository) -> Self {
        Self {
            repository,
            role_bindings: Vec::new(),
            pull_request_creates: Vec::new(),
            external_tools: Vec::new(),
            lease_ttl: Duration::minutes(30),
            poll_interval: Duration::seconds(30),
        }
    }

    /// Adds or replaces the binding for `role`, returning `self` for chaining.
    pub fn with_role_binding(mut self, role: RoleId, user: User) -> Self {
        self.set_role_binding(role, user);
        self
    }

    /// Adds or replaces the binding for `role`.
    pub fn set_role_binding(&mut self, role: RoleId, user: User) -> &mut Self {
        if let Some(binding) = self
            .role_bindings
            .iter_mut()
            .find(|binding| binding.role == role)
        {
            binding.user = user;
        } else {
            self.role_bindings.push(RoleBinding { role, user });
        }
        self
    }

    /// Returns the configured binding for `role`, if present.
    pub fn role_binding(&self, role: &RoleId) -> Option<&RoleBinding> {
        self.role_bindings
            .iter()
            .find(|binding| &binding.role == role)
    }

    /// Adds a first-effect pull-request create binding for `role`.
    pub fn with_pull_request_create(
        self,
        role: RoleId,
        transition: TransitionId,
        input: CreatePullRequest,
    ) -> Self {
        self.with_pull_request_create_at(role, transition, 0, input)
    }

    /// Adds a pull-request create binding for `role` and `effect_index`.
    pub fn with_pull_request_create_at(
        mut self,
        role: RoleId,
        transition: TransitionId,
        effect_index: usize,
        input: CreatePullRequest,
    ) -> Self {
        self.pull_request_creates.push(PullRequestCreateBinding {
            role,
            transition,
            effect_index,
            input,
        });
        self
    }

    /// Adds an external tool binding for `role` and `tool`.
    pub fn with_external_tool_binding(
        mut self,
        role: RoleId,
        tool: ExternalToolId,
        provider: impl Into<String>,
    ) -> Self {
        self.set_external_tool_binding(role, tool, provider);
        self
    }

    /// Adds an external tool binding for `role` and `tool`.
    pub fn set_external_tool_binding(
        &mut self,
        role: RoleId,
        tool: ExternalToolId,
        provider: impl Into<String>,
    ) -> &mut Self {
        self.external_tools.push(ExternalToolBinding {
            role,
            tool,
            provider: provider.into(),
        });
        self
    }

    /// Returns whether `role`/`tool` has a runner metadata binding.
    pub fn has_external_tool_binding(&self, role: &RoleId, tool: &ExternalToolId) -> bool {
        self.external_tools
            .iter()
            .any(|binding| &binding.role == role && &binding.tool == tool)
    }

    /// Validates external-tool bindings for one role manifest.
    pub fn validate_role_external_tool_bindings(
        &self,
        role: &RoleManifest,
    ) -> Result<(), ExternalToolBindingError> {
        let role_bindings = self
            .external_tools
            .iter()
            .filter(|binding| binding.role == role.id)
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        for binding in &role_bindings {
            if !role
                .external_tools
                .iter()
                .any(|manifest| manifest.id == binding.tool)
            {
                return Err(ExternalToolBindingError::UndeclaredTool {
                    role: binding.role.clone(),
                    tool: binding.tool.clone(),
                });
            }
            if !seen.insert(binding.tool.clone()) {
                return Err(ExternalToolBindingError::DuplicateBinding {
                    role: binding.role.clone(),
                    tool: binding.tool.clone(),
                });
            }
        }

        for manifest in &role.external_tools {
            if manifest.required
                && !role_bindings
                    .iter()
                    .any(|binding| binding.tool == manifest.id)
            {
                return Err(ExternalToolBindingError::MissingRequired {
                    role: role.id.clone(),
                    tool: manifest.id.clone(),
                });
            }
        }
        Ok(())
    }

    /// Validates all external tool bindings against a compiled workflow.
    pub fn validate_external_tool_bindings(
        &self,
        compiled: &CompiledWorkflow,
    ) -> Result<(), ExternalToolBindingError> {
        for binding in &self.external_tools {
            if compiled.role(&binding.role).is_none() {
                return Err(ExternalToolBindingError::UnknownRole {
                    role: binding.role.clone(),
                });
            }
        }
        for role in compiled.roles() {
            self.validate_role_external_tool_bindings(role)?;
        }
        Ok(())
    }

    /// Sets the lease TTL.
    pub fn with_lease_ttl(mut self, lease_ttl: Duration) -> Self {
        self.lease_ttl = lease_ttl;
        self
    }

    /// Sets the poll interval used by poll-loop topologies.
    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// Builds the workflow execution context for a role worker.
    ///
    /// All role-to-user bindings are included because transitions may assign or
    /// request review from roles other than the caller. Pull-request create
    /// inputs are role-scoped, so only bindings for `role` are installed.
    pub fn execution_context(&self, role: &RoleId) -> ExecutionContext {
        let mut context = ExecutionContext::new();
        for binding in &self.role_bindings {
            context.set_assignee(binding.role.clone(), binding.user.id.clone());
        }
        for binding in self
            .pull_request_creates
            .iter()
            .filter(|binding| &binding.role == role)
        {
            context.set_pull_request_create_at(
                binding.transition.clone(),
                binding.effect_index,
                binding.input.clone(),
            );
        }
        context
    }
}

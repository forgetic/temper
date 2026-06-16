//! Per-role worker that scans active queues and delegates to an agent.

use super::{Progress, Worker, WorkerError};
use crate::agent::{Agent, RoleTools};
use crate::observability::{
    ScanSummaryEvent, WorkItemSelectedEvent, render_scan_summary_event,
    render_work_item_selected_event,
};
use crate::scan::{WorkItem, scan_role, scan_role_audit, scan_role_wake};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use temper_forge::{Forge, RepositoryId};
use temper_workflow::{CompiledWorkflow, ExecutionContext, RoleId, ValidatedWorkflow};

/// Per-role worker that scans active queues and delegates behavior to an agent.
pub struct RoleWorker<'a, F: Forge + ?Sized> {
    name: String,
    forge: &'a F,
    repo: &'a RepositoryId,
    workflow: &'a ValidatedWorkflow,
    compiled: &'a CompiledWorkflow,
    role: RoleId,
    agent: Arc<dyn Agent<F> + 'a>,
    tools: RoleTools<'a, F>,
}

impl<'a, F: Forge + ?Sized> RoleWorker<'a, F> {
    /// Creates a role worker with the default `role:<id>` name.
    pub fn new(
        workflow: &'a ValidatedWorkflow,
        compiled: &'a CompiledWorkflow,
        forge: &'a F,
        repo: &'a RepositoryId,
        role: RoleId,
        agent: Arc<dyn Agent<F> + 'a>,
        context: ExecutionContext,
    ) -> Self {
        let name = format!("role:{role}");
        let tools = RoleTools::new(workflow, forge, repo, role.clone(), context);
        Self {
            name,
            forge,
            repo,
            workflow,
            compiled,
            role,
            agent,
            tools,
        }
    }

    /// Workflow role serviced by this worker.
    pub fn role(&self) -> &RoleId {
        &self.role
    }

    /// Ticks this worker while attaching a production tick id to work-item logs.
    pub async fn tick_with_observability_tick_id(
        &self,
        now: DateTime<Utc>,
        tick_id: &str,
    ) -> Result<Progress, WorkerError> {
        let tools = self.tools_with_observability_tick_id(tick_id);
        self.tick_with_tools(now, &tools, RoleScanMode::Normal)
            .await
    }

    /// Runs an audit scan for this role.
    pub async fn tick_audit(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_with_tools(now, &self.tools, RoleScanMode::Audit)
            .await
    }

    /// Runs a wake scan for this role.
    pub async fn tick_wake(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_with_tools(now, &self.tools, RoleScanMode::Wake)
            .await
    }

    /// Runs a wake scan while attaching a production tick id to work-item logs.
    pub async fn tick_wake_with_observability_tick_id(
        &self,
        now: DateTime<Utc>,
        tick_id: &str,
    ) -> Result<Progress, WorkerError> {
        let tools = self.tools_with_observability_tick_id(tick_id);
        self.tick_with_tools(now, &tools, RoleScanMode::Wake).await
    }

    /// Runs an audit scan while attaching a production tick id to work-item logs.
    pub async fn tick_audit_with_observability_tick_id(
        &self,
        now: DateTime<Utc>,
        tick_id: &str,
    ) -> Result<Progress, WorkerError> {
        let tools = self.tools_with_observability_tick_id(tick_id);
        self.tick_with_tools(now, &tools, RoleScanMode::Audit).await
    }

    fn tools_with_observability_tick_id(&self, tick_id: &str) -> RoleTools<'_, F> {
        RoleTools::new(
            self.workflow,
            self.forge,
            self.repo,
            self.role.clone(),
            self.tools.execution_context(),
        )
        .with_observability_tick_id(tick_id.to_string())
    }

    async fn tick_with_tools(
        &self,
        now: DateTime<Utc>,
        tools: &RoleTools<'_, F>,
        mode: RoleScanMode,
    ) -> Result<Progress, WorkerError> {
        let items = match mode {
            RoleScanMode::Normal => {
                scan_role(
                    self.forge,
                    self.repo,
                    self.workflow,
                    self.compiled,
                    now,
                    &self.role,
                )
                .await?
            }
            RoleScanMode::Wake => {
                scan_role_wake(
                    self.forge,
                    self.repo,
                    self.workflow,
                    self.compiled,
                    now,
                    &self.role,
                )
                .await?
            }
            RoleScanMode::Audit => {
                scan_role_audit(
                    self.forge,
                    self.repo,
                    self.workflow,
                    self.compiled,
                    now,
                    &self.role,
                )
                .await?
            }
        };

        log_role_scan(
            &self.name,
            self.repo,
            self.workflow.name(),
            &self.role,
            tools,
            &items,
        );

        let mut progress = Progress::unchanged();
        for item in items {
            progress.record(self.agent.service(&item, tools).await?);
        }
        Ok(progress)
    }
}

#[derive(Clone, Copy)]
enum RoleScanMode {
    Normal,
    Wake,
    Audit,
}

#[async_trait]
impl<F: Forge + ?Sized> Worker for RoleWorker<'_, F> {
    async fn tick(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.tick_with_tools(now, &self.tools, RoleScanMode::Normal)
            .await
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn log_role_scan<F: Forge + ?Sized>(
    worker: &str,
    repo: &RepositoryId,
    workflow_id: &str,
    role: &RoleId,
    tools: &RoleTools<'_, F>,
    items: &[WorkItem],
) {
    let Some(tick_id) = tools.observability_tick_id() else {
        return;
    };
    if items.is_empty() {
        return;
    }
    tracing::info!(
        target: "temper_runner",
        "{}",
        render_scan_summary_event(&ScanSummaryEvent {
            tick_id: Some(tick_id),
            worker_kind: "role",
            worker,
            repo,
            workflow_id,
            role: Some(role.as_str()),
            work_item_count: items.len(),
        })
    );
    for item in items {
        let identity = tools.work_item_identity(item);
        tracing::info!(
            target: "temper_runner",
            "{}",
            render_work_item_selected_event(&WorkItemSelectedEvent {
                identity: &identity,
                workflow_id,
                worker,
            })
        );
    }
}

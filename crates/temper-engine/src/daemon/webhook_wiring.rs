// SPDX-License-Identifier: MPL-2.0

//! Webhook intake wiring for [`Daemon`]: installs a [`WakeScanner`] that, on
//! each verified delivery, runs the role-work wake scan and (optionally) an
//! immediate hinted mechanical pass.

use std::sync::Arc;

use temper_forge::Forge;
use temper_workflow::{CompiledWorkflow, ValidatedWorkflow};

use crate::RoleFeedTarget;
use crate::lease_applier::WallClock;
use crate::webhook::{WebhookConfig, run_wake_scan};

use super::machine::DaemonCompletion;
use super::{Daemon, HintedMechanical, WakeScanner};

impl Daemon {
    /// Enables `POST /forgejo/webhook` intake on this daemon's HTTP surface:
    /// deliveries are verified and parsed by the daemon machine, acknowledged
    /// with `202`, then executed as background wake scans against the given
    /// forge/workflow.
    pub fn with_webhook<F: Forge + Send + Sync + ?Sized + 'static>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        compiled: Arc<CompiledWorkflow>,
        config: Arc<WebhookConfig>,
        clock: WallClock,
    ) -> Self {
        self.with_webhook_and_mechanical(forge, workflow, compiled, config, clock, None)
    }

    /// Like [`Self::with_webhook`], but also drives a mechanical accelerator: each
    /// verified delivery runs the role-work wake scan **and** an immediate
    /// mechanical pass for the hinted repository. This is what lets the mechanical
    /// backstop cadence be slow (idle quiet) without losing reaction latency —
    /// the webhook is the edge-trigger (ADR 0009). Pass `None` to keep the
    /// wake-scan-only behavior.
    pub fn with_webhook_and_mechanical<F: Forge + Send + Sync + ?Sized + 'static>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        compiled: Arc<CompiledWorkflow>,
        config: Arc<WebhookConfig>,
        clock: WallClock,
        mechanical: Option<Arc<dyn HintedMechanical>>,
    ) -> Self {
        let wake_targets = config.targets.clone();
        let daemon =
            self.with_wake_scanner(forge, workflow, compiled, wake_targets, clock, mechanical);
        let _ = daemon.cq.send(DaemonCompletion::ConfigureWebhook {
            config: (*config).clone(),
        });
        daemon
    }

    pub(super) fn with_wake_scanner<F: Forge + Send + Sync + ?Sized + 'static>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        compiled: Arc<CompiledWorkflow>,
        wake_targets: Vec<RoleFeedTarget>,
        clock: WallClock,
        mechanical: Option<Arc<dyn HintedMechanical>>,
    ) -> Self {
        let scanner = Arc::new(ForgeWakeScanner {
            daemon: self.wake_scan_handle(),
            forge,
            workflow,
            compiled,
            wake_targets,
            clock,
            mechanical,
        });
        *self.scanner_slot.lock().expect("scanner slot") = Some(scanner);
        self
    }

    fn wake_scan_handle(&self) -> Self {
        Self {
            cq: self.cq.clone(),
            scanner_slot: Arc::new(std::sync::Mutex::new(None)),
            context_reader_slot: Arc::clone(&self.context_reader_slot),
            change_source_listeners: Arc::new(std::sync::Mutex::new(Vec::new())),
            artifact_catalog: Arc::clone(&self.artifact_catalog),
        }
    }
}

struct ForgeWakeScanner<F: Forge + Send + Sync + ?Sized + 'static> {
    daemon: Daemon,
    forge: Arc<F>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: Arc<CompiledWorkflow>,
    wake_targets: Vec<RoleFeedTarget>,
    clock: WallClock,
    mechanical: Option<Arc<dyn HintedMechanical>>,
}

impl<F: Forge + Send + Sync + ?Sized + 'static> WakeScanner for ForgeWakeScanner<F> {
    fn scan(
        &self,
        hint: temper_runner::ChangeHint,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let daemon = self.daemon.clone();
        let forge = Arc::clone(&self.forge);
        let workflow = Arc::clone(&self.workflow);
        let compiled = Arc::clone(&self.compiled);
        let wake_targets = self.wake_targets.clone();
        let mechanical = self.mechanical.clone();
        let now = (self.clock)();
        Box::pin(async move {
            run_wake_scan(
                &daemon,
                forge.as_ref(),
                workflow.as_ref(),
                compiled.as_ref(),
                now,
                &wake_targets,
                &hint,
            )
            .await;
            // Accelerate the mechanical loop for the hinted repo: a push / CI /
            // review event should drive reconciliation now, not wait for the slow
            // backstop cadence. Coalesced inside the trigger, so a burst collapses
            // to one pass.
            if let Some(mechanical) = mechanical {
                mechanical.run_hinted(vec![hint]).await;
            }
        })
    }
}

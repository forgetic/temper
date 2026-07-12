// SPDX-License-Identifier: MPL-2.0

//! Type-erased bounded Forge reader installed into the daemon executor.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use temper_forge::Forge;
use temper_protocol_worker::{ForgeContextErrorCode, ForgeContextOperation, ForgeContextResult};
use temper_workflow::ValidatedWorkflow;

use crate::{ArtifactContextService, ConfiguredRepositoryCatalog};

pub(super) trait ContextReader: Send + Sync {
    fn read(
        &self,
        operation: ForgeContextOperation,
    ) -> Pin<Box<dyn Future<Output = Result<ForgeContextResult, ForgeContextErrorCode>> + Send + '_>>;
}

pub(super) struct BoundedContextReader<F: Forge + ?Sized> {
    forge: Arc<F>,
    catalog: Arc<ConfiguredRepositoryCatalog>,
    workflow: Arc<ValidatedWorkflow>,
}

impl<F: Forge + ?Sized> BoundedContextReader<F> {
    pub(super) fn new(
        forge: Arc<F>,
        catalog: Arc<ConfiguredRepositoryCatalog>,
        workflow: Arc<ValidatedWorkflow>,
    ) -> Self {
        Self {
            forge,
            catalog,
            workflow,
        }
    }
}

impl<F: Forge + Send + Sync + ?Sized + 'static> ContextReader for BoundedContextReader<F> {
    fn read(
        &self,
        operation: ForgeContextOperation,
    ) -> Pin<Box<dyn Future<Output = Result<ForgeContextResult, ForgeContextErrorCode>> + Send + '_>>
    {
        Box::pin(async move {
            ArtifactContextService::new(
                self.forge.as_ref(),
                self.catalog.as_ref(),
                self.workflow.as_ref(),
            )
            .execute(operation)
            .await
        })
    }
}

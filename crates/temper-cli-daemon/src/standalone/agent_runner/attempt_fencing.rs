// SPDX-License-Identifier: MPL-2.0

//! Exact-attempt host bindings for the standalone native runner.

use std::path::PathBuf;

use temper_agent::{ForgeContextHost, SubmitForPrHost};
use temper_protocol_worker::ForgeContextErrorCode;
use temper_worker::{
    AcceptedSubmitProofStore, AgentForgeContextHost, AttemptFence, JobCancellation,
};

pub(super) fn submit_host(
    accepted_submit: AcceptedSubmitProofStore,
    submit_for_pr: SubmitForPrHost,
    fence: AttemptFence,
    cancellation: JobCancellation,
) -> SubmitForPrHost {
    std::sync::Arc::new(move |request, context, cwd: PathBuf| {
        let accepted_submit = accepted_submit.clone();
        let submit_for_pr = submit_for_pr.clone();
        let fence = fence.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            temper_worker::handle_submit_for_pr_with_proof(
                &accepted_submit,
                &fence,
                &cancellation,
                move |request, context, cwd| submit_for_pr(request, context, cwd),
                request,
                context,
                cwd,
            )
            .await
        })
    })
}

pub(super) fn forge_host(
    host: AgentForgeContextHost,
    job_id: String,
    attempt_id: String,
    fence: AttemptFence,
) -> ForgeContextHost {
    std::sync::Arc::new(move |operation| {
        let host = host.clone();
        let job_id = job_id.clone();
        let attempt_id = attempt_id.clone();
        let fence = fence.clone();
        Box::pin(async move {
            if !fence.is_open() {
                return Err(ForgeContextErrorCode::ForgeUnavailable);
            }
            let result = host(job_id, attempt_id, fence.clone(), operation).await;
            if fence.is_open() {
                result
            } else {
                Err(ForgeContextErrorCode::ForgeUnavailable)
            }
        })
    })
}

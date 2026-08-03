//! Attempt-local accepted-submit proof and exact validation reuse.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use temper_protocol_agent::{SubmitForPrRequest, SubmitForPrResponse, WorkspaceContext};

use crate::executor::{AttemptFence, JobCancellation};
use crate::pre_push::fingerprint::fingerprint_writable_repos_controlled;
use crate::pre_push::{WorkspaceFingerprint, fingerprint_writable_repos_blocking};

use super::ATTEMPT_UNAVAILABLE_MESSAGE;

/// Host-owned evidence captured when a live `submit_for_pr` call was accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedSubmitProof {
    pub response: SubmitForPrResponse,
    pub fingerprint: WorkspaceFingerprint,
}

#[derive(Clone, Default)]
pub struct AcceptedSubmitProofStore {
    pub(super) inner: Arc<Mutex<Option<AcceptedSubmitProof>>>,
}

impl AcceptedSubmitProofStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn latest(&self) -> Option<AcceptedSubmitProof> {
        self.inner
            .lock()
            .expect("accepted submit proof lock")
            .clone()
    }

    pub fn clear(&self) {
        *self.inner.lock().expect("accepted submit proof lock") = None;
    }

    /// Reuses an accepted gate result only while its worker-owned workspace
    /// fingerprint remains exact. The store is attempt-local, so the bound
    /// submit handler and process/toolchain identity remain unchanged.
    pub async fn reuse_response_controlled(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        fence: &AttemptFence,
        cancellation: &JobCancellation,
    ) -> Option<SubmitForPrResponse> {
        let proof = self.latest()?;
        let fingerprint = run_fenced(
            self,
            fence,
            cancellation,
            fingerprint_writable_repos_controlled(context, cwd, cancellation),
        )
        .await?;
        let Ok(fingerprint) = fingerprint else {
            self.clear();
            return None;
        };
        if fingerprint != proof.fingerprint {
            self.clear();
            return None;
        }

        let saved_ms = proof
            .response
            .gates
            .iter()
            .map(|gate| gate.elapsed_ms)
            .sum::<u64>();
        let mut response = proof.response;
        response.message = format!(
            "reused worker-observed pre-push gates for exact unchanged workspace; saved {saved_ms}ms"
        );
        for gate in &mut response.gates {
            gate.exit_status = "reused".to_string();
            gate.stdout_tail.clear();
            gate.stderr_tail.clear();
            gate.elapsed_ms = 0;
        }
        Some(response)
    }

    pub fn record_response(
        &self,
        response: SubmitForPrResponse,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> SubmitForPrResponse {
        if !response.accepted {
            return response;
        }
        let fingerprint = match fingerprint_writable_repos_blocking(context, cwd) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                return SubmitForPrResponse::rejected(format!(
                    "submit_for_pr accepted but workspace proof could not be recorded: {error}"
                ));
            }
        };
        self.store(response, fingerprint)
    }

    pub async fn record_response_controlled(
        &self,
        response: SubmitForPrResponse,
        context: &WorkspaceContext,
        cwd: &Path,
        fence: &AttemptFence,
        cancellation: &JobCancellation,
    ) -> SubmitForPrResponse {
        if attempt_unavailable(fence, cancellation) {
            self.clear();
            return unavailable_submit_response();
        }
        if !response.accepted
            || response
                .message
                .starts_with("reused worker-observed pre-push gates")
        {
            return response;
        }
        let fingerprint = match run_fenced(
            self,
            fence,
            cancellation,
            fingerprint_writable_repos_controlled(context, cwd, cancellation),
        )
        .await
        {
            Some(Ok(fingerprint)) => fingerprint,
            Some(Err(error)) => {
                return SubmitForPrResponse::rejected(format!(
                    "submit_for_pr accepted but workspace proof could not be recorded: {error}"
                ));
            }
            None => return unavailable_submit_response(),
        };
        if attempt_unavailable(fence, cancellation) {
            self.clear();
            unavailable_submit_response()
        } else {
            self.store(response, fingerprint)
        }
    }

    fn store(
        &self,
        response: SubmitForPrResponse,
        fingerprint: WorkspaceFingerprint,
    ) -> SubmitForPrResponse {
        *self.inner.lock().expect("accepted submit proof lock") = Some(AcceptedSubmitProof {
            response: response.clone(),
            fingerprint,
        });
        response
    }
}

/// Runs one in-process submit gate under the exact attempt's publication and
/// cancellation controls.
pub async fn handle_submit_for_pr_with_proof<F, Fut>(
    store: &AcceptedSubmitProofStore,
    fence: &AttemptFence,
    cancellation: &JobCancellation,
    handler: F,
    request: SubmitForPrRequest,
    context: WorkspaceContext,
    cwd: PathBuf,
) -> SubmitForPrResponse
where
    F: FnOnce(SubmitForPrRequest, WorkspaceContext, PathBuf) -> Fut,
    Fut: Future<Output = SubmitForPrResponse>,
{
    if attempt_unavailable(fence, cancellation) {
        store.clear();
        return unavailable_submit_response();
    }
    let Some(response) = run_fenced(
        store,
        fence,
        cancellation,
        handler(request, context.clone(), cwd.clone()),
    )
    .await
    else {
        return unavailable_submit_response();
    };
    let response = store
        .record_response_controlled(response, &context, &cwd, fence, cancellation)
        .await;
    if attempt_unavailable(fence, cancellation) {
        store.clear();
        unavailable_submit_response()
    } else {
        response
    }
}

fn attempt_unavailable(fence: &AttemptFence, cancellation: &JobCancellation) -> bool {
    !fence.is_open() || cancellation.is_cancelled()
}

fn unavailable_submit_response() -> SubmitForPrResponse {
    SubmitForPrResponse::rejected(ATTEMPT_UNAVAILABLE_MESSAGE)
}

async fn run_fenced<F: Future>(
    store: &AcceptedSubmitProofStore,
    fence: &AttemptFence,
    cancellation: &JobCancellation,
    future: F,
) -> Option<F::Output> {
    let mut future = Box::pin(future);
    let mut cancelled = Box::pin(cancellation.cancelled());
    let output = std::future::poll_fn(|cx| {
        if attempt_unavailable(fence, cancellation) || cancelled.as_mut().poll(cx).is_ready() {
            store.clear();
            return std::task::Poll::Ready(None);
        }
        match future.as_mut().poll(cx) {
            std::task::Poll::Ready(output) if !attempt_unavailable(fence, cancellation) => {
                std::task::Poll::Ready(Some(output))
            }
            std::task::Poll::Ready(_) => {
                store.clear();
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    })
    .await;
    drop(future);
    if output.is_none() {
        store.clear();
    }
    output
}

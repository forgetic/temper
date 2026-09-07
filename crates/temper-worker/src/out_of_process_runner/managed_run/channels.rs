//! Bounded host-side channels used by the managed agent run.
use super::*;

pub(super) fn bounded_forge_future(
    future: AgentForgeContextFuture,
    timeout: Duration,
) -> AgentForgeContextFuture {
    Box::pin(async move {
        match skein::time::timeout(temper_worker_io::engine_now(), timeout, future).await {
            Ok(result) => result,
            Err(_) => Err(temper_protocol_agent::ForgeContextErrorCode::ForgeUnavailable),
        }
    })
}

pub(super) fn bounded_submit_future(
    future: SubmitForPrFuture,
    timeout: Duration,
) -> SubmitForPrFuture {
    Box::pin(async move {
        match skein::time::timeout(temper_worker_io::engine_now(), timeout, future).await {
            Ok(response) => response,
            Err(_) => SubmitForPrResponse::rejected(format!(
                "submit_for_pr exceeded the generic tool deadline of {:.3}s",
                timeout.as_secs_f64()
            )),
        }
    })
}

pub(super) fn optional_listener(
    enabled: bool,
    label: &str,
) -> Result<Option<(TcpListener, String)>, AgentRunError> {
    if !enabled {
        return Ok(None);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| AgentRunError::transient(format!("bind {label}: {error}")))?;
    let address = listener
        .local_addr()
        .map_err(|error| AgentRunError::transient(format!("read {label} address: {error}")))?;
    Ok(Some((listener, address.to_string())))
}

pub(super) fn forge_unavailable() -> ForgeContextResponse {
    ForgeContextResponse::error(temper_protocol_agent::ForgeContextErrorCode::ForgeUnavailable)
}

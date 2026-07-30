use std::time::Duration;

use temper_agent_core::{AgentOperationLimits, ModelRetryLimits};
use temper_protocol_agent::AgentRuntimeLimitsV1;

pub(super) fn operation_limits(value: AgentRuntimeLimitsV1) -> AgentOperationLimits {
    AgentOperationLimits {
        tool_timeout: Duration::from_secs(value.tool_timeout_secs),
        model_connect_timeout: Duration::from_secs(value.model_connect_timeout_secs),
        model_idle_timeout: Duration::from_secs(value.model_idle_timeout_secs),
        model_retry: ModelRetryLimits {
            max_attempts: value.model_retry_max_attempts,
            base_delay: Duration::from_millis(value.model_retry_base_delay_ms),
            max_delay: Duration::from_millis(value.model_retry_max_delay_ms),
            jitter_percent: value.model_retry_jitter_percent,
        },
    }
}

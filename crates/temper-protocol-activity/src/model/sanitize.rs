use super::{AgentActivityEventV1, ToolStatusV1};

impl AgentActivityEventV1 {
    /// Drops malformed, unsupported, or status-inconsistent graph evidence
    /// before an untrusted activity frame can reach durable storage. Valid
    /// records contain no raw argument or result content.
    pub fn sanitize_graph_correlation(&mut self) {
        let Self::ToolFinished(finished) = self else {
            return;
        };
        if finished
            .graph_correlation
            .as_ref()
            .is_some_and(|correlation| {
                finished.status != ToolStatusV1::Succeeded
                    || !correlation.is_valid()
                    || correlation.tool.public_name() != finished.name
            })
        {
            finished.graph_correlation = None;
        }
        if finished
            .decision_anchor_lineage
            .as_ref()
            .is_some_and(|lineage| {
                finished.status != ToolStatusV1::Succeeded
                    || finished
                        .graph_correlation
                        .as_ref()
                        .is_none_or(|correlation| !lineage.is_valid_for(correlation))
            })
        {
            finished.decision_anchor_lineage = None;
        }
    }

    /// Applies every content-free sanitizer required before an untrusted activity
    /// event can enter transport, journal, or source-digest processing.
    pub fn sanitize_untrusted_activity(&mut self) {
        self.sanitize_retry_failure_message();
        self.normalize_model_failure();
        self.sanitize_graph_correlation();
    }
}

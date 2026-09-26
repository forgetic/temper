//! Jig model startup and observations for live graph fixtures.
use super::*;
use jig_core::{Reply, Script, ScriptFile};
use jig_server::FakeLlm;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub(in crate::live_manifest) struct CodebaseMemoryFake {
    // Drop gates and join forwarding before stopping the sequential Jig server.
    shared_router: Option<shared_lifecycle::JigRouter>,
    fake: FakeLlm,
    engineer_requests: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
    require_current_root_source: bool,
    privacy_safe_log: bool,
    shared_lifecycle: Option<Arc<shared_lifecycle::Control>>,
}

impl CodebaseMemoryFake {
    pub(in crate::live_manifest) fn start(
        script_path: &Path,
        require_current_root_source: bool,
        lifecycle_profile: Option<&str>,
    ) -> Result<Self, String> {
        let script = ScriptFile::load(script_path)
            .map_err(|error| {
                format!(
                    "load scenario Jig script {}: {error}",
                    script_path.display()
                )
            })?
            .into_script();
        let engineer_requests = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::clone(&engineer_requests);
        let observations = Arc::new(Mutex::new(ModelObservations::default()));
        let observations_for_rule = Arc::clone(&observations);
        let shared_lifecycle = (lifecycle_profile == Some("shared-codebase-memory-lifecycle"))
            .then(|| Arc::new(shared_lifecycle::Control::default()));
        let mut shared_router = None;
        let fake = if let Some(control) = &shared_lifecycle {
            let (fake, router) =
                shared_lifecycle::start(Arc::clone(&request_count), Arc::clone(control))?;
            shared_router = Some(router);
            fake
        } else if script_path
            .file_name()
            .is_some_and(|name| name == "scoped-graph-evidence.json")
        {
            scoped_graph_evidence::start(request_count, observations_for_rule)?
        } else if matches!(
            lifecycle_profile,
            Some("result-driven-decision-guidance" | "provider-result-anchor")
        ) {
            result_driven_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("provider-neutral-anchor-lineage") {
            typed_lineage_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-graph-consumption")
            && script_path
                .file_name()
                .is_some_and(|name| name == "mapped-live-companion-read.json")
        {
            mapped_companion_read_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-graph-consumption")
            && script_path
                .file_name()
                .is_some_and(|name| name == "mapped-live-patch-creation.json")
        {
            mapped_patch_creation_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-graph-consumption") {
            mapped_graph_consumption_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-denied-shell-classification") {
            mapped_denied_shell_classification_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-ordinary-tool-convergence") {
            mapped_ordinary_convergence_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-graph-convergence") {
            mapped_graph_convergence_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-decision-gap-recovery") {
            mapped_decision_gap_recovery_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-exact-source-selection") {
            mapped_exact_source_selection_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-focused-test-source-relevance") {
            mapped_focused_test_relevance_fake::start(request_count, observations_for_rule)?
        } else {
            FakeLlm::start(Script::rule(move |view| {
                if !messages_contain(view, "ROLE: engineer") {
                    return Reply::text("unexpected codebase-memory fake-LLM request");
                }
                request_count.fetch_add(1, Ordering::SeqCst);
                let mut observations = observations_for_rule.lock().expect("observations lock");
                if messages_contain(view, "CODEBASE MEMORY") {
                    observations.prompt_guidance_seen = true;
                }
                if messages_contain(view, MEMORY_RESULT_NEEDLE)
                    || messages_contain(view, "SEQUENTIAL_GRAPH_RESULT")
                {
                    observations.memory_result_seen = true;
                }
                if messages_contain(view, "FAKE_MCP_CODE_RESULT")
                    || messages_contain(view, "SEQUENTIAL_CODE_RESULT")
                {
                    observations.code_refinement_seen = true;
                }
                if messages_contain(view, "FAKE_MCP_TRACE_RESULT")
                    || messages_contain(view, "SEQUENTIAL_TRACE_RESULT")
                {
                    observations.graph_trace_seen = true;
                }
                let current_root_source_results = view
                    .messages
                    .iter()
                    .filter(|message| is_current_root_source_result(&message.content))
                    .count();
                observations.current_root_source_seen |= current_root_source_results > 0;
                observations.current_root_source_results += current_root_source_results;
                if messages_contain(view, SAFE_PROVIDER_FAILURE) {
                    observations.safe_failure_seen = true;
                }
                if messages_contain(view, RAW_PROVIDER_FAILURE_NEEDLE) {
                    observations.raw_provider_text_seen = true;
                }
                if messages_contain(view, BOUNDED_GRAPH_RESULT_NEEDLE) {
                    observations.bounded_graph_result_seen = true;
                }
                if view.messages.iter().any(|message| {
                    message.role == "tool" && message.content.len() > MAX_MODEL_MESSAGE_BYTES
                }) {
                    observations.oversized_message_seen = true;
                }
                drop(observations);
                script.next_reply(view)
            }))
            .map_err(|error| format!("start scenario Jig fake LLM: {error}"))?
        };
        Ok(Self {
            shared_router,
            fake,
            engineer_requests,
            observations,
            require_current_root_source,
            privacy_safe_log: privacy::is_privacy_safe_profile(lifecycle_profile),
            shared_lifecycle,
        })
    }

    pub(in crate::live_manifest) fn shared_lifecycle(
        &self,
    ) -> Option<&Arc<shared_lifecycle::Control>> {
        self.shared_lifecycle.as_ref()
    }

    pub(in crate::live_manifest) fn base_url(&self) -> String {
        self.shared_router
            .as_ref()
            .map_or_else(|| self.fake.base_url(), |router| router.base_url())
    }

    pub(in crate::live_manifest) fn engineer_requests(&self) -> usize {
        self.engineer_requests.load(Ordering::SeqCst)
    }

    pub(super) fn validate_observations(&self, mcp: &FakeMcpServer) -> Result<(), String> {
        let (
            prompt_guidance_seen,
            memory_result_seen,
            current_root_source_seen,
            safe_failure_seen,
            raw_provider_text_seen,
            bounded_graph_result_seen,
            code_refinement_seen,
            graph_trace_seen,
            current_root_source_results,
            oversized_message_seen,
        ) = {
            let observations = self
                .observations
                .lock()
                .map_err(|_| "model observation mutex poisoned".to_string())?;
            (
                observations.prompt_guidance_seen,
                observations.memory_result_seen,
                observations.current_root_source_seen,
                observations.safe_failure_seen,
                observations.raw_provider_text_seen,
                observations.bounded_graph_result_seen,
                observations.code_refinement_seen,
                observations.graph_trace_seen,
                observations.current_root_source_results,
                observations.oversized_message_seen,
            )
        };
        if !prompt_guidance_seen {
            return Err(format!(
                "fake LLM did not receive CODEBASE MEMORY prompt guidance\n{}",
                self.log_tail()
            ));
        }
        if !memory_result_seen {
            return Err(format!(
                "fake LLM did not receive the fake MCP graph result\n{}",
                self.log_tail()
            ));
        }
        if self.require_current_root_source && !current_root_source_seen {
            return Err(format!(
                "fake LLM did not receive source served after current-checkout rebinding\n{}",
                self.log_tail()
            ));
        }
        if !bounded_graph_result_seen
            && !matches!(
                mcp.lifecycle_profile.as_deref(),
                Some(
                    "graph-consumption"
                        | "sequential-graph-evidence"
                        | "result-driven-decision-guidance"
                        | "provider-result-anchor"
                        | "provider-neutral-anchor-lineage"
                        | "mapped-live-graph-consumption"
                        | "mapped-live-denied-shell-classification"
                        | "mapped-live-ordinary-tool-convergence"
                        | "mapped-live-graph-convergence"
                        | "mapped-live-decision-gap-recovery"
                        | "mapped-live-exact-source-selection"
                        | "mapped-live-focused-test-source-relevance"
                )
            )
        {
            return Err(format!(
                "fake LLM did not receive the bounded graph result marker\n{}",
                self.log_tail()
            ));
        }
        if mcp.forced_systemic_failure.is_some() && !safe_failure_seen {
            return Err(format!(
                "fake LLM did not receive the bounded typed systemic diagnostic\n{}",
                self.log_tail()
            ));
        }
        if matches!(
            mcp.lifecycle_profile.as_deref(),
            Some(
                "graph-consumption"
                    | "sequential-graph-evidence"
                    | "result-driven-decision-guidance"
                    | "provider-result-anchor"
                    | "provider-neutral-anchor-lineage"
                    | "mapped-live-graph-consumption"
                    | "mapped-live-denied-shell-classification"
                    | "mapped-live-ordinary-tool-convergence"
                    | "mapped-live-graph-convergence"
                    | "mapped-live-decision-gap-recovery"
                    | "mapped-live-exact-source-selection"
                    | "mapped-live-focused-test-source-relevance"
            )
        ) && !(graph_trace_seen
            && current_root_source_results >= 2
            && (matches!(
                mcp.lifecycle_profile.as_deref(),
                Some(
                    "provider-neutral-anchor-lineage"
                        | "mapped-live-graph-consumption"
                        | "mapped-live-denied-shell-classification"
                        | "mapped-live-ordinary-tool-convergence"
                        | "mapped-live-graph-convergence"
                        | "mapped-live-decision-gap-recovery"
                        | "mapped-live-exact-source-selection"
                        | "mapped-live-focused-test-source-relevance"
                )
            ) || code_refinement_seen))
        {
            return Err(format!(
                "fake LLM did not consume the complete graph-to-graph/current-root source chain\n{}",
                self.log_tail()
            ));
        }
        let minimum_requests =
            if mcp.lifecycle_profile.as_deref() == Some("mapped-live-ordinary-tool-convergence") {
                15
            } else if matches!(
                mcp.lifecycle_profile.as_deref(),
                Some(
                    "provider-neutral-anchor-lineage"
                        | "mapped-live-graph-consumption"
                        | "mapped-live-denied-shell-classification"
                        | "mapped-live-graph-convergence"
                        | "mapped-live-decision-gap-recovery"
                        | "mapped-live-exact-source-selection"
                        | "mapped-live-focused-test-source-relevance"
                )
            ) {
                8
            } else {
                9
            };
        if self.engineer_requests() < minimum_requests {
            return Err(format!(
                "fake LLM did not complete the codebase-memory validation loop\n{}",
                self.log_tail()
            ));
        }
        if raw_provider_text_seen {
            return Err(format!(
                "raw provider failure text leaked into the fake LLM request\n{}",
                self.log_tail()
            ));
        }
        if oversized_message_seen {
            return Err(format!(
                "a model-visible message exceeded the scenario's bounded result allowance\n{}",
                self.log_tail()
            ));
        }
        Ok(())
    }

    pub(in crate::live_manifest) fn log_tail(&self) -> String {
        if self.shared_lifecycle.is_some() {
            return format!(
                "shared lifecycle Jig: {} engineer requests; correlated acceptance is retained separately",
                self.engineer_requests()
            );
        }
        let requests = self.fake.requests();
        let observations = self.observations.lock().expect("observations lock");
        let mut lines = vec![format!(
            "observations: prompt_guidance_seen={} memory_result_seen={} current_root_source_seen={} code_refinement_seen={} graph_trace_seen={} current_root_source_results={} bounded_graph_result_seen={} safe_failure_seen={} raw_provider_text_seen={} oversized_message_seen={}",
            observations.prompt_guidance_seen,
            observations.memory_result_seen,
            observations.current_root_source_seen,
            observations.code_refinement_seen,
            observations.graph_trace_seen,
            observations.current_root_source_results,
            observations.bounded_graph_result_seen,
            observations.safe_failure_seen,
            observations.raw_provider_text_seen,
            observations.oversized_message_seen,
        )];
        if requests.is_empty() {
            lines.push("<fake LLM received no requests>".to_string());
            return lines.join("\n");
        }
        let start = requests.len().saturating_sub(20);
        lines.extend(
            requests[start..]
                .iter()
                .enumerate()
                .map(|(offset, request)| {
                    let index = start + offset + 1;
                    let view = request.view.as_ref();
                    let prior = view.map(|v| v.prior_tool_results).unwrap_or_default();
                    if self.privacy_safe_log {
                        return format!(
                            "#{index} {} {} role=engineer prior_tool_results={prior}",
                            request.method, request.path
                        );
                    }
                    let last = view
                        .and_then(RequestView::last_message)
                        .map(|m| format!("{}: {}", m.role, snippet(&m.content, 160)))
                        .unwrap_or_else(|| "<no projected message>".to_string());
                    format!(
                        "#{index} {} {} role=engineer prior_tool_results={prior} last={last}",
                        request.method, request.path
                    )
                }),
        );
        lines.join("\n")
    }
}

# Agent trace end-to-end validation matrix

Agent trace acceptance is intentionally hermetic and split at durable failure
boundaries. Each scenario uses temporary directories, loopback transports,
in-memory exporters/forges, injected clocks, or fake engine/model clients; none
requires a live collector, Forgejo, or model provider.

| Scenario | Deterministic coverage |
| --- | --- |
| Producer → child socket → trusted worker spool → lost-ack restart → standalone/authenticated forwarding → engine journal/query → web/OTel projection | `temper-worker::trace::full_path_tests::first_party_events_cross_the_complete_standalone_and_distributed_paths` (the canonical batch is recovered from the producer-fed spool, never constructed at the engine boundary) |
| Standalone/distributed carrier parity, auth, duplicate batch, lost reply | The full-path capstone above, plus focused carrier checks in `temper-engine/tests/activity_transport.rs::http_and_in_process_carriers_share_durable_ack_and_deduplication` and `distributed_delivery_requires_configured_auth_but_trusted_carrier_does_not` |
| Canonical run/scope/turn/model/tool OTel tree, timestamp/duration/status/provider/usage/gap attributes | `temper-log::activity::tests::canonical_boundaries_form_a_nested_privacy_safe_span_tree` |
| Parallel sub-agent scope uniqueness and parentage | `temper-worker::trace::tests::parallel_frames_get_one_gap_free_sequence_and_trusted_identity`, `child_root_is_mapped_to_one_unique_canonical_scope_with_correct_parentage`, and `temper-log::activity::tests::parallel_sub_agent_scopes_keep_unique_ids_and_parentage` |
| Child crash with host terminal failure; storage failure does not alter job result | `temper-worker::out_of_process_runner_trace_tests::child_crash_leaves_host_failed_metadata_and_trace_storage_errors_are_non_fatal` |
| Raw child stderr and in-process provider/tool errors stay outside terminal activity in every capture mode | `temper-worker::trace::full_path_tests::raw_child_failures_never_cross_the_canonical_trace_plane` injects credential/header/environment/stderr sentinels and checks worker spools, engine journals, authorized query/export, web, and OTel surfaces; `temper-cli-daemon::standalone::agent_runner::tests::in_process_terminal_failures_never_capture_tool_diagnostics` covers the standalone carrier and capture-off behavior |
| Provider/model retry diagnostics never enter canonical activity in any capture mode | `temper-agent::activity::tests::provider_retry_diagnostics_are_fixed_in_every_capture_mode` covers producer normalization; `temper-worker::trace::tests::forged_child_retry_diagnostics_are_sanitized_before_spooling` covers the untrusted child boundary; `temper-worker::trace::full_path_retry_tests::retry_diagnostics_are_sanitized_across_every_canonical_surface` injects credential/header/environment/provider-response sentinels into a forged direct batch and checks worker spools, engine journals, authorized query/export, web, OTel, and capture-off behavior |
| Valid-looking forged model failures cannot use syntactically valid text or identifiers as a durable channel | `temper-protocol-activity::tests::model_failure::untrusted_valid_looking_diagnostics_do_not_retain_strings` checks the trust-boundary contract; `temper-worker::trace::full_path_model_failure_tests::valid_looking_forged_model_failures_are_redacted_across_every_surface` isolates prompt/raw-body/environment/credential/provider-response sentinels in message, provider-code, request-ID, provider, and model fields, then checks every capture mode, worker spool, engine journal and retransmit identity, query/export, web, human/structured logs, and OTel |
| Worker restart, partial spool, blob/cursor recovery | `temper-worker::trace::tests::restart_recovers_blobs_cursor_and_truncates_only_final_fragment` |
| Lost acknowledgement and forwarding restart | `temper-worker::trace::forwarder::tests::lost_reply_retransmits_and_restart_observes_the_durable_cursor` |
| Engine restart, partial journal, duplicate/conflicting retransmit | `temper-engine/tests/trace_journal.rs::recovery_truncates_only_a_partial_tail_and_acknowledges_a_lost_reply` and `duplicate_gap_binding_and_conflicting_retransmit_are_isolated` |
| Queue saturation drops only deltas and emits a gap | `temper-agent::activity::transport_tests::saturation_discards_only_delta_and_emits_an_ordered_gap` |
| Metadata-default privacy and bounded quota | `temper-agent::activity::tests::metadata_keeps_tool_start_identity_without_any_argument_bytes`, `metadata_excludes_content_and_all_modes_redact_and_bound`, `temper-worker::trace::tests::metadata_policy_rejects_forged_message_and_tool_argument_content`, and `temper-log::activity::tests::canonical_boundaries_form_a_nested_privacy_safe_span_tree` |
| Retention and private storage permissions | `temper-engine/tests/trace_journal.rs::retention_uses_the_injected_clock_and_preserves_incomplete_or_recovered_runs`, `unix_layout_is_owner_only`, and `temper-worker::trace::tests::spool_directories_and_files_are_owner_only` |
| Query authorization, stable pagination after restart, JSONL ordering | `temper-engine/tests/trace_query_api.rs::trace_authorization_distinguishes_missing_and_wrong_without_leaking_secrets` and `run_pages_use_stable_equal_timestamp_order_and_composable_filters_after_restart` |
| Web drawer outage/reconnect/dedup/close | `temper-web::server::trace_proxy_tests::detailed_sse_recovers_from_outage_resumes_without_duplicates_and_stops_on_close` and `ui/test/trace-drawer.dom.test.ts` |
| Projection/exporter failure isolation | `temper-log::activity::tests::exporter_failures_never_escape_projection` and the worker/engine trace degradation tests above |
| W3C assignment propagation | `temper-protocol-context::tests::w3c_trace_context_validation_is_strict_and_bounded`, `temper-protocol-worker::tests::job_context::assignment_trace_context_round_trips_and_stays_optional`, and `temper-worker/tests/coding_executor/context.rs::w3c_trace_context_propagates_from_assignment_to_agent_workspace` |

Run the complete repository pre-PR lane:

```sh
./.temper/pre-pr
```

The full-path capstone is part of `temper-worker`'s ordinary test target, so the
repository lane cannot accidentally validate only hand-built engine batches.
The lane covers Rust protocol/storage/transport tests, dependency-graph rules,
and web TypeScript tests. Also compile the disabled-by-default exporter path:

```sh
cargo test -p temper-log --features otel-otlp
cargo check -p temper --features otel
```

The validation invariant is that a scenario may lose telemetry, but it may not
change the `JobResult`, create a product-work retry, or make a web/OTel projection
more authoritative than the canonical journal. Host-generated `run.failed`
events contain only the trusted failure code and retryability plus a fixed
summary selected from the trusted failure classification; raw agent errors and
bounded child stderr remain available only to job reporting and worker
diagnostics.

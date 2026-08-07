# Target-era operator contract matrix

This is the convergence audit for the target-era operator contract. It maps the
public lifecycle (`init -> check -> plan -> apply -> serve`) and its deliberately
retained migration surfaces to code, operator documentation, and the narrowest
regression authority. **Converged** means the implementation, docs, and durable
test agree; it does not promote a compatibility/internal surface.

| Normative group | Implementation symbols / artifacts | Operator documentation | Focused regression authority | Compatibility classification | Final status |
| --- | --- | --- | --- | --- | --- |
| CLI | `temper_cli::USAGE`, `temper_cli::dispatch`, `temper_cli::maintenance`, `temper_cli_daemon::parse_serve_invocation` | root `README.md`; [codebase-memory recovery](../how-to/recover-codebase-memory.md); [operator compatibility surfaces](operator-compatibility-surfaces.md) | `operator_help_contract::{top_level_help_exposes_only_the_public_operator_contract,hidden_compatibility_and_internal_commands_remain_dispatchable}`; `temper_cli::maintenance::tests`; `target_ux::compatibility` | `init`, `check`, `plan`, `apply`, `serve`, and `maintenance` are public; `daemon`, `agent`, and `trigger-forgejo` are hidden compatibility/internal dispatch | **Converged** |
| Loading and secrets | `temper_config::load_explicit`, `temper_cli_init::load_deployment`, `temper_cli_daemon::load_for`, `temper_config::resolve_engine_secret_references` | [environment variables](environment-variables.md); [systemd deployment](../how-to/deploy-with-systemd.md) | `target_ux::deployment::config_relative_paths_and_explicit_systemd_secret_precedence_are_durable`; `temper_config::inputs::tests::{explicit_config_directory_resolves_relative_paths_under_bundle_root,explicit_config_file_resolves_relative_paths_under_file_parent}` | Explicit `--secrets` is public precedence; systemd `CREDENTIALS_DIRECTORY` and sibling credentials are supported loading sources; legacy path fields are fallback-only | **Converged; output redacted** |
| Workflows | `temper_workflow::load_workflow`, `ValidatedWorkflow::compile`, `temper_cli_init::load_deployment` | [workflow specification](workflow-specification.md); [workflow layer](workflow-layer.md) | `target_ux::deployment::checked_in_json_and_yaml_bundles_pass_every_static_loading_seam`; `temper_cli_init::deployment::tests::json_and_yaml_load_to_the_same_all_repository_model` | JSON and YAML are equal public input formats; legacy `[engine] workflow` is retained fallback | **Converged** |
| Init | `temper_cli_init::run_init`, `temper_cli_init::build_artifacts`, `temper_cli_init::write_artifacts`, `temper_cli_init::apply_target_shape` | [systemd deployment](../how-to/deploy-with-systemd.md), section 1 | `target_ux::onboarding::generated_standalone_bundle_converges_from_init_through_apply_and_runtime_adaptation`; `run_init::apply::{interactive_decline_retains_generated_bundle_without_forge_call,apply_yes_provisions_two_repositories_in_one_deployment_call}` | Top-level `init` is public; `config init` and `init --apply --yes` are compatibility/demo conveniences | **Converged** |
| Plan and apply | `temper_cli_init::run_plan`, `temper_cli_init::run_apply`, `ApplyProvisioner::provision_apply_plan`, `DeploymentBundle::expose_provision_plans` | [`temper plan` report](plan-report.md); [cross-repository workflows](cross-repo-workflows.md); [systemd deployment](../how-to/deploy-with-systemd.md) | `plan_cli::{multi_repository_basic_auth_plan_is_observably_read_only,human_plan_lists_every_repository_and_scopes_failures}`; `target_ux::deployment::multi_repository_plan_decline_and_yes_share_one_deployment_model`; `run_init::apply::apply_yes_provisions_two_repositories_in_one_deployment_call` | Public deployment-wide contract; singular JSON projection and `--existing-repo` are bounded compatibility | **Converged; plan read-only, apply confirmed** |
| Workers and agents | `temper_cli_daemon::apply_runtime_overrides`, `temper_engine_service::daemon_run_config`, `temper_worker_service::worker_config`, `temper_worker_service::agent_invocation` | [production worker](production-worker.md); [operator compatibility surfaces](operator-compatibility-surfaces.md) | `selected_pool_agent_profile_controls_command_and_env`; `no_pool_registration_preserves_legacy_capabilities_even_with_policies`; `register_without_pool_preserves_legacy_capabilities_with_pool_policies`; `pool_without_agent_profile_uses_legacy_provider_fallback`; `legacy_only_config_has_no_target_metadata_and_preserves_runtime_fields` | Named pools/profiles are public target-era config; no-pool and top-level provider behavior are retained migration fallbacks | **Converged; focused suites remain authoritative** |
| Webhook | `temper_engine::serve`, `temper_engine::handle_webhook`, `temper_engine::webhook_signature`, `temper_trigger_forgejo::run` | [systemd deployment](../how-to/deploy-with-systemd.md), section 5; [operator compatibility surfaces](operator-compatibility-surfaces.md) | `target_ux::webhook::signed_webhook_proves_engine_intake_and_selected_operator_contract`; `posted_webhook_wakes_target_then_worker_is_assigned`; `posted_webhook_drives_success_apply_to_pull_request`; `posted_webhook_with_invalid_signature_is_unauthorized_and_enqueues_nothing`; `selected_forgejo_contract_accepts_forgejo_headers_and_sha256_prefix` | Engine/standalone `POST /forgejo/webhook` is public; polling is the correctness backstop; `trigger-forgejo` is a dispatchable legacy adapter; `serve trigger` is rejected | **Converged** |
| Docs and systemd | `examples/systemd/config.example.toml`, `credentials.example.toml`, `temper-engine.service`, `temper-worker@.service` | [deploy Temper with systemd](../how-to/deploy-with-systemd.md); `examples/systemd/README.md` | `systemd_examples::{offline_engine_and_every_pool_check_pass_and_redact_secrets,units_use_public_serve_commands_and_the_bundled_credentials}` | Units use only public `serve` commands; `daemon` and a separate trigger unit are intentionally absent | **Converged** |
| Target-UX scenario | `scenarios/target-ux-e2e/scenario.toml`; `tests/target_ux_e2e.rs`; `tests/target_ux/{support,onboarding,deployment,runtime,webhook,compatibility}.rs` | [target-UX scenario README](../../scenarios/target-ux-e2e/README.md) | `cargo test --test target_ux_e2e`; `cargo dev-scenario-check` | Broad convergence evidence only; registry, transport, agent invocation, and webhook internals stay in focused suites | **Converged** |

## Convergence record

### Confirmed gaps

- The target-UX regression was one oversized file and did not carry one durable
  deployment-wide proof across plan, apply confirmation, redaction, systemd
  precedence, and serve-startup loading.
- The scenario manifest and README stopped at init/check/apply and trigger
  selection; they did not inventory plan, multi-repository behavior, operator
  docs, or systemd examples.
- There was no single normative audit tying the target-era surface to symbols,
  docs, focused tests, compatibility classification, and final status.

### Resolutions

- `tests/target_ux_e2e.rs` is now a thin facade over responsibility-based modules
  that exercise checked-in JSON/YAML bundles and generated temporary bundles.
- The regression now proves config-relative paths, explicit/systemd secret
  precedence, human/JSON redaction, all-repository plan/apply, decline no-op,
  `--yes`, and bounded serve-startup pool/profile adaptation.
- The scenario manifest and README now inventory plan/apply, confirmation,
  operator docs, systemd examples, and this matrix.
- This matrix is linked from the reference index and records the focused suites
  that remain authoritative.

### Deliberately retained compatibility surfaces

- `temper daemon`, `temper agent`, and `temper trigger-forgejo` remain
  dispatchable but unpromoted; `temper serve trigger` remains rejected.
- `temper config init`, `temper config validate`, `--existing-repo`, and the
  one-repository plan JSON projection remain bounded migration contracts.
- No-pool worker registration, legacy capability advertisement, top-level agent
  provider fallback, and legacy config fields remain covered by the cited
  focused tests rather than copied into this broad family.

### Unchanged areas

- Registry matching, pool authorization, transport protocol, and worker/agent
  invocation internals are unchanged.
- Forge provisioning adapters and token minting behavior are unchanged; the
  target-UX apply tests use the existing `ApplyProvisioner` seam.
- Engine webhook route parsing/authentication and legacy adapter internals are
  unchanged; the checked-in signed payload test composes their existing APIs.
- Live Forgejo scenario execution remains inherited from `basic-delivery`; this
  family adds compact static/operator convergence evidence, not another live
  end-to-end stack.

## Required validation and handoff

A change to this matrix or target-UX family is complete only when both commands
run from the repository root and their exact outcomes are included in the PR
implementation report:

```text
./.temper/pre-pr
cargo dev-scenario-check
```

The report must repeat the current **Confirmed gaps**, **Resolutions**,
**Deliberately retained compatibility surfaces**, and **Unchanged areas** lists
so review can compare the implementation handoff with this durable audit.

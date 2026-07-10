# Operator compatibility surfaces

The public operator lifecycle is `temper init -> temper check -> temper plan ->
temper apply -> temper serve`. The surfaces below remain available so existing
deployments can migrate, but they are intentionally absent from top-level
onboarding help unless noted otherwise.

## Command compatibility

| Surface | Status and migration contract |
| --- | --- |
| `temper daemon` | **Hidden, supported compatibility.** Bare `daemon` maps to `serve standalone`; `daemon --service engine|worker` maps to the corresponding public `serve` component. Existing automation may continue to use it, but new units and docs must use `temper serve`. |
| `temper config init` | **Compatibility utility under public `config`.** It writes the older bare config/credentials templates. New deployments use top-level `temper init`, which writes the complete config, workflow, credential, and webhook bundle. |
| `temper config validate` | **Compatibility utility under public `config`.** It performs legacy whole-config validation. New automation uses top-level, component-aware `temper check`. `config init` and `config validate` remain separate commands; neither implies the other. |
| `--existing-repo` on `init`, `plan`, and `apply` | **Supported compatibility behavior.** It applies to every configured repository, requires each repository to exist, and suppresses repository content seeding. It is not the default onboarding path. |
| `temper agent` | **Hidden internal process boundary.** Workers spawn it with a job context, result path, profile flags, and a provider credential envelope. It is dispatchable for worker integration and diagnostics, not a long-lived operator service. |
| `temper trigger-forgejo` | **Legacy/internal adapter.** It verifies old webhook deliveries and sends authenticated host-local wake hints to wake sockets. Public webhook intake is `POST /forgejo/webhook` on `temper serve engine` or `temper serve standalone`; no trigger unit should be installed. |

Wake directories, named wake sockets, and the `temper-wake` protocol remain
supported for fixtures and older topologies that still use the legacy trigger
adapter. They are internal migration surfaces, not an alternative public
trigger plane. Polling remains the correctness backstop.

## Configuration and worker fallbacks

The resolver/runtime keeps these bounded migration fallbacks:

- A worker registration with no pool name keeps its explicitly advertised
  legacy capabilities, even when the engine has pool policies. Pool policy is
  enforced once a worker names a pool.
- A selected pool with no `agent_profile` uses the legacy top-level `[agent]`
  provider, model, limits, and `[agent.providers.<name>]` credential. A named
  profile, when present, takes precedence for that pool.
- Legacy config fields remain accepted where the target fields are absent:
  `[engine] workflow` falls back behind `[workflow] file`, `[worker] workspace`
  behind `[paths] workspace_dir`, `[forge] admin` credentials behind
  `[engine] forge_token`, and `[engine] webhook_secret_file` behind the named
  `[engine] webhook_secret`. Supplying conflicting target and legacy path fields
  is an error rather than a precedence guess.

These fallbacks already have focused contract tests and should not be copied
into broad example tests:

- `temper-worker-registry`:
  `no_pool_registration_preserves_legacy_capabilities_even_with_policies` and
  `register_without_pool_preserves_legacy_capabilities_with_pool_policies`.
- `temper-cli-daemon`:
  `worker_without_pool_preserves_legacy_capabilities`.
- `temper-worker-service`:
  `pool_without_agent_profile_uses_legacy_provider_fallback`.
- `temper-config`:
  `legacy_workflow_and_workspace_remain_supported` and the matching/conflicting
  target/legacy path tests.

Compatibility means the behavior is tested and may be relied on during
migration. It does not make the surface part of new-deployment onboarding.

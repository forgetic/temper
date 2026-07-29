# Scenario corpus

`scenarios/` is Temper's checked-in corpus of declarative validation cases. A
scenario is a small, reviewable bundle that describes a workflow, seed forge
state, agent shape, expected outcome, and local fixture files. The corpus is for
stable regression inputs that future validation tooling can load without having
to reverse-engineer shell demos or test code.

This directory is intentionally separate from `examples/`. Examples remain
operator-facing demos with launch scripts and runtime instructions. Scenarios are
portable inputs for validation and post-merge regression; adding a scenario does
not migrate, deduplicate, or replace an example.

Agent-session performance inputs live separately in
[`benchmarks/`](../benchmarks/README.md). Use `temper-benchmark` for trace
analysis, repetitions, structural metrics, and caller-owned baseline
comparisons; do not add benchmark or timing semantics to scenario manifests.
See [Benchmark agent sessions](../docs/how-to/benchmark-agent-sessions.md) for
the operator workflow.

## Runnable scenarios

`temper-scenario run` exposes one public runner:

- `manifest` — live-only validation-grade e2e runner. The checked-in
  `basic-delivery`, `implementation-pr-handoff`, `codebase-memory-agent`,
  `model-failure-recovery`, `plan-centric-feature-branch`, and `target-ux-e2e`
  scenarios select this runner with
  `runner.uses = "manifest"` and boot real Forgejo, real `forgejo-runner` CI, a
  real standalone `temper` process, and Jig fake LLM agents.
  Hermetic/MemoryForge/in-process substitutes are rejected, and no scenario-name
  compatibility aliases are registered.

Run output always prints the scenario source classification, confidence tier, and
manifest topology before the verdict. Bundles under this repository's
`scenarios/` directory are labeled `checked-in scenario`; valid copied bundles
outside that corpus are labeled `ephemeral validation bundle`. Live manifest
output also includes the Forgejo URL, issue/PR numbers, CI job
evidence when applicable, convergence timing, fake LLM request counts, structured
Temper event facts, and log/artifact paths. Use
`cargo dev-scenario-run scenarios/<name>` for an explicit manual live lane (it
builds and passes the standalone `temper` binary). The command intentionally has
no default scenario.

### Single validator workflow command

Use `temper-scenario validate` when you want the complete run-and-report UX for
an explicitly supplied validation bundle. It runs the bundle, writes
`run-evidence.json`, runs the `validate-pr` report builder against that evidence,
and leaves Markdown plus JSON validation output in one artifact directory. It
does not resolve a feature mapping or grant landing authority:

```sh
cargo run -p temper-scenario-cli -- validate \
  --pr <merged-pr-number> \
  --sha <merged-main-sha> \
  --scenario /tmp/renamed-inherited-delivery \
  --tier live \
  --output-dir /tmp/temper-validation/pr-<merged-pr-number>
```

For live runners that need a standalone `temper`, the command resolves an
existing binary or builds `cargo build --bin temper` automatically. Promotion is
not part of this workflow; run `temper-scenario promote` separately only when an
ad-hoc bundle should become a checked-in regression input.

A config-only inherited bundle can be as small as:

```toml
name = "renamed-inherited-delivery"
intent = "Validate the merged change with the checked-in basic-delivery fixtures."

[fixtures]
extends = "scenarios/basic-delivery"

[runner]
uses = "manifest"
```

Run it with the command above and inspect the artifact directory for
`run-evidence.json`, `validation-pr-<pr>-<sha>.md`, and
`validation-pr-<pr>-<sha>.json`.

A bundle with one focused script hook adds the hook declaration and a local
script, but uses the same command path:

```toml
name = "delivery-with-branch-hook"
intent = "Validate basic delivery plus one provider-side branch cleanup check."

[fixtures]
extends = "scenarios/basic-delivery"

[runner]
uses = "manifest"

[[assertions]]
id = "branch-cleanup-observed"
kind = "command"
command = "scripts/assert-branch-cleanup.sh"
phase = "after-convergence"
timeout_ms = 5000
```

```sh
#!/usr/bin/env bash
set -euo pipefail
context="${1:?context}"
grep -q '"runner_id": "manifest"' "$context"
echo "branch cleanup evidence checked from $context"
```

Hook context, stdout, stderr, and status files are retained under
`<output-dir>/script-assertions/` and are also cited from the run evidence and
validation report.

### Structured run evidence

`temper-scenario run` can also write a versioned JSON run-evidence artifact:

```sh
temper-scenario run --tier live \
  --evidence-out validation-artifacts/run-evidence.json \
  scenarios/basic-delivery
```

The artifact records the schema/version, scenario source classification,
manifest path, scenario name, selected runner/tier/topology, resolved fixture
paths, final issue/PR/CI facts observed by the runner, convergence data, and any
known provider/log/artifact paths. `validate-pr` can render from that artifact
without scraping stdout or rerunning the scenario:

```sh
temper-scenario validate-pr \
  --pr <merged-pr-number> \
  --sha <merged-main-sha> \
  --run-evidence validation-artifacts/run-evidence.json \
  --output-dir validation-artifacts
```

`--run-evidence` accepts either a JSON file or a directory containing
`run-evidence.json` (or one `*.run-evidence.json` file). Supplying both
`--scenario` and `--run-evidence` makes `validate-pr` re-check the manifest and
report scenario/tier/runner/source mismatches, but it still does not rerun the
scenario for evidence population. The older direct path remains available: omit
`--run-evidence` and pass `--scenario <PATH>` when you want `validate-pr` to run
a supported scenario itself.

### Declarative expectation assertions

After a supported runner completes, `temper-scenario run` evaluates manifest
`[expect]` counts and `[[expect.checks]]` entries against the structured run
evidence it just produced. Results are printed under an `assertions:` block and
stored in the run-evidence JSON as `assertions.results[]`. A failed assertion
makes `temper-scenario run` exit non-zero after the runner has completed; when
`--evidence-out` is supplied, the evidence file is still written with the failed
assertion diagnostics. `temper-scenario validate-pr --run-evidence ...` renders
those stored assertion results without rerunning the scenario and fails the
report when any stored assertion failed.

Supported primitives are intentionally limited to facts already present in run
evidence:

- `[expect] merged_pull_requests = <n>` counts final PRs whose state is
  `merged`.
- `[expect] closed_parent_issues = <n>` counts final issues whose state is
  `closed`.
- `[expect] created_pull_requests = <n>` counts structured `pr.opened` events
  with `action = "created"`.
- `[expect] refreshed_pull_requests = <n>` counts structured `pr.updated` events
  with `action = "refreshed"`.
- `template = "single-pr-merged-source-closed"` checks for one merged PR and one
  closed source/parent issue when the runner identifies that issue (or when only
  one issue is present).
- `template = "no-duplicate-prs"` checks implementation-labeled PRs for duplicate
  `head_branch` facts.
- `[[expect.checks]] artifact = "issue:<id>"` supports `state`, `labels`, and
  `labels_cleared` against final issue facts. If older evidence has no issue ids
  and exactly one issue, the engine uses that single issue for compatibility.
- `[[expect.checks]] artifact = "pull_request"` (or `pull_request:<id>`) supports
  `state`, `labels`, `labels_cleared`, `title`, `body_prefix`,
  `body_prefix_file`, `stale_body_absent`, `metadata_kind`, `metadata_parent`,
  `correlation_key`, and `ci = "passed"`/`"failed"` against final PR, body,
  metadata, and CI-job conclusion facts.
- `[[expect.events]]`, `[[expect.sequence]]`, and `[[expect.count]]` assert over
  captured structured Temper JSON events from live manifest runs. They support
  event presence, ordered sequences, and grouped count/no-duplicate checks over
  fields such as `event`, `artifact_ref`, `pr_ref`, `source_artifact`,
  `transition`, `action`, handoff metadata/title/body-source fields, and CI
  `conclusion`.

Model-recovery scenarios additionally have bounded, pre-convergence primitives:

```toml
# On the Jig step: inject one late failure after two normal engineer
# requests (so the same-turn retry can succeed), then a bounded consecutive
# burst later in the same role's request stream (so deferral can be exercised).
late_stream_failure = { role = "engineer", bursts = [{ after_requests = 2, failures = 1 }, { after_requests = 5, failures = 14 }] }

[[steps]]
id = "observe-deferral"
action = "provider.wait_deferred"
artifact = "issue:source"
generation = 1
timeout_ms = 45000

[[steps]]
id = "wake-provider"
action = "provider.health_wake"
artifact = "issue:source"
expected_generation = 1
event_id = "provider-healthy-1"
```

`provider.wait_deferred` is observation-only. `provider.health_wake` must follow a
matching observation and invokes the engine's HMAC-authenticated, workstream-
scoped health capability; assertions cannot invoke either operation. The live
harness may bound recovery timing without changing production defaults:

```toml
[live_harness.recovery]
model_retry_max_attempts = 2
model_retry_base_delay_ms = 1
model_retry_max_delay_ms = 2
model_retry_jitter_percent = 0
session_failure_limit = 1
fresh_session_limit = 1
provider_deferral_limit = 3
provider_deferral_delay_secs = 300
model_recovery_slo_secs = 7200
```

Structured recovery assertions use `[[expect.provider_requests]]` (`role`,
`exactly`/`min`/`max`, `unique`), `[[expect.recovery]]` (event, action,
attempt, disposition, boundary/event kind, provider request/status/code facts,
session/cumulative counts, elapsed time, deferral count, generation),
`[[expect.stimuli]]`, `[[expect.workspace]]` (`retained`, `path_contains`, and
bounded tool-effect counts), and `[[expect.publication]]` (branch/PR counts and
`blocked_while_deferred = true`). All are required by default. Missing event,
request identity, retained path, stimulus, or publication-fence evidence blocks
validation rather than silently passing. Unknown required assertion fields are
reported as unsupported and also block validation.

Unsupported or missing-fact declarations remain visible diagnostics. Assertions
are required by default, so any failed, missing, timed-out, or unsupported
required result blocks the run. Set `required = false` only for deliberately
informational evidence; an optional unsupported result remains visible without
blocking. Add production observability or a generic probe before making a
provider-only fact required.

### Script assertion hooks

Validation bundles may add focused bash hooks as a constrained escape hatch for
provider-side checks that are not declarative yet:

```toml
[[assertions]]
id = "branch-deleted"
kind = "command"
command = "scripts/assert-branch-deleted.sh"
phase = "after-convergence"
timeout_ms = 30000
# cwd = "repo"             # optional; bundle root is the default
# env = ["SAFE_FLAG"]      # optional explicit pass-through allowlist
```

Only `kind = "command"` bash hooks at `phase = "after-convergence"` are
supported. `command` and optional `cwd` are local manifest paths: absolute paths,
URLs, missing files, and `..` components are rejected by `temper-scenario check`.
When a hook is inherited through `[fixtures] extends`, those path fields resolve
from the manifest that declared them; otherwise they resolve from the current
bundle. Hooks run under Rust-owned orchestration after the runner has produced
structured evidence and after declarative assertions have been evaluated.

Temper writes a JSON context file and passes it as both the first script argument
and `TEMPER_SCENARIO_CONTEXT`. The context contains the full `run_evidence`, the
scenario/manifest paths, hook and run artifact directories, runner id, tier, and
known provider facts such as Forgejo URL, repo slug, issue/PR number, head
branch, and merged SHA. Scripts should read that context, assert one focused
condition, print concise evidence, and exit non-zero on failure. They should not
perform scenario orchestration, cleanup shared state, or require ambient
credentials. The hook environment is cleared except for a minimal `PATH`,
`LC_ALL`, Temper context variables, and extra variables named explicitly in
`env`; allowlisted variables may not override Temper-managed names.

Each hook has a required/default timeout (`timeout_ms`, default 30000, maximum
600000). Stdout, stderr, status, and context paths are retained under the run
artifact directory, appended to the structured run evidence, printed in the
`assertions:` block, and rendered by `temper-scenario validate-pr --run-evidence`.
A failed hook, timeout, or spawn/configuration error makes `temper-scenario run`
exit non-zero after writing evidence when `--evidence-out` is supplied; unsafe
manifest paths are rejected by `check`/`run` before execution.

## Validation reports vs. promotion artifacts

Every post-merge validation run must produce a validation report: what target
(PR, issue, epic, or aggregate) and commit/PR set was validated, which scenario
or ad-hoc case was run, whether it came from the checked-in corpus or an
ephemeral bundle, which confidence tier and manifest topology were used, what
commands or tooling ran, where logs/artifacts live, and the final pass/fail
result. That report is the required deliverable for validation work.

Changing `scenarios/` is optional. A checked-in scenario change is a promotion
artifact: it captures a case that should become a reusable regression input after
it has proven useful. Not every validation report should add or update a
scenario, and a scenario should not be edited merely to make one validation
report pass. Operators can use `temper-scenario promote` to draft a promotion
candidate from a validation report or artifact directory, but that command is
only a prompt scaffold: it does not create Forgejo issues or PRs, and it does
not replace the required validation report.

## Feature scenario scaffold and deterministic mapping

Use `scaffold` for feature/plan work instead of the Markdown-only `promote`
prompt. It creates a small inherited bundle with a local Jig script, a bounded
runtime, typed feature/plan mapping, and an explicit claim → stimulus →
observable → assertion contract:

```sh
cargo run -p temper-scenario-cli -- scaffold \
  --feature ai/temper#778 \
  --plan ai/temper#779 \
  --source-branch feature/778-exact-head-validation \
  --name exact-head-feature-validation
```

The command refuses to overwrite an existing scenario path and writes only
`scenario.toml`, `README.md`, and `jig/<name>.json`. It does not emit
credentials, logs, caches, evidence, or other runtime state. The generated
manifest inherits `scenarios/basic-delivery` by default and uses `[jig]` to
replace the inherited fake-LLM script without copying the workflow, repo, or CI
fixtures. Authors should replace the scaffold Jig responses and narrow the
required assertions while preserving the inherited validation-grade topology.

Mapped scenarios declare both typed authoring metadata sections:

```toml
[validation]
feature = "ai/temper#778"
plan = "ai/temper#779"
source_branch = "feature/778-exact-head-validation"
change = "new" # use "updated" when the scenario exists at the landing base

[feature_contract]
claim = "The feature enforces exact-head validation."
stimulus = "Exercise one stale and one current validation attempt."
observable = "Structured head, digest, assertion, and landing-gate facts."
assertion = "Only current passing evidence authorizes landing."
runtime_budget_seconds = 600
jig_script_path = "jig/exact-head-feature-validation.json"
```

Resolve a feature mapping at the checked-out head before a focused CI or
validator run:

```sh
cargo run -p temper-scenario-cli -- resolve-feature \
  --feature ai/temper#778 \
  --landing-base origin/main \
  --json-out target/feature-scenario-mapping.json
```

Resolution selects exactly one active explicit mapping and rejects missing,
duplicate, inactive, unsafe, dirty, unchanged, or digest-mismatched scenarios.
It compares the mapped path and prior feature mapping against the supplied
landing base, so copied or renamed directories do not acquire an implicit
mapping. Successful stdout and `--json-out` content are identical
`temper.scenario.feature-mapping.v1` JSON. They include the stable mapping id,
feature and plan, repo-relative scenario and manifest paths, declared source
branch, exact checkout head, resolved landing-base SHA, change classification,
and a SHA-256 digest.

For the cohesive pre-merge run, use the mapping-aware command instead of copying
the resolved path into a second command:

```sh
cargo dev-scenario-validate-feature \
  --feature ai/temper#778 \
  --landing-base origin/main \
  --source-branch feature/778-exact-head-validation \
  --pr <landing-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

It verifies that mapping, branch, and head still agree, executes only the mapped
live scenario, and retains a joined audit payload. See
[Run focused feature validation](../docs/how-to/run-focused-feature-validation.md).

The digest canonicalizes the resolved inherited manifest, hashes referenced
fixture content, and hashes all files owned by the mapped scenario in sorted
order. Absolute checkout paths and directory enumeration order do not affect it.
The same mapping fields are represented in `ScenarioMetadataContext`, run
evidence, and `ValidatorResult` so validator handoffs and Forge audit rendering
can carry the identity without scraping CLI prose.

## Authoring model

Author scenarios as data, not as runners:

- Keep the manifest in `scenario.toml` and keep new paths relative to the
  manifest that declares them.
- Store fixture inputs as ordinary files that can be copied into a throwaway
  environment by a future Rust checker or runner.
- Do not commit credentials, generated logs, runtime state, caches, or secrets.
- Prefer minimal repo seeds. A scenario should include only the default-branch
  files required to exercise the workflow.
- If a scenario is promoted from an example, copy only the stable fixture inputs
  needed by validation. Leave the source example unchanged.

## Fixture inheritance

Ephemeral validation bundles may reuse fixture material from a checked-in
scenario instead of copying `config/`, `repo/`, issue body files, or workflow
JSON. Declare the relationship explicitly in the child manifest:

```toml
[fixtures]
extends = "scenarios/basic-delivery"
```

`extends` is a local relative filesystem path to another scenario directory or
manifest. It is resolved first relative to the child manifest and then relative
to the repository workspace root so throwaway bundles can point at
`scenarios/basic-delivery`. Absolute paths, URLs, missing bases, inheritance
cycles, and `..` components are rejected with `fixtures.extends` diagnostics.

Overlay semantics are intentionally simple: the inherited manifest supplies
defaults, and the child manifest recursively overrides tables while replacing
arrays and scalar values wholesale. This lets a validation bundle set a distinct
`name`, `[runner] uses = "manifest"`, or local `[expect]` metadata while
reusing workflow, repo seed, CI, and issue-body fixtures. Local file references
are resolved relative to the manifest that declared them, so inherited
`config/workflow.json`, `repo`, and issue body paths continue to point at the
base scenario.

Promotion remains optional and reviewable. Checked-in scenarios should either be
self-contained or explicitly declare `[fixtures] extends = ...`; do not rely on
implicit fixture lookup. Promoting an inherited ephemeral bundle into the corpus
should preserve the explicit inheritance only when reviewers want that ongoing
coupling, otherwise copy the stable fixture inputs as part of the promotion PR.

## Manifest fields

The first-pass manifest shape is intentionally small and TOML-native so a later
Rust checker can parse it directly.

Top-level metadata:

| Field | Purpose |
| --- | --- |
| `schema` | Manifest schema identifier, currently `temper.scenario.v1`. |
| `name` | Stable scenario slug matching the directory name. |
| `status` | Lifecycle state such as `draft`, `active`, or `retired`. |
| `intent` | One-sentence behavior the scenario protects. |
| `introduced_by` | Issue or PR that introduced the scenario. |
| `owner_area` | Product area responsible for keeping the scenario useful. |
| `stability` | Compatibility promise, for example `provisional` or `stable`. |
| `timeout` | Human-readable wall-clock budget for a future runner. |

Required sections:

| Section | Purpose |
| --- | --- |
| `[topology]` | Runtime boundary being validated: forge, runner, Temper process shape, agent model, and repo set. |
| `[workflow]` | Workflow fixture name, format, and local path. |
| `[[repos]]` | Repositories to create, their default branches, optional seed directories, and CI file placement. |
| `[[issues]]` | Issues to seed, including title, author model, labels, and body file. |
| `[[agents]]` | Roles expected to service the scenario and the tool or automation mode they use. |
| `[expect]` | High-level convergence result plus machine-checkable expectation entries. |
| `[[steps]]` | Manifest-runner setup primitives for the real e2e stack (Forgejo, forgejo-runner, repo/issue seeding, Jig fake LLM, Temper launch, convergence wait). |
| `[change_policy]` | Compatibility notes for future edits and whether a validation report is required. |

Optional sections:

| Section | Purpose |
| --- | --- |
| `[fixtures]` | Explicit local fixture inheritance for bundles that reuse another scenario's manifest/fixture defaults. |
| `[observability]` | Structured log capture settings for live manifest runs (`log_format = "json"`, `rust_log = "temper=debug"`). |

Scenarios may add explanatory keys inside those sections, but new keys should be
documented in the scenario README when they affect validation semantics.

## Assertion templates

`[expect]` may name stable assertion templates before every template has a full
runner implementation. Use `template = "<name>"` for one contract or
`templates = ["<name>", ...]` for several; `[[expect.checks]]` entries can remain
beside templates for explanatory or future machine checks.

The initial catalog accepted by `temper-scenario check` is:

- `single-pr-merged-source-closed` — one implementation PR merges and closes its source issue.
- `review-requested-then-approved` — a review request is made before approval unblocks landing.
- `ci-fails-then-passes` — a failing CI signal is followed by a passing replacement signal.
- `cross-repo-fanout-converges` — coordinated work fans out across repositories and converges.
- `no-duplicate-prs` — repeated progress signals do not create duplicate implementation PRs.
- `quiescent-after-merge` — no further workflow actions remain after successful merge convergence.
- `webhook-progress-before-poll-backstop` — webhook progress is observed before any polling backstop is needed.

Unknown template names are manifest validation errors so checked-in scenarios
refer only to cataloged behavior contracts.

## Expected layout

```text
scenarios/
├── README.md
└── <scenario-slug>/
    ├── scenario.toml
    ├── README.md
    ├── config/
    │   ├── workflow.json
    │   ├── ci.yml
    │   └── intake-issue.md
    └── repo/                  # optional default-branch seed
        ├── README.md
        └── .forgejo/workflows/ci.yml
```

Local path references in `scenario.toml` must point at files or directories
relative to the manifest that declares them. Self-contained checked-in scenarios
keep those files in the same bundle; inherited references continue to point at
the explicitly extended base. Prefer duplicating small fixture files over
depending on paths in `examples/` so validation can run from the checked-in
corpus alone.

## Relationship to landing and post-merge validation

Focused feature validation resolves and executes one mapped scenario before an
aggregate feature PR lands. Its CI evidence is retained and compatible with the
workflow-native validator contract, but the workflow-native exact-head gate is
the landing authority. Any feature-head change makes an older attempt stale.
See [Run focused feature validation][focused-validation].

The broad post-merge workflow remains a regression report and may still run
`basic-delivery`; it is not the focused feature selector or landing authority.
The [post-merge validator handoff][validator-handoff] describes the generic
workflow architecture. `temper-scenario validate-pr` remains a temporary/manual
report bridge. Scenario promotion also stays a separate follow-up from
validation.

[focused-validation]: ../docs/how-to/run-focused-feature-validation.md
[validator-handoff]: ../docs/reference/post-merge-validator-handoff.md

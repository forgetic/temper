# Implementation PR handoff scenario verification report

Target feature: #52 / PR #53, "Engineer PR handoff: carry agent-authored implementation PR title/body through PR create/update". This report was first prepared for #76 and was refined for #78 after the proof was promoted into a checked-in `temper-scenario run` target.

## Verdict

**Pass.** `temper-scenario run` now executes a deterministic in-process proof for `scenarios/implementation-pr-handoff`. The runner drives the daemon `ForgeApplier` workflow path over `MemoryForge` with scripted engineer/coding-workspace success results, then checks that authored implementation PR titles/bodies are present and that Temper-owned metadata still classifies the PR as `implementation_pr` with the source issue parent/correlation relation.

## Behavior contract extracted from #52/#53 and current code

The contract under validation is:

- a no-verdict engineer/coding-workspace success may author `title` and `body` for the implementation PR handoff;
- PR creation prefers the non-blank authored title over the generic `Implement #<issue>: <issue title>` fallback;
- PR creation prefers the non-blank authored report body over the fallback `Summary: ...` body, then appends the Temper workflow metadata block;
- refreshing an existing implementation PR updates the durable PR title/body to the newly authored handoff instead of leaving stale generated/default text;
- the implementation PR remains classified as `implementation_pr` and retains the parent issue relation in workflow metadata.

Relevant current code paths:

- `crates/temper-engine/src/forge_applier/success.rs` reads `JobResult.title` and `JobResult.body` from the engineer success and passes them into coordinated PR creation/update.
- `crates/temper-engine/src/forge_applier/coordinated.rs` builds the `CreatePullRequest` input with `implementation_pr_title(...)` and `implementation_pr_body_from_report_or_summary(...)`.
- `crates/temper-runner/src/workspace_request.rs` defines the fallback-vs-authored title/body behavior and appends the metadata block.
- `crates/temper-scenario-cli/src/bin/temper-scenario/implementation_pr_handoff.rs` is the checked-in scenario runner that exercises the create and refresh paths.

## Checked-in scenario design

Scenario bundle:

```text
scenarios/implementation-pr-handoff/
├── scenario.toml
├── README.md
├── config/
│   ├── workflow.json
│   ├── source-issue.md
│   ├── create-handoff.md
│   └── refresh-handoff.md
└── repo/
    └── README.md
```

The manifest name is `implementation-pr-handoff`. Its topology describes one in-memory repository, `acme/service`, the ForgeApplier path in process, and a scripted coding-workspace result. The runner performs two deterministic checks:

1. seed a ready code issue, apply a success result with `title = "Implement durable PR handoff"` and `config/create-handoff.md`, then assert the newly opened implementation PR starts with that authored body and has `implementation_pr` metadata with the source issue parent and `pr-for-code-<issue>` correlation key;
2. seed another ready code issue plus an existing implementation PR containing stale title/body but valid metadata, apply a second success result with `title = "Implement refreshed handoff"` and `config/refresh-handoff.md`, then assert the same PR number was refreshed, stale body text is gone, and metadata kind/parent/correlation were preserved.

This remains hermetic: it uses `MemoryForge`, a checked-in workflow fixture, fake `JobResult` structs, exact string/metadata assertions, and no live Forgejo, CI runner, network, model provider, or clock-sensitive convergence loop.

## Commands and evidence

Validate the checked-in scenario manifest:

```sh
cargo run -p temper-scenario-cli -- check scenarios/implementation-pr-handoff
```

Expected output:

```text
OK - checked 1 scenario(s).
```

Run the proof through the scenario framework. The command defaults to the
hermetic tier; `--tier live` is reserved for live manifest scenarios and fails
for this MemoryForge-specific scenario rather than substituting memory evidence:

```sh
cargo run -p temper-scenario-cli -- run --tier hermetic scenarios/implementation-pr-handoff
```

Expected evidence shape:

```text
scenario: implementation-pr-handoff
source: checked-in scenario
confidence tier: hermetic (fast in-process/memory runner; lower confidence than live; not a live Forgejo proof)
manifest topology:
  kind: single-repo-in-memory-forge
  forge: memory
  runner: temper-scenario-forge-applier
  temper: in-process
  agent_model: scripted-coding-workspace-result
verdict: passed
evidence:
  create authored PR title/body: PR #... for issue #... has title "Implement durable PR handoff" and body prefix "# Implementation report"
  refresh authored PR title/body: existing PR #... for issue #... has title "Implement refreshed handoff" and stale body text was cleared
  workflow metadata/source relation: create parent #... correlation pr-for-code-...; refresh parent #... correlation pr-for-code-...
  metadata kind verified: implementation_pr
```

`cargo test -p temper-engine --test forge_apply handoff` remains a useful lower-level implementation test, but it is no longer the main proof command for this verification report.

## `temper-scenario` UX notes

Improved:

- A feature-specific proof can now be launched with `temper-scenario run`, so the report cites the same operator-facing scenario command for manifest validation and execution while labeling the source, hermetic tier, and manifest topology.
- The evidence lines are concise and name the observed title/body and metadata/source relation rather than pointing readers at raw unit-test names.
- `temper-scenario check scenarios` now validates both checked-in runnable scenario bundles.

Current limitation:

- Runner dispatch is still a small hard-coded match in the CLI (`basic-delivery` and `implementation-pr-handoff`), not a generic runner registry. That is intentional for this refinement because the handoff proof is focused and the non-goal was to avoid building a broad registry unless it was smaller than a direct runner.

## Follow-up suggestions

1. Add a small runner-registration seam when a third focused scenario appears.
2. Add assertion templates for implementation PR handoff fields, for example authored title, authored report body prefix, metadata `kind`, metadata `parents`, and correlation key.
3. Let `temper-scenario validate-pr` ingest a scenario-run transcript so feature-specific validation reports can be generated instead of hand-written.

# Implementation PR handoff scenario verification report

Target feature: #52 / PR #53, "Engineer PR handoff: carry agent-authored implementation PR title/body through PR create/update". This report was prepared for #76 from `agent/pr-for-code-76` at `0b7652862ce449f275c56c4705b7d49016b99d5e`, before this report file was added.

## Verdict

**Pass.** A deterministic in-process proof showed that an engineer success result with an authored implementation PR title and report body is carried into both newly-created and refreshed implementation PRs, while Temper-owned workflow metadata keeps the source issue relation intact.

The only framework limitation found is that `temper-scenario run` currently runs only the checked-in `basic-delivery` scenario. The handoff-specific scenario therefore remained ephemeral and used `temper-scenario check` plus the existing focused in-process `ForgeApplier` harness rather than adding a new runner.

## Behavior contract extracted from #52/#53 and current code

The contract under validation is:

- a no-verdict engineer/coding-workspace success may author `title` and `body` for the implementation PR handoff;
- PR creation prefers the non-blank authored title over the generic `Implement #<issue>: <issue title>` fallback;
- PR creation prefers the non-blank authored report body over the fallback `Summary: ...` body, then appends the Temper workflow metadata block;
- refreshing an existing implementation PR updates the durable PR title/body to the newly authored handoff instead of leaving stale generated/default text;
- the implementation PR remains classified as `implementation_pr` and retains the parent issue relation in workflow metadata.

Relevant current code paths inspected:

- `crates/temper-engine/src/forge_applier/success.rs` reads `JobResult.title` and `JobResult.body` from the engineer success and passes them into coordinated PR creation/update.
- `crates/temper-engine/src/forge_applier/coordinated.rs` builds the `CreatePullRequest` input with `implementation_pr_title(...)` and `implementation_pr_body_from_report_or_summary(...)`.
- `crates/temper-runner/src/workspace_request.rs` defines the fallback-vs-authored title/body behavior and appends the metadata block.
- `crates/temper-engine/tests/forge_apply/handoff.rs` contains the focused deterministic create/update assertions used for this proof.

## Scenario design

Ephemeral artifact directory:

```text
/tmp/temper-scenario-handoff-validation-76/
├── scenario.toml
├── workflow.json
└── issue.md
```

The manifest name was `implementation-pr-handoff`. Its topology described a single in-memory Forge repository `acme/service`, the `ForgeApplier` workflow path in process, and a scripted fake engineer/coding-workspace success result. The fake inputs were:

- ready code issue title: `ready code issue`;
- branch: `agent/pr-for-code-<issue>`;
- create title/body: `Implement durable PR handoff` and `# Implementation report\n\n- Added the durable handoff path.`;
- update title/body: `Implement refreshed handoff` and `# Implementation report\n\nLatest compact handoff.`;
- repair title/body: `Refresh PR after feedback` and `# Implementation report\n\nFixed the failing PR feedback.`.

This is deterministic because the proof harness uses `MemoryForge`, the fixed reference-delivery workflow fixture, fake worker `JobResult` structs, exact string assertions, and no live Forgejo, CI runner, network, model provider, or clock-sensitive convergence loop. The ephemeral `workflow.json` file existed only so `temper-scenario check` could validate local manifest path references; the runnable proof used the existing Rust harness.

The proof intentionally stayed ephemeral. Promoting a checked-in scenario or adding a new `temper-scenario run` target would be useful later, but the current framework gap is runner registration for feature-specific focused harnesses; adding that abstraction was larger than needed for this validation.

## Commands run and output

Create/check the ephemeral scenario manifest:

```sh
cargo run -p temper-scenario-cli -- check /tmp/temper-scenario-handoff-validation-76
```

Output:

```text
OK - checked 1 scenario(s).
```

Confirm the current runner boundary:

```sh
cargo run -p temper-scenario-cli -- run /tmp/temper-scenario-handoff-validation-76
```

Output:

```text
temper-scenario run: unsupported scenario `implementation-pr-handoff` at /tmp/temper-scenario-handoff-validation-76; this first runner supports only scenarios/basic-delivery
```

Run the deterministic handoff proof harness:

```sh
cargo test -p temper-engine --test forge_apply handoff -- --nocapture
```

Output:

```text
running 4 tests
test handoff::pull_request_writable_success_refreshes_same_pr_handoff_without_opening_another ... ok
test handoff::success_result_creates_implementation_pr_with_agent_authored_handoff ... ok
test handoff::success_result_refreshes_existing_implementation_pr_handoff ... ok
test trivial::existing_trivial_pr_with_working_label_gets_final_handoff ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 29 filtered out; finished in 0.52s
```

The `trivial::...handoff` test matched the `handoff` filter but is not part of the #52/#53 evidence. The three `handoff::...` tests are the relevant proof.

## Evidence for title/body and source relation

The passing assertions prove the contract as follows:

- `success_result_creates_implementation_pr_with_agent_authored_handoff`
  - applies a fake engineer success with `result.title = "Implement durable PR handoff"` and the authored implementation report body;
  - asserts `pull.title == "Implement durable PR handoff"`;
  - asserts `pull.body.starts_with(report)`;
  - asserts the fallback `Summary: short log summary` is absent;
  - parses the workflow metadata and asserts `kind == implementation_pr`, `parents == [same repo issue]`, and `correlation_key == pr-for-code-<issue>`.
- `success_result_refreshes_existing_implementation_pr_handoff`
  - seeds an existing implementation PR with stale generated title/body and metadata pointing at the issue;
  - applies a new authored title/body;
  - asserts the PR title is `Implement refreshed handoff`, the body starts with the new report, the old report text is absent, and the metadata is still the seeded parent/correlation metadata.
- `pull_request_writable_success_refreshes_same_pr_handoff_without_opening_another`
  - seeds an existing PR and applies a pull-request-writable repair result;
  - asserts exactly one PR remains, the same PR number is refreshed, the title/body match the authored repair handoff, stale body text is absent, and the metadata still points at the original source issue.

Together these cover create, existing-PR update, and pull-request repair update through the workflow/forge-applier path.

## `temper-scenario` UX notes

Easy:

- The manifest shape was quick to reuse for a non-framework feature, and `temper-scenario check` gave a clean deterministic sanity check for the scenario artifact.
- The in-memory forge and existing in-process test harness made the actual proof cheap and hermetic.
- The validation-report convention made it clear what to record: target contract, topology, commands, artifacts, evidence, limitations, and follow-ups.

Hard or confusing:

- `temper-scenario run` cannot dispatch feature-specific scenarios; after a manifest validates, the only runnable scenario is still `basic-delivery`.
- There is no manifest field or CLI option to attach an existing focused Rust harness as the scenario runner/evidence source.
- The current assertion-template catalog has delivery/convergence templates but no template for PR title/body/metadata handoff.
- `validate-pr` is oriented around post-merge PR/SHA reports and the supported runner; it cannot yet express this feature-specific pass verdict with external command evidence without a hand-written report.

## Follow-up suggestions

1. Add a small runner-registration seam so `temper-scenario run <manifest>` can dispatch focused in-process validators without hard-coding every scenario in the CLI front end.
2. Add assertion templates for implementation PR handoff fields, for example authored title, authored report body prefix, metadata `kind`, metadata `parents`, and correlation key.
3. Let `temper-scenario validate-pr` ingest explicit command evidence or a scenario-run transcript so feature-specific validation reports can be generated instead of hand-written.
4. Improve the unsupported-run message with a hint such as: "manifest check passed; no runner is registered for this scenario name."
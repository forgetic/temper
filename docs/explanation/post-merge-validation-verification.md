# Post-merge validation tooling verification report

> Historical note: this report records the first-pass hermetic bridge that
> existed before the validation-grade live `basic-delivery` lane was wired into
> `temper-scenario`. Current post-merge validation uses `--tier live` with a
> standalone `temper` binary and records real Forgejo/CI/convergence/log
> evidence; see [Find a post-merge validation report](../how-to/post-merge-validation-report.md).

This report verifies the first-pass post-merge validation path for #35 using
current `main` as the merged state and PR #73 as the representative recently
merged implementation PR. The verification was run from a clean workspace before
this report was added; `agent/pr-for-code-74` and `main` both resolved to
`8ce54dfb2012e1881196ebbc3d63c60d90aa6d31`.

## Local command path

Commands run:

```sh
cargo run -p temper-scenario-cli -- check scenarios
cargo run -p temper-scenario-cli -- run --tier hermetic scenarios/basic-delivery
mkdir -p /tmp/temper-post-merge-validation/pr-73
cargo run -p temper-scenario-cli -- validate-pr \
  --pr 73 \
  --sha "$(git rev-parse HEAD)" \
  --scenario scenarios/basic-delivery \
  --output-dir /tmp/temper-post-merge-validation/pr-73
cargo run -p temper-scenario-cli -- promote \
  /tmp/temper-post-merge-validation/pr-73 \
  --name verification-basic-delivery \
  --output-dir /tmp/temper-post-merge-validation/promotion
```

Produced artifacts:

- `/tmp/temper-post-merge-validation/pr-73/validation-pr-73-8ce54dfb2012e1881196ebbc3d63c60d90aa6d31.md`
- `/tmp/temper-post-merge-validation/promotion/scenario-candidate-verification-basic-delivery.md`

## Verdict

Local scenario validation path: **pass**.

The manifest check, deterministic `basic-delivery` run, `validate-pr` report
write, and optional `promote` draft generation all completed successfully. The
Markdown report itself correctly returned `Verdict: inconclusive` for the
end-to-end PR identity claim because the temporary bridge accepts the PR number
and SHA as operator input and does not query live Forgejo state. That limitation
does not block the cheap local path; it is the expected current boundary of the
first-pass implementation.

No live Forgejo/e2e suite or broad `cargo test` run was needed.

## Evidence from the validation report

The generated validation report contains scenario check and scenario run evidence
for `basic-delivery`:

```text
- [observed] Scenario `basic-delivery` manifest validates at `scenarios/basic-delivery`.
  - scenario check passed
- [observed] Supported deterministic basic-delivery scenario completes successfully.
  - scenario run passed
```

It also records the concrete scenario evidence:

```text
3. **scenario check** — Scenario check passed for `scenarios/basic-delivery`.
   - scenario: `basic-delivery`
   - manifest: `scenarios/basic-delivery/scenario.toml`
   - source: checked-in scenario
   - confidence tier: hermetic (fast in-process/memory runner; lower confidence than live; not a live Forgejo proof)
   - manifest topology.kind: `single-repo-forgejo-standalone`
   - manifest topology.forge: `forgejo`
   - manifest topology.runner: `forgejo-actions-host`
   - manifest topology.temper: `standalone`
   - manifest topology.agent_model: `scripted-fake-llm`
4. **scenario run** — Deterministic basic-delivery scenario run completed successfully.
   - source: checked-in scenario
   - confidence tier: hermetic (fast in-process/memory runner; lower confidence than live; not a live Forgejo proof)
   - manifest topology.kind: `single-repo-forgejo-standalone`
   - manifest topology.forge: `forgejo`
   - manifest topology.runner: `forgejo-actions-host`
   - manifest topology.temper: `standalone`
   - manifest topology.agent_model: `scripted-fake-llm`
   - seeded issue: #1 "Service banner should identify the environment" closed as code
   - implementation PR: #1 merged with passing CI (1 completed job(s))
   - closed parent issues: 1
   - actions: mechanical=2, role:architect=1, role-audit:architect=0, role:engineer=1, ci=1
   - report: ticks=15 workers=5
```

The direct `run --tier hermetic scenarios/basic-delivery` command also reported:

```text
scenario: basic-delivery
source: checked-in scenario
confidence tier: hermetic (fast in-process/memory runner; lower confidence than live; not a live Forgejo proof)
manifest topology:
  kind: single-repo-forgejo-standalone
  forge: forgejo
  runner: forgejo-actions-host
  temper: standalone
  agent_model: scripted-fake-llm
verdict: passed
```

## Promotion draft evidence

`promote` accepted the validation artifact directory and wrote a scenario
candidate prompt at:

```text
/tmp/temper-post-merge-validation/promotion/scenario-candidate-verification-basic-delivery.md
```

The draft identifies the source artifact directory and intended scenario slug:

```text
# Scenario promotion candidate: verification-basic-delivery

- Source validation artifact: `/tmp/temper-post-merge-validation/pr-73`
- Source artifact kind: validation artifact directory
- Intended scenario name/slug: `verification-basic-delivery` (supplied from `verification-basic-delivery`)
```

The generated file is intentionally a scaffold with TODO sections for promotion
rationale, stable behavior, fixture notes, and promotion boundaries; it does not
claim to create a complete scenario.

## Post-merge workflow sanity check

`.forgejo/workflows/post-merge-validation.yml` is wired as the expected
merged-only lane:

- Trigger: `pull_request` events targeting `main` with `types: [closed]`.
- Merge guard: the job runs only when
  `${{ github.event.pull_request.merged == true }}`.
- Target checkout: `MERGED_MAIN_SHA` is populated from
  `${{ github.event.pull_request.merge_commit_sha || github.sha }}`, and
  `actions/checkout@v4` checks out the same expression. The workflow then
  compares `git rev-parse HEAD` with `MERGED_MAIN_SHA`, records the checked-out
  SHA in `$GITHUB_ENV`, and logs the PR number plus artifact directory.
- Local mirror: the workflow runs the same cheap commands checked here:
  scenario manifest check, deterministic hermetic `basic-delivery` run, and
  `validate-pr` into `validation-artifacts/post-merge-pr-<pr-number>`.
- Report retention and visibility: the workflow writes `report-path.txt`, tests
  that the Markdown report exists, prints the path, prints the full report in a
  log group, appends it to `$GITHUB_STEP_SUMMARY` when available, and uploads the
  whole report directory as artifact `post-merge-validation-pr-<pr-number>`.
- Upload fallback: artifact upload is `continue-on-error: true`; on upload
  failure, a fallback step prints a notice, retains the job-workspace path for
  the run, and reprints the report contents if the file is still present.

This matches the intended behavior: validate the merged `main` SHA for merged PRs
only, and make the generated report available through logs, step summary, and
artifact upload when the runner supports it.

## UX notes and follow-ups

Easy:

- The local command sequence is small and mirrors the workflow steps closely.
- `validate-pr` prints the exact Markdown report path, which makes artifact
  discovery straightforward.
- The generated report is readable and contains both high-level claims and
  concrete `basic-delivery` evidence.
- `promote` is safe to run against the artifact directory and creates a draft
  prompt without mutating scenario fixtures.

Confusing:

- A successful local validation path still produces an overall report verdict of
  `inconclusive` because live PR/SHA identity is unproven. That is accurate, but
  operators need to distinguish the local scenario proof from the live Forgejo
  identity claim.
- The Cargo package is invoked as `temper-scenario-cli`, while the executed
  binary is printed as `target/debug/temper-scenario`; the alias is harmless but
  slightly surprising during first use.
- `promote` currently prints only the output path. The generated file explains
  the scaffold after opening it, but the CLI output does not say that the result
  is a TODO-bearing draft rather than a complete scenario.

Small follow-up improvements worth filing:

1. Add a one-line interpretation hint to the `validate-pr` report and/or how-to:
   scenario evidence can be satisfied while the overall temporary-bridge verdict
   remains inconclusive due to no live Forgejo lookup.
2. Have `promote` print that it wrote a draft scaffold and that promotion still
   requires human/agent completion of the TODO sections.
3. Consider documenting the package/binary name distinction in the scenario CLI
   help or post-merge validation how-to if more operators trip over it.

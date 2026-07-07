# Find a post-merge validation report

Merged pull requests targeting `main` run the `post-merge-validation`
Forgejo Actions workflow. The workflow listens for `pull_request` `closed`
events and its job is guarded so it continues only when the pull request was
merged. Opening, synchronizing, or reopening a pull request still uses the
separate PR CI workflow in `.forgejo/workflows/ci.yml`.

The post-merge job checks out the merged `main` commit for the close event,
runs the validation-grade live manifest lane for `scenarios/basic-delivery`,
and writes a Markdown
report with the temporary bridge. Both the direct run and the report record that
this is the checked-in scenario corpus, the `live` confidence tier, the manifest
topology, Forgejo URL, issue/PR numbers, CI evidence, convergence timing, fake
LLM request counts, and log/artifact paths.

```sh
cargo run -p temper-scenario-cli -- validate \
  --pr <merged-pr-number> \
  --sha <merged-main-sha> \
  --scenario scenarios/basic-delivery \
  --tier live \
  --output-dir validation-artifacts/post-merge-pr-<merged-pr-number>
```

For the live `manifest` runner, `validate` resolves an existing
standalone `temper` binary or builds one with `cargo build --bin temper` before
starting the live topology. Pass `--temper-bin <PATH>` only when you need to pin
a prebuilt binary explicitly.

The lower-level commands remain available when you need to split the run from
report rendering or inspect an intermediate artifact manually:

```sh
cargo run -p temper-scenario-cli -- run \
  --tier live \
  --temper-bin target/debug/temper \
  --evidence-out validation-artifacts/post-merge-pr-<merged-pr-number>/run-evidence.json \
  scenarios/basic-delivery
cargo run -p temper-scenario-cli -- validate-pr \
  --pr <merged-pr-number> \
  --sha <merged-main-sha> \
  --run-evidence validation-artifacts/post-merge-pr-<merged-pr-number>/run-evidence.json \
  --output-dir validation-artifacts/post-merge-pr-<merged-pr-number>
```

When both `--scenario` and `--run-evidence` are supplied, `validate-pr` checks
that the artifact's scenario, tier, source classification, and runner match the
supplied manifest but still does not rerun the scenario for report evidence. To
validate a focused ephemeral bundle instead of the checked-in `basic-delivery`
scenario, keep the same `validate` command and replace `--scenario` with the
bundle path; the [scenario authoring guide](../../scenarios/README.md#single-validator-workflow-command)
shows both a config-only inherited bundle and a bundle with a small script hook.

## Where to find the report in CI

When artifact upload is available on the Forgejo runner, the workflow uploads
the whole report directory as an artifact named:

```text
post-merge-validation-pr-<merged-pr-number>
```

Inside that artifact, the Markdown report uses this layout:

```text
validation-artifacts/post-merge-pr-<merged-pr-number>/
├── live-manifest-artifacts/              # live tier logs/artifacts when used
│   ├── init.log
│   ├── repo-populate.log
│   ├── standalone.log
│   ├── fake-llm.log
│   └── ci-diagnostics.log
├── script-assertions/                    # present when bundle hooks run
│   └── .../context.json stdout.log stderr.log status.txt
├── run-evidence.json
├── report-path.txt
├── result-path.txt
├── validation-pr-<merged-pr-number>-<merged-main-sha>.md
└── validation-pr-<merged-pr-number>-<merged-main-sha>.json
```

The Markdown report is the human-readable validation report. The sibling JSON
file is the structured `temper.validator.result.v1` result rendered from the
same `validate-pr` report model and run-evidence artifact. `report-path.txt` and
`result-path.txt` repeat the exact paths printed by the workflow command for
artifact consumers that prefer stable filenames.

The workflow always prints the report path and report contents in the job log
before attempting the upload. If `actions/upload-artifact` is not available
for the runner, the upload step is non-fatal and a fallback notice points back
to the printed Markdown contents. In that fallback mode the file is retained
only in the job workspace for the life of the run, so the log is the durable
record.

## Re-run locally

Check out the commit that was validated, then run:

```sh
mkdir -p /tmp/temper-validation/pr-<merged-pr-number>
cargo run -p temper-scenario-cli -- validate \
  --pr <merged-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --scenario scenarios/basic-delivery \
  --tier live \
  --output-dir /tmp/temper-validation/pr-<merged-pr-number>
```

Use the merged `main` SHA from the Forgejo run or PR page when reproducing an
older report. The temporary report bridge records the supplied PR number and
SHA; it does not fetch live Forgejo PR context or prove that the SHA is still
the current tip of `main`. The scenario command has no validation-grade
hermetic substitute; use focused Rust tests for fast local coverage and reserve
post-merge reports for the live manifest stack.

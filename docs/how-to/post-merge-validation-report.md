# Find a post-merge validation report

Merged pull requests targeting `main` run the `post-merge-validation`
Forgejo Actions workflow. The workflow listens for `pull_request` `closed`
events and its job is guarded so it continues only when the pull request was
merged. Opening, synchronizing, or reopening a pull request still uses the
separate PR CI workflow in `.forgejo/workflows/ci.yml`.

The post-merge job checks out the merged `main` commit for the close event,
runs the validation-grade live `basic-delivery` lane, and writes a Markdown
report with the temporary bridge. Both the direct run and the report record that
this is the checked-in scenario corpus, the `live` confidence tier, the manifest
topology, Forgejo URL, issue/PR numbers, CI evidence, convergence timing, fake
LLM request counts, and log/artifact paths.

```sh
cargo dev-scenario-check
cargo build --bin temper
cargo run -p temper-scenario-cli -- run \
  --tier live \
  --temper-bin target/debug/temper \
  scenarios/basic-delivery
cargo run -p temper-scenario-cli -- validate-pr \
  --pr <merged-pr-number> \
  --sha <merged-main-sha> \
  --scenario scenarios/basic-delivery \
  --tier live \
  --temper-bin target/debug/temper \
  --output-dir validation-artifacts/post-merge-pr-<merged-pr-number>
```

## Where to find the report in CI

When artifact upload is available on the Forgejo runner, the workflow uploads
the whole report directory as an artifact named:

```text
post-merge-validation-pr-<merged-pr-number>
```

Inside that artifact, the Markdown report uses this layout:

```text
validation-artifacts/post-merge-pr-<merged-pr-number>/
├── live-basic-delivery-artifacts/
│   ├── init.log
│   ├── repo-populate.log
│   ├── standalone.log
│   ├── fake-llm.log
│   └── ci-diagnostics.log
├── report-path.txt
├── validate-pr.stderr
└── validation-pr-<merged-pr-number>-<merged-main-sha>.md
```

The same directory also contains `report-path.txt`, which records the exact path
printed by `temper-scenario validate-pr`. The live artifact subdirectory is the
retained copy of the log paths cited by the report.

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
cargo dev-scenario-check
cargo build --bin temper
cargo run -p temper-scenario-cli -- run \
  --tier live \
  --temper-bin target/debug/temper \
  scenarios/basic-delivery
cargo run -p temper-scenario-cli -- validate-pr \
  --pr <merged-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --scenario scenarios/basic-delivery \
  --tier live \
  --temper-bin target/debug/temper \
  --output-dir /tmp/temper-validation/pr-<merged-pr-number>
```

Use the merged `main` SHA from the Forgejo run or PR page when reproducing an
older report. The temporary `validate-pr` bridge records the supplied PR
number and SHA; it does not fetch live Forgejo PR context or prove that the SHA
is still the current tip of `main`. For a quick local smoke test that does not
produce validation-grade evidence, run `cargo dev-scenario-run-hermetic`.

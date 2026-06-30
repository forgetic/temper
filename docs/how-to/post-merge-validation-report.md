# Find a post-merge validation report

Merged pull requests targeting `main` run the `post-merge-validation`
Forgejo Actions workflow. The workflow listens for `pull_request` `closed`
events and its job is guarded so it continues only when the pull request was
merged. Opening, synchronizing, or reopening a pull request still uses the
separate PR CI workflow in `.forgejo/workflows/ci.yml`.

The post-merge job checks out the merged `main` commit for the close event,
runs the deterministic `basic-delivery` validation lane, and writes a Markdown
report with the temporary bridge:

```sh
cargo run -p temper-scenario-cli -- check scenarios
cargo run -p temper-scenario-cli -- run scenarios/basic-delivery
cargo run -p temper-scenario-cli -- validate-pr \
  --pr <merged-pr-number> \
  --sha <merged-main-sha> \
  --scenario scenarios/basic-delivery \
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
└── validation-pr-<merged-pr-number>-<merged-main-sha>.md
```

The same directory also contains `report-path.txt`, which records the exact path
printed by `temper-scenario validate-pr`.

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
cargo run -p temper-scenario-cli -- check scenarios
cargo run -p temper-scenario-cli -- run scenarios/basic-delivery
cargo run -p temper-scenario-cli -- validate-pr \
  --pr <merged-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --scenario scenarios/basic-delivery \
  --output-dir /tmp/temper-validation/pr-<merged-pr-number>
```

Use the merged `main` SHA from the Forgejo run or PR page when reproducing an
older report. The temporary `validate-pr` bridge records the supplied PR
number and SHA; it does not fetch live Forgejo PR context or prove that the SHA
is still the current tip of `main`.

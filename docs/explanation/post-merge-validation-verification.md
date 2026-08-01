# Retired post-merge validation bridge report

This document used to record the first-pass validation bridge. That path has
been retired: `temper-scenario` no longer registers a `basic-delivery` runner,
no scenario dispatches by legacy `name`, and validation-grade scenario evidence
now comes from the single implicit topology declared by
`[runner] uses = "manifest"`.

Use the current workflow instead:

```sh
cargo run -p temper-scenario-cli -- check scenarios
cargo run -p temper-scenario-cli -- validate \
  --pr <merged-pr-number> \
  --sha <merged-main-sha> \
  --scenario scenarios/basic-delivery \
  --output-dir validation-artifacts/post-merge-pr-<merged-pr-number>
```

The manifest runner uses the one implicit scenario topology: real Forgejo, a
host `forgejo-runner` for CI, standalone Temper, and Jig fake-LLM responses. It
writes `run-evidence.json`, a Markdown validation report, and a structured JSON
validator result. MemoryForge, filesystem-forge, in-process, hermetic
real-stack, and simulation tests remain lower-level coverage and do not produce
this validation-grade scenario evidence.

For operator-facing details, see
[Find a post-merge validation report](../how-to/post-merge-validation-report.md).

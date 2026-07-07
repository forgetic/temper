# Retired post-merge validation bridge report

This document used to record the first-pass lower-confidence validation
bridge. That path has been retired: `temper-scenario` no longer registers a
`basic-delivery` runner, no scenario dispatches by legacy `name`, and
validation-grade scenario evidence now comes from one public runner only:
`[runner] uses = "manifest"` on the live stack.

Use the current workflow instead:

```sh
cargo run -p temper-scenario-cli -- check scenarios
cargo run -p temper-scenario-cli -- validate \
  --pr <merged-pr-number> \
  --sha <merged-main-sha> \
  --scenario scenarios/basic-delivery \
  --tier live \
  --output-dir validation-artifacts/post-merge-pr-<merged-pr-number>
```

The manifest runner boots real Forgejo, real host `forgejo-runner` CI, a real
Temper process, and Jig fake LLM responses. It writes `run-evidence.json`, a
Markdown validation report, and a structured JSON validator result. It rejects
hermetic, MemoryForge-only, in-process, and runner-name compatibility paths
instead of substituting lower-confidence evidence.

For operator-facing details, see
[Find a post-merge validation report](../how-to/post-merge-validation-report.md).

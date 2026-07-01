# Implementation PR handoff scenario

`implementation-pr-handoff` is a focused feature scenario for the #52/#53
engineer handoff behavior. It proves that a scripted coding-workspace success
can author the durable implementation PR title/body and that Temper preserves the
workflow metadata that relates the PR back to its source code issue.

Unlike `basic-delivery`, this scenario is not a full convergence run with CI and
mechanical merge. The runner is intentionally narrow: it drives the daemon
`ForgeApplier` path in process over `MemoryForge`, once for a newly-created PR
and once for an already-existing implementation PR with stale handoff text.

## Files

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

- `config/workflow.json` is the bundled workflow fixture used by the focused
  in-process proof.
- `config/source-issue.md` is the ready code issue body seeded by the runner.
- `config/create-handoff.md` is the authored report body expected on a newly
  opened implementation PR.
- `config/refresh-handoff.md` is the authored report body expected to replace a
  stale existing PR handoff.
- `repo/` is a minimal placeholder default-branch seed so manifest path checks
  remain self-contained.

## Expected flow

1. Create one in-memory repository, `acme/service`, with default branch `main`.
2. Seed a ready `code` issue and apply a scripted engineer success carrying
   `title = "Implement durable PR handoff"` and the create report body.
3. Expect Temper to open one implementation PR through the ForgeApplier workflow
   path, with the authored title/body and an `implementation_pr` metadata block
   whose parent is the source issue and whose correlation key is
   `pr-for-code-<issue>`.
4. Seed another ready `code` issue plus an existing implementation PR containing
   stale title/body but correct metadata.
5. Apply a second scripted engineer success carrying
   `title = "Implement refreshed handoff"` and the refresh report body.
6. Expect Temper to update the same PR rather than opening a duplicate, replace
   the stale handoff text, and preserve metadata kind, parent, and correlation.

## Running

```sh
cargo run -p temper-scenario-cli -- check scenarios/implementation-pr-handoff
cargo run -p temper-scenario-cli -- run --tier hermetic scenarios/implementation-pr-handoff
```

The `run` command prints the source classification (`checked-in scenario` for
this corpus bundle), the `hermetic` confidence tier, manifest topology, and then
concise evidence lines for the authored create handoff, the authored refresh
handoff, and the preserved workflow metadata/source issue relation. `--tier
live` is not implemented yet and fails instead of silently reusing this memory
runner.

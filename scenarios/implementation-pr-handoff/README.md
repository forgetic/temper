# Implementation PR handoff scenario

`implementation-pr-handoff` validates the engineer handoff behavior for
implementation PRs on the manifest runner's validation-grade live stack. It
proves that a Jig-scripted coding-workspace success can author the durable PR
title/body and that Temper preserves workflow metadata relating the PR back to
its source code issue.

The scenario uses real Forgejo, a registered host `forgejo-runner`, a real
standalone `temper` process, and Jig fake LLM responses. The checked-in runner
path is `runner.uses = "manifest"`; there is no hermetic MemoryForge or
in-process compatibility runner for this scenario.

## Files

```text
scenarios/implementation-pr-handoff/
├── scenario.toml
├── README.md
├── config/
│   ├── workflow.json
│   ├── ci.yml
│   ├── source-issue.md
│   ├── create-handoff.md
│   └── refresh-handoff.md
└── repo/
    ├── README.md
    └── .forgejo/workflows/ci.yml
```

- `config/workflow.json` is the workflow fixture loaded by `temper init`.
- `config/ci.yml` is copied into the repo seed so the live stack provisions
  Forgejo Actions/runner state; it is intentionally manual so the PRs remain
  open for handoff assertions.
- `config/source-issue.md` is the ready code issue body seeded for the create
  and refresh phases.
- `config/create-handoff.md` and `config/refresh-handoff.md` are the authored
  reports expected on the created and refreshed implementation PRs.

## Expected flow

1. Provision real Forgejo, register the host `forgejo-runner`, seed
   `acme/service`, configure Jig fake engineer responses, and launch real
   standalone Temper with JSON observability.
2. Seed a ready code issue. Temper assigns an engineer job, Jig returns
   `title = "Implement durable PR handoff"` and `config/create-handoff.md`, and
   Temper opens an implementation PR.
3. Seed a second ready code issue, let Temper claim the engineer job, then seed
   an existing stale implementation PR on the expected branch/correlation key.
   Jig returns `title = "Implement refreshed handoff"` and
   `config/refresh-handoff.md`, and Temper refreshes the same PR rather than
   opening a duplicate.
4. Manifest assertions check final PR state plus structured `pr.opened` and
   `pr.updated` events for authored title/body source, `metadata.kind =
   "implementation_pr"`, source parent refs, correlation keys, and
   `action = "created" | "refreshed"`.

## Running

```sh
cargo run -p temper-scenario-cli -- check scenarios/implementation-pr-handoff
cargo run -p temper-scenario-cli -- run --tier live --temper-bin target/debug/temper scenarios/implementation-pr-handoff
```

An explicit hermetic tier request is rejected by the manifest runner. Use
`cargo dev-scenario-run scenarios/implementation-pr-handoff` if you want the
helper to build and pass the standalone `temper` binary.

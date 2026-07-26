# Basic-delivery scenario

`basic-delivery` is the first declarative scenario in the checked-in corpus. It
captures the same happy path as `examples/basic-delivery/` without moving or
removing that example: a thin site-admin intake issue is triaged into ready code,
an engineer produces one implementation PR, Forgejo Actions CI passes, and the
mechanical worker lands the PR.

A validation-grade manifest runner exercises the topology described by the
manifest: real Forgejo, real host-mode `forgejo-runner` CI, a real standalone
`temper` process, and Jig fake LLM agents. The `manifest` runner is intentionally
live-only; MemoryForge, hermetic, and in-process Temper substitutes are rejected
so this scenario cannot be mistaken for lower-confidence validation.

## Files

```text
scenarios/basic-delivery/
├── scenario.toml
├── README.md
├── config/
│   ├── workflow.json
│   ├── ci.yml
│   └── intake-issue.md
├── jig/
│   └── basic-delivery.json
└── repo/
    ├── README.md
    └── .forgejo/workflows/ci.yml
```

- `config/workflow.json` is a copy of the basic-delivery workflow fixture used by
  the example.
- `config/ci.yml` is the CI workflow the example installs into the demo repo.
- `config/intake-issue.md` is the deliberately thin seed intake body.
- `jig/basic-delivery.json` owns every architect, engineer, and CI-repair fake
  response selected by the manifest's `jig.fake_llm` action.
- `repo/` is the minimal default-branch seed a runner can copy into `acme/service`
  before it files the intake issue. The CI workflow is present under its final
  repository path so the seed is self-contained.

## Expected flow

1. Create one Forgejo repository, `acme/service`, with default branch `main`.
2. Seed the repository from `repo/`.
3. Configure Temper with `config/workflow.json` and run the standalone topology
   described in `scenario.toml`.
4. After the webhook listener is ready, create the intake issue whose title and
   body are declared in the manifest.
5. Expect the architect to return `ready_code`, the engineer to open one
   implementation PR, CI to pass, and mechanical automation to merge the PR and
   close the parent issue.

## Running

```sh
cargo run -p temper-scenario-cli -- check scenarios/basic-delivery
cargo dev-scenario-run                      # live validation-grade manifest lane
```

Direct live invocation is also supported when the standalone binary is already
built or supplied by automation:

```sh
cargo build --bin temper
cargo run -p temper-scenario-cli -- run \
  --tier live \
  --temper-bin target/debug/temper \
  scenarios/basic-delivery
```

The live run output labels this bundle as `checked-in scenario`, reports the
`live` confidence tier and `runner.uses = "manifest"` selection, prints the
manifest topology, and then shows the Forgejo URL, issue/PR numbers, CI job
evidence, convergence timing, fake LLM request counts, structured Temper JSON
event log path, and other log/artifact paths. A copied bundle outside
`scenarios/` runs with the live tier too and is labeled `ephemeral validation
bundle`.

## Provenance

This scenario is seeded from `examples/basic-delivery/` as a corpus promotion.
The example remains the operator-facing launcher; this directory is the
validation input bundle. Future validation reports should reference this
scenario when they run this happy path, but the reports themselves remain the
required deliverables.

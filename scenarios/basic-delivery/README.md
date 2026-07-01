# Basic-delivery scenario

`basic-delivery` is the first declarative scenario in the checked-in corpus. It
captures the same happy path as `examples/basic-delivery/` without moving or
removing that example: a thin site-admin intake issue is triaged into ready code,
an engineer produces one implementation PR, Forgejo Actions CI passes, and the
mechanical worker lands the PR.

A validation-grade live runner now exercises the topology described by the
manifest: real Forgejo, host-mode `forgejo-runner`, standalone `temper`, and Jig
fake LLM agents. The fast hermetic runner remains available for local smoke
checks, but its memory/in-process evidence is lower confidence and is labeled as
such.

## Files

```text
scenarios/basic-delivery/
├── scenario.toml
├── README.md
├── config/
│   ├── workflow.json
│   ├── ci.yml
│   └── intake-issue.md
└── repo/
    ├── README.md
    └── .forgejo/workflows/ci.yml
```

- `config/workflow.json` is a copy of the basic-delivery workflow fixture used by
  the example.
- `config/ci.yml` is the CI workflow the example installs into the demo repo.
- `config/intake-issue.md` is the deliberately thin seed intake body.
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
cargo dev-scenario-run                      # live validation-grade lane
cargo dev-scenario-run-hermetic             # fast lower-confidence smoke lane
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
`live` confidence tier, prints the manifest topology, and then shows the Forgejo
URL, issue/PR numbers, CI job evidence, convergence timing, fake LLM request
counts, and log/artifact paths. A copied bundle outside `scenarios/` runs with
the live tier too and is labeled `ephemeral validation bundle`.

For the lower-confidence in-process runner:

```sh
cargo run -p temper-scenario-cli -- run --tier hermetic scenarios/basic-delivery
```

That hermetic output records the same source/topology labels plus memory-backed
evidence for the seeded issue, implementation PR, CI signal, and closed parent
issue; it must not be cited as live Forgejo validation.

## Provenance

This scenario is seeded from `examples/basic-delivery/` as a corpus promotion.
The example remains the operator-facing launcher; this directory is the
validation input bundle. Future validation reports should reference this
scenario when they run this happy path, but the reports themselves remain the
required deliverables.

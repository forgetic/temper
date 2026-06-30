# Basic-delivery scenario

`basic-delivery` is the first declarative scenario in the checked-in corpus. It
captures the same happy path as `examples/basic-delivery/` without moving or
removing that example: a thin site-admin intake issue is triaged into ready code,
an engineer produces one implementation PR, Forgejo Actions CI passes, and the
mechanical worker lands the PR.

No executable runner is defined here. The files are stable inputs for a future
checker/runner and for post-merge validation reports to cite by scenario name and
commit.

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

## Provenance

This scenario is seeded from `examples/basic-delivery/` as a corpus promotion.
The example remains the operator-facing launcher; this directory is the
validation input bundle. Future validation reports should reference this
scenario when they run this happy path, but the reports themselves remain the
required deliverables.

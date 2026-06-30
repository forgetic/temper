# Scenario corpus

`scenarios/` is Temper's checked-in corpus of declarative validation cases. A
scenario is a small, reviewable bundle that describes a workflow, seed forge
state, agent shape, expected outcome, and local fixture files. The corpus is for
stable regression inputs that future validation tooling can load without having
to reverse-engineer shell demos or test code.

This directory is intentionally separate from `examples/`. Examples remain
operator-facing demos with launch scripts and runtime instructions. Scenarios are
portable inputs for validation and post-merge regression; adding a scenario does
not migrate, deduplicate, or replace an example.

## Validation reports vs. promotion artifacts

Every post-merge validation run must produce a validation report: what target
(PR, issue, epic, or aggregate) and commit/PR set was validated, which scenario
or ad-hoc case was run, which topology was used, what commands or tooling ran,
where logs/artifacts live, and the final pass/fail result. That report is the
required deliverable for validation work.

Changing `scenarios/` is optional. A checked-in scenario change is a promotion
artifact: it captures a case that should become a reusable regression input after
it has proven useful. Not every validation report should add or update a
scenario, and a scenario should not be edited merely to make one validation
report pass.

## Authoring model

Author scenarios as data, not as runners:

- Keep the manifest in `scenario.toml` and keep all paths relative to the
  scenario directory.
- Store fixture inputs as ordinary files that can be copied into a throwaway
  environment by a future Rust checker or runner.
- Do not commit credentials, generated logs, runtime state, caches, or secrets.
- Prefer minimal repo seeds. A scenario should include only the default-branch
  files required to exercise the workflow.
- If a scenario is promoted from an example, copy only the stable fixture inputs
  needed by validation. Leave the source example unchanged.

## Manifest fields

The first-pass manifest shape is intentionally small and TOML-native so a later
Rust checker can parse it directly.

Top-level metadata:

| Field | Purpose |
| --- | --- |
| `schema` | Manifest schema identifier, currently `temper.scenario.v1`. |
| `name` | Stable scenario slug matching the directory name. |
| `status` | Lifecycle state such as `draft`, `active`, or `retired`. |
| `intent` | One-sentence behavior the scenario protects. |
| `introduced_by` | Issue or PR that introduced the scenario. |
| `owner_area` | Product area responsible for keeping the scenario useful. |
| `stability` | Compatibility promise, for example `provisional` or `stable`. |
| `timeout` | Human-readable wall-clock budget for a future runner. |

Required sections:

| Section | Purpose |
| --- | --- |
| `[topology]` | Runtime boundary being validated: forge, runner, Temper process shape, agent model, and repo set. |
| `[workflow]` | Workflow fixture name, format, and local path. |
| `[[repos]]` | Repositories to create, their default branches, optional seed directories, and CI file placement. |
| `[[issues]]` | Issues to seed, including title, author model, labels, and body file. |
| `[[agents]]` | Roles expected to service the scenario and the tool or automation mode they use. |
| `[expect]` | High-level convergence result plus machine-checkable expectation entries. |
| `[change_policy]` | Compatibility notes for future edits and whether a validation report is required. |

Scenarios may add explanatory keys inside those sections, but new keys should be
documented in the scenario README when they affect validation semantics.

## Assertion templates

`[expect]` may name stable assertion templates before every template has a full
runner implementation. Use `template = "<name>"` for one contract or
`templates = ["<name>", ...]` for several; `[[expect.checks]]` entries can remain
beside templates for explanatory or future machine checks.

The initial catalog accepted by `temper-scenario check` is:

- `single-pr-merged-source-closed` — one implementation PR merges and closes its source issue.
- `review-requested-then-approved` — a review request is made before approval unblocks landing.
- `ci-fails-then-passes` — a failing CI signal is followed by a passing replacement signal.
- `cross-repo-fanout-converges` — coordinated work fans out across repositories and converges.
- `no-duplicate-prs` — repeated progress signals do not create duplicate implementation PRs.
- `quiescent-after-merge` — no further workflow actions remain after successful merge convergence.
- `webhook-progress-before-poll-backstop` — webhook progress is observed before any polling backstop is needed.

Unknown template names are manifest validation errors so checked-in scenarios
refer only to cataloged behavior contracts.

## Expected layout

```text
scenarios/
├── README.md
└── <scenario-slug>/
    ├── scenario.toml
    ├── README.md
    ├── config/
    │   ├── workflow.json
    │   ├── ci.yml
    │   └── intake-issue.md
    └── repo/                  # optional default-branch seed
        ├── README.md
        └── .forgejo/workflows/ci.yml
```

Local path references in `scenario.toml` must point at files or directories in
the same scenario bundle. Prefer duplicating small fixture files over depending
on paths in `examples/` so validation can run from the checked-in corpus alone.

## Relationship to post-merge validation

Post-merge validation should cite a scenario by `name` and repo commit when
it uses this corpus. The [post-merge validator handoff][validator-handoff]
specifies the workflow-native architecture: it treats
`temper-scenario validate-pr` as a temporary/manual bridge, not the final
validator workflow. The validation report remains the required artifact; the
scenario is the reusable input that made the run reproducible. When an ad-hoc
post-merge run uncovers a useful regression shape, promote it by adding or
editing a scenario in a normal PR, with the validation report linked from the PR
or issue that justifies the promotion.

[validator-handoff]: ../docs/reference/post-merge-validator-handoff.md

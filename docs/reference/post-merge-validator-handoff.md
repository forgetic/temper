# Workflow-native post-merge validator handoff

This page is a design/schema slice for routing post-merge validation as normal
workflow work. It is not implemented yet. The current manual bridge,
`temper-scenario validate-pr`, remains temporary operator tooling and is not the
final architecture.

Validation firing policy is workflow-defined. Temper must not hard-code a global
rule such as "validate after every implementation PR merges." One workflow may
bind validation to each merged implementation PR; another may bind validation to
a parent issue, master-plan issue, `epic`, or any other workflow-defined artifact
kind after an aggregate deliverable becomes coherent and testable.

The target shape mirrors engineer and reviewer jobs: Temper prepares a bounded
context bundle, assigns one role/action job, accepts one structured result, and
applies workflow effects itself. The validator never receives a Forge token and
never mutates Forge directly.

## Validation bindings and lifecycle

A workflow can add a `validator` role and one or more validation bindings. A
binding connects workflow state to a concrete role/action handoff; examples are
`validate_merged_pr` for an `implementation_pr` target and `validate_epic` for an
aggregate `epic` target. A binding declares:

- `id`: a stable workflow-local validation binding id.
- `role` and `action`: the role id and action/transition id to assign, for
  example `validator` / `validate_merged_pr`.
- `target_artifact`: the artifact kind the validator is judging, such as
  `implementation_pr`, `issue`, `epic`, or another kind declared by the workflow
  spec.
- `trigger` and `readiness`: criteria expressed in workflow terms, such as
  labels, queue membership, gates, native merge/CI/review facts, parent/child
  relations, dependency completion, produced-PR relations, or an explicit
  validation-handoff label such as `validation-ready`.
- `target_selection`: how to choose the artifact under validation from the
  triggering fact, for example the merged PR itself, the parent issue of a merged
  child PR, or an epic reached through `parent` relations.
- `aggregation`: which related issues, PRs, commits, CI jobs, scenario results,
  and acceptance criteria belong to the target when the target is not a single
  PR.
- `idempotency_key`: a key template that includes the binding id, target kind,
  target id, and the target state or aggregate fingerprint that has already been
  validated.

A reference lifecycle is:

1. Forge or workflow state changes: for example an implementation PR merges to
   `main`, a child issue closes, dependencies all land, or a `validation-ready`
   label is applied.
2. A workflow transition or reconciler evaluates declared validation bindings.
   Criteria are read from the workflow spec, not from a global validator policy.
3. When a binding is ready, Temper selects the declared target and computes a
   durable idempotency key for the target state. If a validation work item or
   completed result already exists for that key, no duplicate handoff is created.
4. Temper creates validator work linked to the selected target and any source or
   aggregate artifacts. The work item carries a context-bundle pointer, not raw
   prompt prose.
5. The runner prepares a read-only workspace with the context bundle and any
   declared scenario inputs. It may also expose local scenario tools.
6. The validator inspects the bundle, reruns or checks suggested scenarios when
   useful, and returns a `ValidatorResult`.
7. Temper validates the result schema, stores or publishes the report, and
   applies follow-up effects through normal workflow transitions.

The validator role may coexist with existing post-merge architect and owner
queues. Architect reconciliation still updates plans or closes source issues;
owner alignment still batches holistic review; validator work answers whether
the workflow-selected target demonstrably satisfied its claims.

Bindings should avoid early validation when intermediate child PRs are not
independently meaningful. If a child PR only contributes one part of an aggregate
feature, the workflow should target the parent issue or epic and wait for
parent/child completion, dependencies, gates, or an explicit validation-ready
state instead of firing a per-child validation run.

## Illustrative workflow policies

The following fragments are illustrative design examples, not implemented config
syntax.

### Validate each merged implementation PR

This policy is appropriate when every implementation PR is independently
meaningful and can be validated against its linked issue after landing.

```json
{
  "id": "validate_each_merged_implementation_pr",
  "role": "validator",
  "action": "validate_merged_pr",
  "target_artifact": "implementation_pr",
  "trigger": {
    "kind": "native_merge",
    "branch": "main",
    "artifact": "implementation_pr"
  },
  "readiness": {
    "all": ["merged_to_default_branch", "ci_passed_at_merge"]
  },
  "target_selection": { "kind": "triggering_artifact" },
  "aggregation": { "include": ["source_issue", "produced_pr_relation"] },
  "idempotency_key": "validator:{binding_id}:pr:{pr_number}:main:{merged_main_sha}"
}
```

The context bundle for this policy has target kind `implementation_pr` and one
PR entry. Re-running the reconciler for the same merged SHA does not enqueue a
second validation job.

### Validate a parent, master-plan, or epic issue after aggregate readiness

This policy is appropriate when several child issues and PRs form one coherent
release or feature. The validator should judge the parent artifact once the
aggregate is ready, not each child PR as it lands.

```json
{
  "id": "validate_epic_when_ready",
  "role": "validator",
  "action": "validate_epic",
  "target_artifact": "epic",
  "trigger": {
    "any": [
      { "label_added": "validation-ready" },
      { "child_completion_changed": true }
    ]
  },
  "readiness": {
    "any": [
      { "labels": ["validation-ready"] },
      {
        "all_children": {
          "issues": "closed_or_workflow_done",
          "produced_prs": "merged_to_default_branch",
          "dependencies": "complete",
          "blocking_gates": "passed"
        }
      }
    ]
  },
  "target_selection": {
    "kind": "related_artifact",
    "relation": "parent",
    "artifact": "epic"
  },
  "aggregation": {
    "include": ["child_issues", "produced_prs", "diffs", "ci", "scenario_evidence"],
    "child_depth": 2
  },
  "idempotency_key": "validator:{binding_id}:epic:{issue_number}:state:{aggregate_fingerprint}"
}
```

The aggregate fingerprint should change when relevant child completion, produced
PR SHAs, target labels, or declared validation inputs change. That lets a
workflow intentionally revalidate a new target state without repeatedly firing
for the same ready aggregate.

## Validator context bundle

The bundle should be generated by Temper from Forge and repository facts. It must
identify the selected validation target and the binding that selected it; the
target is not always a PR. These fields are required for a first live
implementation and include the PR context fields planned by #35 without losing
PR-level detail when PRs are part of an aggregate:

- `schema`: stable id, for example `temper.validator.context.v1`.
- `target_repo`: repository owner/name and default branch under validation.
- `target`: target kind, target reference, title/body summary, URL, state,
  target labels, trigger reason, readiness facts, and target state fingerprint.
  The kind may be `implementation_pr`, `issue`, `epic`, or any other
  workflow-defined artifact kind.
- `validation_binding`: binding id, validator role id, action id, queue id,
  declared target artifact kind, trigger/readiness summary, aggregation rules,
  and idempotency key.
- `pull_requests`: PR number, title, body, author, URL, state, merge time,
  merged SHA, observed `main` SHA, source issue, produced-PR relation, and
  per-PR validation-relevant labels for every PR in scope. A per-PR validation
  has one entry; an epic validation may have several.
- `issues`: target issue, source code issues, parent design/epic links,
  dependency links, child completion state, closing keywords, acceptance
  criteria, and labels for every issue in scope.
- `aggregate`: when the target is a parent/master-plan/epic or other aggregate,
  include the child issue/PR inventory, completion rollup, remaining blockers,
  relevant commits or merged SHA set, and the rule that made the aggregate ready.
- `comments`: relevant issue and PR comments, with author, timestamp, body, and
  URL. Aggregate bundles should preserve which artifact each comment came from.
- `reviews`: review requests, review decisions, review bodies, and resolved or
  open discussion threads for PRs in scope.
- `diffs`: changed files, diffstat, notable paths, raw diff pointers, and concise
  summaries for each PR or aggregate diff range under validation.
- `ci`: job names, conclusions, head SHA, run URLs, and log or artifact pointers
  for each PR or aggregate validation run.
- `scenario_metadata`: existing scenario names, paths, status, stability,
  templates, and commit for scenarios related to the target or aggregate.
- `suggested_scenarios`: scenario names or ad-hoc cases to check, with rationale
  and expected signals.
- `workflow`: role/action id, queue id, artifact ids, labels, relationships,
  gates, trigger reason, and relevant acceptance criteria from the selected
  target and aggregate work items.

The bundle should prefer pointers for large bodies: CI log URLs, artifact paths,
raw diff paths, and discussion URLs are better than embedding unbounded text.
Generation is a product feature of the future handoff; this page does not
require live Forgejo extraction yet.

## Validator result schema

The result should be a machine-readable structure that can render the #55
Markdown validation report without losing fields. The #55 fields remain intact;
`target` is generalized so it can describe a PR or a non-PR aggregate, and PR
facts move into `related_prs` when the selected target is not itself a PR:

```json
{
  "schema": "temper.validator.result.v1",
  "target": {
    "kind": "epic",
    "repo": "ai/temper",
    "ref": { "issue_number": 35 },
    "trigger_reason": "validation-ready after all child issues completed",
    "state_fingerprint": "aggregate:abc123"
  },
  "related_prs": [
    {
      "pr_number": 60,
      "source_issue": 59,
      "merged_main_sha": "abc123"
    }
  ],
  "verdict": "passed",
  "validated_claims": [],
  "acceptance_criteria": [],
  "evidence": [],
  "limitations": [],
  "follow_up_issue": null,
  "scenario_promotion": null
}
```

For per-PR validation, `target.kind` is `implementation_pr` and `target.ref`
contains the PR number and merged/main SHA. For aggregate validation,
`related_prs[]` preserves PR-level merge and source-issue details while the
`target` remains the issue, epic, or workflow-defined artifact that was actually
validated.

`verdict` is one of `passed`, `failed`, or `inconclusive`.

`validated_claims[]` records claims the validator attempted to prove or observe.
Each entry has `description`, `status`, and `evidence_refs`. Status values
should preserve the #55 report vocabulary: `satisfied`, `observed`, `failed`,
`unproven`, and `not applicable`.

`acceptance_criteria[]` records observable criteria from the issue, PR, review,
or workflow bundle. It has the same `description`, `status`, and
`evidence_refs` shape as claims.

`evidence[]` records command results, scenario checks, scenario runs, artifacts,
and observations. Each entry has a stable `id`, `kind`, `summary`, optional
`details`, and optional `uri` or `artifact_path`. The initial kind vocabulary
should cover the #55 report kinds: `scenario_check`, `scenario_run`, `command`,
`artifact`, and `observation`. Aggregate validation can cite per-PR diffs, CI
rollups, scenario evidence, and child completion facts using those same evidence
entries.

`limitations[]` records what the validator could not prove, missing logs,
unsupported scenarios, flaky external systems, or context omissions.

`follow_up_issue` is optional intent for workflow-owned issue creation. It
contains `title`, `body`, `labels`, and optional relation hints. Failed or
inconclusive validation should either include this intent or explain in
`limitations` why no follow-up is useful.

`scenario_promotion` is optional intent to turn an ad-hoc validation case into a
checked-in scenario. It can include `scenario_name`, `intent`, source evidence,
fixture notes, and whether the validator proposes an issue or PR. This is only
intent; scenario promotion remains a separate workflow effect.

## Workflow effects

Temper, not the validator, applies effects after result validation:

- store the structured result and rendered Markdown report as a durable
  artifact;
- publish a report link or summary on the selected target, related PRs, source
  issues, or validation item according to the workflow binding;
- mark the validation item `passed`, `failed`, or `inconclusive` with workflow
  labels or state owned by the validation workflow;
- open a follow-up issue when a failed or inconclusive result supplies
  `follow_up_issue` intent, linking it to the selected target and relevant
  source or aggregate artifacts;
- optionally open a scenario-candidate issue or PR when `scenario_promotion`
  intent is present and the workflow declares that effect;
- leave Forge mutation idempotent, correlated, and auditable through the normal
  executor path.

A passed result normally closes the validation item without creating follow-up
work. A failed result should route repair work. An inconclusive result should
route investigation unless the limitations make human/operator action clearer.

## Temporary `validate-pr` bridge

`temper-scenario validate-pr` is a temporary/manual bridge introduced before the
workflow-native handoff. It accepts operator-supplied PR and SHA values, may run
local scenario checks, and writes a Markdown report using the #55 report model.
It does not fetch live Forgejo PR context, prove that the SHA is current `main`,
queue validator work, evaluate workflow-defined validation bindings, select
aggregate issue/epic targets, apply workflow effects, or promote scenarios.

The future validator may call the scenario checker or runner for local evidence,
but the CLI bridge should be treated as compatibility tooling. The final
architecture is the workflow role/action handoff described above: prepared
validation target context in, structured validator result out, workflow effects
applied by Temper.

# ADR 0022: Generalize role work into a sandboxed workspace with verdict routing

## Status

Accepted

## Context

At the time of this ADR, workflow roles were serviced by a shallow upfront
selector: the runner built a small work-item context (artifact
title/body/labels/state), asked a sandboxed, single-shot, **tool-less** process
for **one** authorized transition name, and applied that transition's fixed
effects. That selector did not read the repository, the diff, CI, or related
artifacts.

Two narrow exceptions hint at what is missing. `coding_workspace` is the lone
content-producing executor: it runs from the engineer's `open_pr` path and
yields a PR head. `on_merge_conflict` is the lone place a tool's *outcome*
selects a different transition: a hardcoded two-way branch on the `landing`
queue's merge effect.

So an informed, "fat" role — one that analyses with real tools, produces a work
product, and emits a typed outcome that selects the transition (including
escalation discovered mid-work) — cannot be expressed. The route is selected
*before* any analysis, the chosen transition cannot be re-selected by what the
work reveals, and only the engineer can run a content executor at all.

An earlier draft (issue #27) proposed a *family* of executor kinds —
`code_workspace`, `issue_workspace`, `review_workspace`. We reject that shape:
naming executors after roles/artifacts leaks workflow semantics into an engine
that must stay **role- and workflow-agnostic**. The thing to generalize is the
*workspace* itself.

## Decision

Add one role-agnostic primitive — the **workspace** — plus the two seams it
needs, all opt-in via workflow declaration. The engine learns no role names.

### A. The workspace executor (one primitive, capability-parameterized)

A workspace is a sandboxed agent session the runner can bind and invoke from an
action. It is **not** keyed to a role or artifact kind. It receives the
work-item context, is granted a declared set of **capabilities** (e.g. a repo
checkout with edit, read-only checkout, CI read, none), runs as an isolated
process/tool, and returns `{ verdict, work_product }`.

- It **never** holds a Forge token and **never** mutates Forge (ADR 0002
  boundary). The capabilities it is granted and the work-product slots it may
  fill are workflow declaration, not engine knowledge.
- `coding_workspace` becomes one configuration of this primitive (a
  write-in-checkout workspace whose work product is a PR head). The
  `ExternalToolExecutors` registry gains generic registration/lookup keyed by
  executor id, replacing the `coding_workspace`-only methods, and dispatch stops
  being gated on "creates a pull request".

### B. Verdict → transition routing (generalize `on_merge_conflict`)

A workspace-backed action declares an `outcomes` map from **workflow-declared
verdict ids** to transitions. The engine treats verdict ids as opaque tokens; it
only validates, at compile time, that each declared verdict maps to a transition
that is legal for the action's artifact/role. At runtime the workspace returns a
verdict, the engine selects *that* transition, and applies its effects
transactionally (leases/idempotency unchanged). `on_merge_conflict` becomes the
first instance of this mechanism (kept as sugar over it).

```jsonc
{ "id": "triage_intake", "artifact": "issue", "roles": ["architect"],
  "executor": "workspace",
  "outcomes": {                       // verdict ids are workflow vocabulary
    "ready_code":      "triage_to_code",     // engine sees opaque ids only
    "needs_design":    "triage_to_design",
    "needs_breakdown": "break_into_children"
  } }
```

### C. Content-bearing / multi-artifact effects with a runtime seam

Add role-agnostic effect kinds that consume the workspace work product through a
keyed runtime-input seam, exactly as `create_pull_request` reads its head from
the coder today (`run_with_pull_request_create_at` + correlation key):

- `set_body` — write an agent-authored body onto the current artifact.
- `create_issues` — create one-or-many child artifacts with authored
  titles/bodies/labels and declared parent/dependency relations (ADR 0011/0015),
  idempotent via correlation keys; the principled, in-workflow form of fan-out.
- `attach_review` — submit a native review (ADR 0016) carrying the work
  product's review body/comments, not just a bare decision.

### D. Workspaces on assigned role jobs and automation paths

Workspaces are bindable from queue automation and from concrete role/action jobs.
A workflow may assign a workspace-backed action directly and let its verdict
route — dropping the uninformed upfront classification where it adds nothing.
This is independent of A–C.

The agent still only ever returns `{ verdict, work_product }`; the engine owns
transition legality and effect application. The workspace is precisely the
bounded execution context where work happens, opposite the engine where
authority lives — that split is the reason the primitive exists.

## Consequences

- Engineer, architect, and reviewer become the *same* shape — "analyse with
  granted tools → produce a work product → return a verdict" — and differ only
  in declared capabilities, work-product slots, verdict vocabulary, and
  `outcomes`/effects. The engine hardcodes none of it.
- The target intake-to-merge flow is expressible purely by declaration: a
  default/catch-all issue artifact kind admits newly filed unlabeled human
  issues; a mechanical bot labels them `untriaged`; an architect
  `triage_intake` workspace routes `ready_code` (rewrite body via `set_body` →
  code+ready), `needs_design` (design proposal body → needs-owner), or
  `needs_breakdown` (`create_issues` for dependent children targeting
  engineers); an engineer workspace routes `implemented → open_pr` or
  `needs_architect`; a reviewer workspace reads the diff/CI and routes
  `approve` / `changes` (+`attach_review`) / `escalate`; the landing queue
  merges when gates are green.
- The current prompt-level workarounds (reviewer told to approve from the PR
  summary; engineer told `open_pr` is what produces the diff) are removed: the
  reviewer reads the real diff and the verdict carries the judgment.
- Authority/sandbox boundary, leases, journals, and idempotency are unchanged;
  no mega-agent that both edits code and writes Forge state.
- Migration is staged (see issue #27): first add the default/catch-all intake
  artifact-kind support needed for unlabeled human issues, then generalize the
  executor binding; add the verdict + routing (subsumes #26); add
  content/multi-artifact effects; optional automation-path workspaces; then
  re-express reference-delivery on the primitives.

## Alternatives considered

- **A family of role-named executor kinds (`issue_workspace`, `review_workspace`).**
  Rejected: it encodes role/artifact semantics in a role-agnostic engine. The
  same capabilities are expressed by declaring one workspace's tools and outputs.
- **A mega-agent that edits content and mutates Forge directly.** Rejected: it
  dissolves the ADR 0002 authority boundary. Routing a verdict through the engine
  keeps execution and authority separate.
- **Keep the prompt workarounds.** Rejected: they hide a core gap in role prose,
  make the reviewer approve work it never read, and do not generalize.

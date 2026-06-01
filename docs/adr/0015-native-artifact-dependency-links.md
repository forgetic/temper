# ADR 0015: Promote dependency links to native Forge state

## Status

Accepted

## Context

ADR 0011 made workflow relations first-class in the workflow spec, but every
concrete link was still projected through the artifact metadata block. That was
sufficient to validate relation declarations and drive `dependencies_resolved`,
but it treated dependencies as workflow-owned metadata instead of Forge-owned
collaboration state.

The native-Forge-state roadmap Phase B calls for promoting the portable
intersection both target providers support: one issue or pull request can be
blocked by one or more other repository items. GitHub exposes richer hierarchy
features such as sub-issues / parent-child links, while Forgejo has no native
parent-child equivalent, so only `depends_on` is portable.

## Decision

Add a native dependency-link concept to `harness-forge`:

- the source is an issue or pull request, identified by its stable `IssueId` or
  `PullRequestId`;
- the target is an `ItemNumber` in the same repository as the source;
- a source may depend on multiple target item numbers;
- add and remove operations are set-like and idempotent;
- artifact records expose their current dependency item numbers in a
  deterministic list.

The Forge trait exposes source-specific operations:
`add_issue_dependency`, `remove_issue_dependency`,
`add_pull_request_dependency`, and `remove_pull_request_dependency`. The read
path is a field on `Issue` and `PullRequest` (`dependencies: Vec<ItemNumber>`),
so list/get calls return dependencies with the artifact record.

Adding a link requires the source to exist and the target item number to exist in
the same repository as either an issue or pull request. Removing a missing link
is a no-op once the source exists. A link change advances the source artifact's
version and timestamp; an idempotent no-op returns the current record unchanged.

Native dependency links are now the source of truth for same-repository
`dependency` relations. The metadata `dependencies` field remains a
compatibility fallback for older artifacts, backends with no native links, and
cross-repository targets represented as repo-qualified `ArtifactRef` values. When
same-repository native dependencies are present on an artifact, the classifier
ignores same-repository metadata dependency fallbacks but preserves explicit
repo-qualified metadata targets that native same-repository links cannot express.

`parent` and `produced_pr` remain metadata-projected relation kinds under ADR
0011. Forgejo has no native parent/child link, and GitHub sub-issues are a
richer non-portable superset, so they are out of scope for the portable Forge
interface.

## Consequences

- Workflows observe dependency links as Forge-owned state, while the workflow
  spec still declares which artifact kinds may participate in `dependency`
  relations.
- `dependencies_resolved` keeps the pure-planner shape: the planner reads a
  `DependencyStatus` signal, while runtime layers derive that signal from fresh
  Forge state. Issue targets are landed when closed; pull-request targets are
  landed when merged. Repo-qualified metadata fallback targets are read in their
  own repository. Missing or temporarily unreadable targets remain not landed.
- The reference backends persist dependency links on issue and pull-request
  records and share the same ordering, idempotency, target-existence, and version
  semantics (ADR 0008).
- Existing metadata-only artifacts continue to classify because metadata
  `dependencies` is still read when no native dependency links are present.
- The portable Forge dependency-link trait remains same-repository in this ADR;
  cross-repository native links can be promoted later with a separate trait
  change if a portable backend intersection proves out.

## Alternatives considered

- **Keep all relations metadata-only.** Rejected: dependencies are native state
  in both target providers and should not drift from a workflow-owned metadata
  projection.
- **Promote every relation kind.** Rejected: `parent` and `produced_pr` do not
  have a common provider-native shape. GitHub sub-issues would leak a
  provider-specific hierarchy concept into the portable interface.
- **Expose only a list-dependencies method.** Rejected for the reference model:
  storing the dependency list on the artifact record lets classification consume
  one already-loaded artifact without an extra backend call.

# ADR 0021: Use repo-qualified artifact references for workflow links

## Status

Accepted.

## Context

Workflow relations originally stored linked artifacts as a bare `ItemNumber`.
That was sufficient while every relation target lived in the same repository as
its source: the ambient `repo_id` from the worker scan supplied the missing
scope. Cross-repo workflows need a parent, dependency, or produced-PR link to
point at an item in another repository without encoding provider-specific URLs
or paths.

The Forge trait already scopes item lookup by `(RepositoryId, ItemNumber)`, but
its native dependency-link operations deliberately model the current portable
provider intersection from ADR 0015: same-repository dependency links. Extending
that trait before we have cross-provider evidence would overfit Phase 1.

## Decision

Add `artifact::ArtifactRef` in `harness-workflow` as the workflow-layer link
reference. Its portable fully resolved shape is:

- `RepositoryId` — the repository containing the linked item;
- `ItemNumber` — the issue or pull-request number inside that repository.

The Rust type keeps `repository_id: Option<RepositoryId>` so existing metadata
and native dependency reads can use a same-repository shorthand. `None` means
"resolve against the source artifact's repository"; `Some(repo_id)` is an
explicit repo-qualified target. Constructors make both cases clear:
`ArtifactRef::same_repo(number)` and `ArtifactRef::in_repo(repo_id, number)`.

`ArtifactRef` lives in `harness-workflow`, not `harness-forge`, because it is a
workflow relation/reference model. The Forge domain model continues to expose
provider-owned native dependency links as same-repository `ItemNumber`s per ADR
0015, and no `Forge` trait method changes in this phase.

Metadata relation fields (`parents` and fallback `dependencies`) accept both the
old bare-number representation and an object representation:

```json
{ "repository_id": "repo-123", "number": 34 }
```

Rendering preserves the compatibility rule: same-repository refs serialize as
bare numbers; explicit refs serialize as objects. Classification converts native
dependency item numbers into same-repository `ArtifactRef`s and preserves
metadata repo qualifications in `ClassifiedRelation.target`.

Dependency gate planning now compares repo-qualified targets in its
`DependencyStatus` set. Runtime dependency resolution initially resolved only
same-repository targets through the ambient repo; the later cross-repo aggregation
phase now resolves each explicit target by reading that target's repository
without changing the Forge trait.

## Consequences

- Existing single-repo metadata and native dependency links continue to parse and
  classify as same-repository references.
- A bare item number and an explicit cross-repository reference with the same
  number are distinct dependency targets.
- Cross-repo links are representable independently from provider-native link
  features.
- The Forge trait stays unchanged; future phases can add provider-backed
  cross-repo dependency operations only if a portable need is proven.

## Alternatives considered

- **Move the type to `harness-forge`.** Rejected for this phase: native Forge
  artifacts still expose same-repository dependency numbers, while this type is
  specifically the workflow relation projection.
- **Require every reference to serialize with a repository id immediately.**
  Rejected: it would churn existing metadata and tests without changing behavior.
- **Store provider URLs.** Rejected: URLs are not portable identities and would
  leak backend-specific routing into workflow logic.

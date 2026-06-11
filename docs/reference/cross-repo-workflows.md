# Cross-repository workflow contracts

This page records the portable contracts for cross-repository planning. It is a
reference page: for rationale and operator mental model, see
`docs/explanation/cross-repo-workflows.md`.

## Artifact references

A workflow link target is an `ArtifactRef`:

- `repository_id: Option<RepositoryId>`
- `number: ItemNumber`

`repository_id = None` is the same-repository shorthand and resolves against the
artifact carrying the link. `Some(repo)` is an explicit target repository. JSON
metadata accepts both forms:

```json
12
```

```json
{ "repository_id": "forgejo:acme/service-canary", "number": 12 }
```

Classifiers preserve the distinction. A same-repo `#12` and an explicit
cross-repo `repo-x#12` are different dependency targets.

## Global child correlation keys

Architect fan-out creates child issues idempotently with
`global_child_correlation_key(parent_repo, parent_number, child_slug)`.

The canonical key is:

```text
parent-repo:<repo-len>:<repo-id>#parent:<number>/child:<slug-len>:<slug>
```

Requirements:

- `parent_repo` is the source parent artifact repository id.
- `parent_number` is the parent issue's repository-local item number.
- `child_slug` is a stable child intent chosen by the architect; it must not
  contain timestamps, random data, or retry counters.
- Repository ids and slugs are length-prefixed, so delimiters inside either value
  do not collide.
- The key is global across repositories and target repos. Re-running fan-out with
  the same parent and slug must find or repair the existing child issue instead
  of creating another.

The child issue body carries both this correlation key and a repo-qualified
`parents` reference back to the source parent.

The `create_issues` effect path uses the same contract when a
`CreateIssuesChild` names `target_repo`: the child is ensured in that repository
with a global child key and repo-qualified parent backref. A fan-out containing
any cross-repository child also records repo-qualified dependency refs for every
child on the parent issue so dependency aggregation observes the same graph as
scripted architect fan-out. Children without `target_repo` keep the legacy
same-repository key/backref shape unless the parent dependency list must qualify
them because a sibling crossed repositories.

## Relation and dependency semantics

`parent` and `produced_pr` relations are metadata-projected. `dependency`
relations use native Forge dependency links for same-repository dependencies when
available, plus metadata fallback for compatibility and for explicit
cross-repository targets.

Dependency aggregation resolves each `ArtifactRef` in its own repository:

- issue targets are landed when the issue is closed;
- pull-request targets are landed when the pull request is merged;
- unreadable or missing target repositories are recorded as read failures and
  treated as not landed for that scan;
- the parent unblocks only when every dependency target is landed.

The planner remains pure. Runtime readers reduce fresh Forge state into
`DependencyStatus`, and the planner only tests set membership. The portable
native dependency-link operations remain same-repository; cross-repo dependencies
are represented by metadata `ArtifactRef` values unless a future ADR changes the
Forge contract.

## Authority boundary

`RoleTools::ensure_issue_in_repo` is the agent-facing fan-out tool. It takes an
explicit target `RepositoryId`, checks that the Forge handle can see that repo,
and relies on the Forge backend to authorize creation there. A worker's
configured scan shard is not write authority; the token's Forge permissions are.

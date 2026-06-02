# ADR 0011: Promote workflow relations to first-class spec declarations

## Status

Accepted; superseded in part by ADR 0015 for native `dependency` links. The
`parent` and `produced_pr` relation kinds remain metadata-projected under this
ADR.

## Context

The reference delivery workflow needs links between artifacts: design issues
belong to epics, code issues belong to designs or epics, code issues can depend
on other code issues, and implementation PRs are produced for code issues.

Before this decision, the only durable representation was metadata in artifact
bodies (`parents` and `dependencies`). That let a classifier preserve item
numbers, but the workflow spec could not declare which relation kinds were valid
between which artifact kinds. As a result, validation could not catch dangling
artifact-kind endpoint references, and the later `dependency_gate` would have no
spec-level relation contract to evaluate.

## Decision

Add a `relation` spec primitive. Each declaration has:

- a fixed `kind` enum: `parent`, `dependency`, or `produced_pr`;
- a `source` artifact kind, whose artifact body carries the metadata projection;
- a `target` artifact kind, identified at runtime by a Forge item number.

Validation rejects relation declarations whose source or target artifact kind is
undeclared. The validated model stores typed artifact-kind ids and the relation
kind enum.

Metadata remains the on-artifact projection that classifiers read:

- `parents` stores repository item numbers for declared `parent` links;
- `dependencies` stores repository item numbers for declared `dependency` links;
- `produced_pr` is projected through the same parent-number field on an
  implementation PR, because the existing metadata format only has parent and
  dependency number lists.

A classifier combines metadata numbers with the validated relation declarations
for the classified artifact kind and surfaces typed relations. The linked item
number is known immediately; the target artifact kind is the set of declared
allowed targets because the metadata number alone does not contain the linked
artifact's kind.

## Consequences

- Workflows can declare and validate relation contracts before runtime.
- The reference fixture can express parent, dependency, and produced-PR links
  without changing `temper-forge`.
- Existing metadata blocks remain compatible. ADR 0015 later promotes the
  portable `dependency` subset to the Forge interface while preserving metadata
  `dependencies` as a fallback.
- `dependency_gate` landed as Phase 12b: the `dependencies_resolved` gate
  condition evaluates the declared `dependency` relations against
  `DependencyStatus`, and the reconciler mechanically unblocks
  `blocked` work once every prerequisite lands. ADR 0015 later
  made runtime layers derive that status from native Forge dependency links.

## Alternatives considered

Keep metadata-only relations. Rejected because the spec would still have no
validated relation contract.

Add provider-level relations to `temper-forge`. Rejected for this phase because
relation semantics were workflow policy. ADR 0015 later accepts the narrower
portable dependency-link subset; GitHub sub-issues / parent-child hierarchy stay
out of scope because Forgejo has no native equivalent.

Add a new typed metadata relation array. Deferred because `parents` and
`dependencies` already exist in artifacts and are sufficient for Phase 12a; a
future metadata version can add exact per-link target kinds if needed.

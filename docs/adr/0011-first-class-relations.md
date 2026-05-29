# ADR 0011: Promote workflow relations to first-class spec declarations

## Status

Accepted

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
  without changing `harness-forge`.
- Existing metadata blocks remain compatible; no provider-specific relation API
  is added.
- `dependency_gate` remains a separate Phase 12b change that will evaluate the
  declared `dependency` relations against fresh Forge state.

## Alternatives considered

Keep metadata-only relations. Rejected because the spec would still have no
validated relation contract.

Add provider-level relations to `harness-forge`. Rejected because relation
semantics are workflow policy, while Forge backends only need to preserve item
numbers and bodies portably.

Add a new typed metadata relation array. Deferred because `parents` and
`dependencies` already exist in artifacts and are sufficient for Phase 12a; a
future metadata version can add exact per-link target kinds if needed.

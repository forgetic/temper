# Architecture decision records

ADRs capture significant decisions that should remain visible to future agents.

Current ADRs:

- [ADR 0002: Define a backend-agnostic Forge interface](0002-backend-agnostic-forge-interface.md)
- [ADR 0003: Separate Forge abstractions from the workflow crate](0003-separate-forge-and-core-crates.md)
- [ADR 0007: Define the workflow layer and agent compilation boundary](0007-workflow-layer-and-agent-compilation.md)
- [ADR 0008: Add an in-memory Forge backend and name backends by provider](0008-in-memory-backend-and-backend-naming.md)
- [ADR 0009: Webhook-accelerated, poll-backstopped triggering off the Forge trait](0009-triggering-model-webhook-accelerated-poll-backstopped.md)
- [ADR 0010: Model external-signal gates as Forge-projected conditions](0010-external-signal-gates.md)
- [ADR 0011: Promote workflow relations to first-class spec declarations](0011-first-class-relations.md)
- [ADR 0012: Extend queues with activation and richer matching](0012-queue-primitive-extensions.md)
- [ADR 0013: Portable optimistic concurrency for conditional artifact writes](0013-portable-optimistic-concurrency.md)
- [ADR 0014: Derive merge eligibility from gates and read native CI](0014-derive-merge-eligibility-and-native-ci-gate.md)
- [ADR 0015: Promote dependency links to native Forge state](0015-native-artifact-dependency-links.md)
- [ADR 0016: Model native pull-request reviews portably](0016-native-pull-request-reviews.md)
- [ADR 0017: Retire testing labels in favor of native CI status](0017-retire-testing-labels-for-native-ci-status.md)
- [ADR 0018: Filesystem backend cross-process concurrency via advisory locking](0018-filesystem-cross-process-concurrency.md)
- [ADR 0019: Read Forgejo CI status through the password-authenticated web UI](0019-forgejo-ci-read-via-web-ui.md)
- [ADR 0021: Use repo-qualified artifact references for workflow links](0021-repo-qualified-artifact-references.md)
- [ADR 0022: Generalize role work into a sandboxed workspace with verdict routing](0022-workspace-executor-and-verdict-routing.md)
- [ADR 0023: Multi-repo co-development jobs](0023-multi-repo-co-development-jobs.md)

# Exact-head feature validation

This active scenario is the sole checked-in mapping for feature
`ai/temper#778`, planned by `ai/temper#779`, on
`feature/778-exact-head-validation`.

## Claim → stimulus → observable → assertion

- **Claim:** Only passing evidence for the mapped scenario and current feature
  head can authorize landing.
- **Stimulus:** Complete one delivery through the inherited isolated live stack,
  then submit a deliberately stale head to the focused validator entry point.
- **Observable:** Structured Forgejo, host runner, standalone Temper, local Jig,
  mapping digest, checkout-head, and retained stale-rejection facts.
- **Assertion:** The current mapped run passes and the stale head is rejected
  before scenario execution or landing authority.
- **Runtime budget:** 600 seconds.

The bundle inherits the production-shaped `basic-delivery` workflow and fixture
inputs while owning its Jig script and exact-head assertion hook. The inherited
run provisions real Forgejo and `forgejo-runner`, launches the feature-built
standalone Temper binary, and uses only generic manifest actions. The hook runs
after convergence, first checks that run evidence contains the current checkout
head and canonical mapping digest, then invokes `validate-feature` with a stale
SHA and requires its retained `evidence would be stale` rejection.

Runtime evidence is written only to the caller-provided artifact directory and
must not be committed.

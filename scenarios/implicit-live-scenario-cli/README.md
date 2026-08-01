# Implicit-live scenario CLI

This active scenario is the sole checked-in mapping for feature
`ai/temper#824`, planned by `ai/temper#825`, on
`agent/pr-for-feature-824`.

## Claim → stimulus → observable → assertion

- **Claim:** Scenario execution is implicitly live. The public runner commands
  have no tier selector, fixed live evidence stays explicit, and one manual
  scenario-run alias reaches the manifest topology.
- **Stimulus:** Complete one delivery through the inherited manifest stack
  without a tier argument, then probe command help, legacy tier spellings, and
  repository alias declarations after convergence.
- **Observable:** Structured mapping, standalone-binary, real-topology, Jig,
  observability, assertion, artifact, CLI usage-error, and single-alias facts.
- **Assertion:** The outer no-tier run converges through real Forgejo, the host
  `forgejo-runner`, standalone Temper, and the local Jig; `run`, `validate`, and
  `validate-pr` help omit the removed option; all three legacy forms are usage
  errors before execution; and only the unsuffixed Cargo/internal command
  remains.
- **Runtime budget:** 600 seconds.

The bundle inherits the production-shaped `basic-delivery` workflow, repository,
and CI fixtures while owning its Jig script and focused after-convergence hook.
The successful outer mapped run is the no-tier execution proof: the hook does
not start a nested Forgejo or Temper stack. It invokes only the feature-built
`temper-scenario` binary's help and argument parser, checks the completed outer
run evidence, and inspects the two repository files that define the manual
alias.

Run the sole manual alias where the live validation environment is available:

```sh
cargo dev-scenario-run scenarios/implicit-live-scenario-cli
```

All hook output, parser probes, run evidence, logs, and audit artifacts remain in
the caller-supplied artifact directory. No generated runtime evidence belongs
in this checked-in bundle.

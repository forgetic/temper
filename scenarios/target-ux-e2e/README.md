# Target UX e2e validation bundle

This bundle is the checked-in regression input for the target-era operator UX.
It inherits the live `basic-delivery` manifest runner fixture so
`temper-scenario run` remains on the existing Forgejo/Actions/Jig fake-LLM
convention, and it adds hermetic fixture bundles that root integration tests
exercise without live provider credentials.

The companion `tests/target_ux_e2e.rs` test covers:

- `temper init` writing a local bundle from a JSON workflow and `temper check`
  reporting the expected pre-apply credential gap.
- the apply/provisioning seam updating credentials and preserving the selected
  workflow/webhook inputs.
- offline and fake-online `temper check` behavior for a distributed YAML
  workflow bundle.
- worker-pool role auth, capacity limits, selected agent profile credentials,
  and serve-time guardrails.
- the selected trigger surface: hidden `temper trigger-forgejo` remains the
  runnable trigger contract while `temper serve trigger` is explicitly rejected.

All tokens and passwords in `config/*.toml` are fake fixture values.

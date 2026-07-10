# Target UX e2e validation bundle

This bundle is the checked-in regression input for the target-era operator UX.
It inherits the live `basic-delivery` manifest runner fixture so
`temper-scenario run` remains on the existing Forgejo/Actions/Jig fake-LLM
convention, and it adds hermetic fixture bundles that root integration tests
exercise without live provider credentials.

## Forgejo webhook intake

Operators should send HMAC-signed Forgejo events to `POST /forgejo/webhook` on
a configured `temper serve engine` or `temper serve standalone` runtime. Set the
shared signing secret with `[engine] webhook_secret` or
`[engine] webhook_secret_file`. Webhooks accelerate reaction to Forgejo events;
periodic polling remains the correctness backstop if a delivery is delayed or
lost.

`temper trigger-forgejo` is a legacy/internal adapter command retained only for
focused adapter compatibility test coverage. Operator deployments should use
the webhook endpoint on one of the supported serve runtimes above.

The companion `tests/target_ux_e2e.rs` test covers:

- `temper init` writing a local bundle from a JSON workflow and `temper check`
  reporting the expected pre-apply credential gap.
- the apply/provisioning seam updating credentials and preserving the workflow
  and webhook inputs.
- offline and fake-online `temper check` behavior for a distributed YAML
  workflow bundle.
- worker-pool role auth, capacity limits, selected agent profile credentials,
  and serve-time guardrails.
- the checked-in signed payload receiving `202` from an in-process engine
  webhook route while the contract identifies both supported runtime surfaces,
  and `temper serve trigger` stays explicitly rejected with actionable guidance.
- legacy/internal `temper trigger-forgejo` adapter dispatch compatibility in a
  separate, narrowly scoped test.

All tokens and passwords in `config/*.toml` are fake fixture values.

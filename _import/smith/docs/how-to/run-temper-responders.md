# Run anvil responders from Temper

Anvil (the sibling agent repo) implements Temper's provider-neutral process
protocols. Temper still owns validation, transcript/proposal storage, workflow
authority, and all Forge mutation.

## Workflow-role decision responder

Build anvil's role responder (from `../anvil`):

```sh
cargo build --bin anvil-workflow-role-decision
```

Point Temper role workers at it:

```sh
export TEMPER_WORKER_ROLE_DECISION_COMMAND=$PWD/../anvil/target/debug/anvil-workflow-role-decision
export TEMPER_WORKER_ROLE_DECISION_ARGS_JSON='["--auth","chatgpt-oauth"]'
```

The binary reads one `WorkflowRoleDecisionRequest` JSON value on stdin and writes
one `WorkflowRoleDecisionReply` JSON value on stdout. It receives no Forge
handle, Forge token, SDK bash/file tools, or workflow mutation tools.

## Product-manager example interaction responder

```sh
cargo build --bin anvil-product-manager-responder
```

(from `../anvil`)

The binary reads one Temper `ConversationRequest` JSON value on stdin and writes
one `ConversationReply` JSON value on stdout. It serves only the
`product-manager` example profile and returns reply text plus inert issue-draft
proposals. External frontends should call Temper's generic `temper-interaction`
REPL or HTTP service, not this responder directly.

Bind the responder through the interaction deployment binding file that Temper
loads with the user-defined profile spec (fragment shown):

```json
{
  "responders": {
    "product-manager-responder": {
      "command": "/path/to/anvil/target/debug/anvil-product-manager-responder",
      "args": ["--auth", "chatgpt-oauth"],
      "env_allowlist": [],
      "timeout_secs": 120
    }
  }
}
```

Then launch Temper, for example:

```sh
temper-interaction repl \
  --spec /path/to/product-manager.json \
  --bindings /path/to/interaction-bindings.json \
  --profile product-manager
```

Temper dogfood's `./run.sh product-chat` command generates the generic
interaction binding and points it at this anvil binary by default.

## Provider args and env

Pass provider options through the role responder `*_ARGS_JSON` variables
or the interaction binding file's `responders.<id>.args` array. Use env
allow-lists only for provider names anvil documents, such as
`TEMPER_DEEPSEEK_API_KEY` or `TEMPER_AGENTS_ANTHROPIC_MODEL`.

Protocol details live in Temper's `docs/reference/` directory; the responder
contract summary is `docs/reference/process-responders.md`, and the decision
events/captures are documented in
`docs/reference/workflow-role-observability.md`.

## Reference-delivery note

`examples/reference-delivery` no longer binds this role-decision responder. The
current demo uses `smith-worker --executor coding`; provider/auth options flow
through `ANVIL_AGENT_ARGS`, and runtime evidence is documented in
`examples/reference-delivery/observability.md`. Use the capture variables for
older role-worker deployments that still invoke the workflow-role-decision
responder, not for the daemon/worker reference-delivery launcher.

# Interactive process responder protocol

`temper-process-protocol` owns the serialization-oriented
`ConversationRequest`/`ConversationReply` DTOs, deterministic id helpers,
proposal DTOs, and proposal validation helpers. `temper-interaction` consumes and
re-exports those DTOs while owning the provider-neutral process adapter and
Forge-backed transcript/acceptance runtime. The protocol crate has no pi SDK,
provider-auth, Forgejo, production, workflow, runner, or Forge dependency. The
current JSON shape is the v1 wire contract and is frozen by the fixtures below.

## Invocation

Deployments select external responder processes explicitly in deployment
bindings, keyed by the responder ids declared in the compiled interaction spec.
The generic `temper-interaction` binary reads responder `command`, `args`, `cwd`,
`env_allowlist`, and `timeout_secs` from that binding file; Forge tokens and
service auth are read through configured environment-variable names. Concrete
profile implementations are external processes selected by deployment config.
Temper does not ship in-process LLM fallbacks for any interactive profile.

Frontends must not invoke the responder process directly. They call Temper's
interaction service (`/conversations`, Matrix adapter, mobile adapter, etc.);
Temper owns transcripts, validation, proposal acceptance, and all Forge/workflow
mutation.

`ProcessResponder` is configured with:

- program path and argument vector;
- optional working directory;
- environment-variable allow-list copied from Temper's process;
- one-turn timeout.

The child environment is cleared before allow-listed variables are copied. Do
not pass Forge tokens, Forge handles, workflow tools, or broad ambient env to the
responder. If an external LLM profile needs credentials, expose only the specific
provider variables it requires through the allow-list.

## Wire format

Temper writes one UTF-8 JSON `ConversationRequest` to stdin, appends a newline,
and closes stdin. The responder writes exactly one UTF-8 JSON
`ConversationReply` to stdout. Trailing whitespace is allowed; logs belong on
stderr. Any extra stdout before or after the JSON value makes the reply
malformed. Canonical fixture examples live at
`crates/temper-process-protocol/fixtures/interactive-responder-request.json` and
`crates/temper-process-protocol/fixtures/interactive-responder-reply.json`.

```json
{
  "profile_id": "product-manager",
  "conversation_id": "pc-123",
  "turns": [
    {
      "participant": { "kind": "human", "display_name": "human" },
      "body": "Can we dogfood this from mobile?"
    }
  ],
  "context": {
    "repository": "ai/temper",
    "transcript_url": "https://git.example.test/ai/temper/issues/3"
  }
}
```

```json
{
  "message": "I would start with one small mobile text loop.",
  "proposals": [
    {
      "id": "mobile-text-loop",
      "kind": "issue",
      "title": "Add mobile text loop",
      "summary": "Lets humans dogfood from a phone.",
      "payload": {
        "title": "Add mobile text loop",
        "body": "Create a small mobile-friendly text adapter.",
        "rationale": "Lets humans dogfood from a phone."
      }
    }
  ]
}
```

## Validation and failures

Before a reply is persisted or exposed, Temper deserializes the reply through the
protocol DTOs, validates duplicate proposal ids, and decodes built-in `issue`
proposal payloads. Proposal ids and kinds must use the deterministic slug rule.
Issue proposals remain inert until a human explicitly accepts them through the
interaction/proposal acceptance path.

The adapter reports structured interaction errors for spawn/stdin/wait I/O,
timeout, nonzero exit, malformed JSON, and duplicate proposal ids. A timeout
drops and kills the child process.
